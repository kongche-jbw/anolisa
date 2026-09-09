//! Runs the existing SecCore native content scanner through a bounded Host port.
//! Program/configuration pinning is local provenance, not sandbox attestation.

mod process;
mod response;

use aw_contracts::{canonical, Registry};
use aw_core::ports::{HostError, ProviderHost, ProviderResult};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

/// Independent hard limits; invocation budgets can only tighten them.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Maximum child lifetime, including pipe exchange.
    pub timeout_ms: u64,
    /// Maximum native request size in bytes.
    pub input_bytes: usize,
    /// Maximum native response size in bytes.
    pub output_bytes: usize,
    /// Maximum discarded diagnostic output in bytes.
    pub stderr_bytes: usize,
}

/// Explicit trusted launch configuration; no inherited environment is forwarded.
#[derive(Clone, Debug)]
pub struct Config {
    /// Host registry identity.
    pub provider_id: String,
    /// Operator-pinned Provider version, not an inferred scanner version.
    pub provider_version: String,
    /// Absolute executable path; executable bytes are pinned at construction.
    pub program: PathBuf,
    /// Exact arguments; an interpreter's imported dependencies are not attested.
    pub args: Vec<String>,
    /// Complete child environment, supplied by the trusted embedding application.
    pub environment: BTreeMap<String, String>,
    /// Hard resource bounds.
    pub limits: Limits,
}

/// Configuration or launch preparation failure, without Provider content.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Configuration cannot establish a bounded local execution.
    #[error("invalid SecCore Host configuration: {0}")]
    Configuration(&'static str),
    /// The configured executable could not be read.
    #[error("SecCore executable I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// The descriptor does not satisfy the existing AW contracts.
    #[error("SecCore Host contract failed: {0}")]
    Contract(#[from] aw_contracts::Error),
}

/// One explicitly registered SecCore native-protocol-1 content Provider.
pub struct SecHost {
    config: Config,
    program_digest: String,
    descriptor: Value,
    registry: Registry,
}

impl SecHost {
    /// Pins a launch configuration and validates its capability descriptor.
    ///
    /// # Errors
    /// Rejects non-Linux platforms, non-absolute executables and empty budgets.
    pub fn new(config: Config) -> Result<Self, Error> {
        if !cfg!(target_os = "linux") || !config.program.is_absolute() {
            return Err(Error::Configuration(
                "Linux and an absolute program are required",
            ));
        }
        let limits = &config.limits;
        if limits.timeout_ms == 0
            || limits.input_bytes == 0
            || limits.output_bytes == 0
            || limits.stderr_bytes == 0
        {
            return Err(Error::Configuration("limits must be nonzero"));
        }
        let program = config
            .program
            .to_str()
            .ok_or(Error::Configuration("program must be UTF-8"))?;
        let program_digest = canonical::digest(&fs::read(&config.program)?);
        let manifest = json!({"format":1,"provider_id":config.provider_id,"provider_version":config.provider_version,
            "program":program,"program_digest":program_digest,"args":config.args,"environment":config.environment,
            "limits":{"timeout_ms":limits.timeout_ms,"input_bytes":limits.input_bytes,"output_bytes":limits.output_bytes,"stderr_bytes":limits.stderr_bytes},
            "native_protocol":1,"operation":"content_inspect"});
        let registry = Registry::new()?;
        let descriptor = json!({"provider_id":config.provider_id,"provider_version":config.provider_version,
            "manifest_digest":canonical::document_digest(&manifest)?,"driver":"sec-core/native-stdio/v1",
            "lifecycle":"process/per-call/v1","guarantee":"declared","capabilities":[{
                "capability":"security.content.inspect/v2","authority":"advise",
                "input_schema":registry.reference("security-content-inspect-input-v2")?,
                "output_schema":registry.reference("security-content-inspect-output-v2")?,"boundaries":["post_tool"]}]});
        registry.validate("provider-descriptor-v1", &descriptor)?;
        Ok(Self {
            config,
            program_digest,
            descriptor,
            registry,
        })
    }

    fn inspect(&self, invocation: &Value, started: u64) -> Result<Option<Value>, &'static str> {
        let input = &invocation["input"];
        self.registry
            .validate("security-content-inspect-input-v2", input)
            .map_err(|_| "invalid_input")?;
        if input["boundary"] != "post_tool" {
            return Err("unsupported_boundary");
        }
        let content = input["artifact"]["content"]
            .as_str()
            .ok_or("invalid_content")?;
        if input["artifact"]["digest"] != canonical::digest(content.as_bytes())
            || invocation["input_digest"]
                != canonical::document_digest(input).map_err(|_| "invalid_input")?
        {
            return Err("input_digest_mismatch");
        }
        let bytes = canonical::bytes(input).map_err(|_| "invalid_input")?.len();
        if bytes as u64
            > invocation["budget"]["input_bytes"]
                .as_u64()
                .ok_or("invalid_budget")?
        {
            return Err("input_budget_exceeded");
        }
        let deadline = invocation["deadline_at_ms"]
            .as_u64()
            .ok_or("invalid_deadline")?;
        let remaining = deadline
            .checked_sub(started)
            .filter(|n| *n > 0)
            .ok_or("deadline_exceeded")?;
        let timeout_ms = remaining.min(self.config.limits.timeout_ms).min(
            invocation["budget"]["wall_time_ms"]
                .as_u64()
                .ok_or("invalid_budget")?,
        );
        if timeout_ms == 0 {
            return Err("deadline_exceeded");
        }
        let program_bytes = fs::read(&self.config.program).map_err(|_| "program_unavailable")?;
        if canonical::digest(&program_bytes) != self.program_digest {
            return Err("program_changed");
        }
        let request = serde_json::to_vec(&json!({"protocol_version":1,"operation":"content_inspect","content":content,"source":"tool_output",
            "include_low_confidence":input["constraints"]["include_low_confidence"]})).map_err(|_|"invalid_input")?;
        // Parsing, executable hashing and request encoding consume the same
        // invocation budget. Never launch with a stale pre-preparation timeout.
        let launch_at = now_ms().map_err(|_| "clock_unavailable")?;
        let timeout_ms = started
            .saturating_add(timeout_ms)
            .checked_sub(launch_at)
            .filter(|n| *n > 0)
            .ok_or("deadline_exceeded")?;
        let wire = process::run(&self.config, &request, timeout_ms)?;
        let output = response::translate(&wire, &input["artifact"], &self.registry)?;
        if let Some(output) = &output {
            let bytes = canonical::bytes(output)
                .map_err(|_| "invalid_response")?
                .len();
            if bytes as u64
                > invocation["budget"]["output_bytes"]
                    .as_u64()
                    .ok_or("invalid_budget")?
            {
                return Err("output_budget_exceeded");
            }
        }
        Ok(output)
    }
}

impl ProviderHost for SecHost {
    fn descriptor(&self, provider_id: &str) -> Option<&Value> {
        (self.descriptor["provider_id"] == provider_id).then_some(&self.descriptor)
    }

    fn invoke(&mut self, invocation: &Value) -> Result<ProviderResult, HostError> {
        self.registry
            .validate("capability-invocation-v1", invocation)
            .map_err(|_| host_error("invalid_invocation"))?;
        for key in ["provider_id", "provider_version", "manifest_digest"] {
            if invocation[key] != self.descriptor[key] {
                return Err(host_error("provider_binding_mismatch"));
            }
        }
        let cap = &self.descriptor["capabilities"][0];
        for key in ["capability", "input_schema", "output_schema"] {
            if invocation[key] != cap[key] {
                return Err(host_error("capability_binding_mismatch"));
            }
        }
        let started = now_ms()?;
        let mut result = self.inspect(invocation, started);
        let completed = now_ms()?;
        if completed < started {
            return Err(host_error("clock_regressed"));
        }
        if result.is_ok()
            && (completed
                > invocation["deadline_at_ms"]
                    .as_u64()
                    .ok_or_else(|| host_error("invalid_deadline"))?
                || completed - started
                    > invocation["budget"]["wall_time_ms"]
                        .as_u64()
                        .ok_or_else(|| host_error("invalid_budget"))?)
        {
            result = Err("provider_time_budget_exceeded");
        }
        let mut receipt = json!({"disposition":"failed","meters":[],"evidence":[],"started_at_ms":started,"completed_at_ms":completed});
        for key in [
            "invocation_id",
            "provider_id",
            "provider_version",
            "manifest_digest",
            "capability",
            "scope",
            "input_schema",
            "input_digest",
            "plan_ref",
        ] {
            receipt[key] = invocation[key].clone();
        }
        let output = match result {
            Ok(Some(output)) => {
                receipt["disposition"] = json!("produced");
                receipt["output"] = json!({"schema":invocation["output_schema"],"digest":canonical::document_digest(&output).map_err(|_|host_error("invalid_output"))?,
                    "bytes":canonical::bytes(&output).map_err(|_|host_error("invalid_output"))?.len()});
                Some(output)
            }
            Ok(None) => {
                receipt["disposition"] = json!("bypassed");
                None
            }
            Err(code) => {
                receipt["error_code"] = json!(code);
                None
            }
        };
        self.registry
            .validate("provider-receipt-v1", &receipt)
            .map_err(|_| host_error("invalid_receipt"))?;
        Ok(ProviderResult { receipt, output })
    }
}

fn host_error(code: &str) -> HostError {
    HostError { code: code.into() }
}

fn now_ms() -> Result<u64, HostError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| host_error("clock_unavailable"))?
        .as_millis();
    u64::try_from(millis).map_err(|_| host_error("clock_overflow"))
}
