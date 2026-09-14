//! Runs the existing SecCore native content scanner through a bounded Host port.
//! Program/configuration pinning is local provenance, not sandbox attestation.

use aw_host_process as process;
pub use aw_host_process::{Config, FilePin, Limits, PinState};
/// Native scan interface admitted independently of implementation language.
pub use aw_sec_core::PROTOCOL_PROFILE;

/// Native CLI version whose PII protocol was verified for this Host revision.
pub const NATIVE_CLI_VERSION: &str = "0.12.0";

use aw_contracts::{canonical, Registry};
use aw_core::ports::{Cancellation, HostError, NeverCancel, ProviderHost, ProviderResult};
use aw_sec_core::PiiRequest;
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

/// Configuration or launch preparation failure, without Provider content.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Configuration cannot establish a bounded local execution.
    #[error("invalid SecCore Host configuration: {0}")]
    Configuration(&'static str),
    /// The bounded native version probe failed.
    #[error("SecCore version probe failed: {0}")]
    Native(&'static str),
    /// The synthetic scan could not establish native protocol compatibility.
    #[error("SecCore protocol probe failed: {0}")]
    Protocol(&'static str),
    /// The descriptor does not satisfy the existing AW contracts.
    #[error("SecCore Host contract failed: {0}")]
    Contract(#[from] aw_contracts::Error),
}

/// One explicitly registered Provider using the native SecCore PII CLI protocol.
pub struct SecHost {
    config: Config,
    descriptor: Value,
    registry: Registry,
    cancellation: Arc<dyn Cancellation + Send + Sync>,
}

impl SecHost {
    /// Pins a launch configuration and validates its capability descriptor.
    ///
    /// # Errors
    /// Rejects non-Linux platforms, non-absolute executables and empty budgets.
    pub fn new(config: Config) -> Result<Self, Error> {
        Self::new_cancellable(config, Arc::new(NeverCancel))
    }

    /// Creates a Host whose version probe and native calls share caller cancellation.
    ///
    /// The caller owns signal handling and keeps the cancellation state alive.
    /// Cancellation stops pipe exchange and enters the same bounded group cleanup
    /// as other failures; this library installs no process-wide signal handlers.
    ///
    /// # Errors
    /// Returns configuration, admission or cancellation errors before a usable
    /// Host is returned. Native cleanup failure remains visible.
    pub fn new_cancellable(
        config: Config,
        cancellation: Arc<dyn Cancellation + Send + Sync>,
    ) -> Result<Self, Error> {
        config
            .validate()
            .map_err(|error| Error::Configuration(error.code()))?;
        let manifest = config
            .manifest(
                NATIVE_CLI_VERSION,
                aw_sec_core::PROTOCOL_PROFILE,
                "security.content.inspect/v2",
            )
            .map_err(|error| Error::Configuration(error.code()))?;
        let registry = Registry::new()?;
        let descriptor = json!({"provider_id":config.provider_id,"provider_version":config.provider_version,
            "manifest_digest":canonical::document_digest(&manifest)?,"driver":"sec-core/native-stdio/v1",
            "lifecycle":"process/per-call/v1","guarantee":"declared","capabilities":[{
                "capability":"security.content.inspect/v2","authority":"advise",
                "input_schema":registry.reference("security-content-inspect-input-v2")?,
                "output_schema":registry.reference("security-content-inspect-output-v2")?,"boundaries":["post_tool"]}]});
        registry.validate("provider-descriptor-v1", &descriptor)?;
        config
            .check_pins()
            .map_err(|error| Error::Configuration(error.code()))?;
        let version = process::run(
            &config,
            &["--version"],
            &[],
            config.limits.timeout_ms,
            cancellation.as_ref(),
        )
        .map_err(|error| Error::Native(error.code()))?;
        config
            .check_pins()
            .map_err(|error| Error::Configuration(error.code()))?;
        if cancellation.is_cancelled() {
            return Err(Error::Native("provider_cancelled"));
        }
        if version.exit_code != 0
            || version.stdout != format!("agent-sec-cli {NATIVE_CLI_VERSION}\n").as_bytes()
        {
            return Err(Error::Configuration("unsupported native CLI version"));
        }
        Ok(Self {
            config,
            descriptor,
            registry,
            cancellation,
        })
    }

    /// Verifies the native scan protocol using fixed public synthetic content.
    ///
    /// Uses the execution configuration unchanged, including custom rules and
    /// native audit behavior. Any complete valid verdict is admissible; success
    /// proves only this bounded exchange, not protection of a future Agent.
    /// Constructors and normal hooks do not call this startup-only probe.
    ///
    /// # Errors
    /// Rejects missing commands, incomplete or invalid responses, changed pins,
    /// exhausted budgets and cancellation without disclosing native output.
    pub fn probe_protocol(&self) -> Result<(), Error> {
        const TEXT: &str = "AW startup protocol probe.";
        let started = now_ms().map_err(|_| Error::Protocol("clock_unavailable"))?;
        let input = json!({"artifact":{"id":"aw-startup-protocol-probe",
            "digest":canonical::digest(TEXT.as_bytes()),"content":TEXT,
            "media_type":"text/plain","origin":"command_output"},
            "boundary":"post_tool","constraints":{"include_low_confidence":false}});
        let request = PiiRequest::new(&self.registry, &input)
            .map_err(|_| Error::Protocol("invalid_probe_input"))?;
        self.scan(&request, started, self.config.limits.timeout_ms)
            .map_err(Error::Protocol)?;
        let completed = now_ms().map_err(|_| Error::Protocol("clock_unavailable"))?;
        if self.cancellation.is_cancelled() {
            return Err(Error::Protocol("provider_cancelled"));
        }
        if completed < started {
            return Err(Error::Protocol("clock_regressed"));
        }
        if completed - started > self.config.limits.timeout_ms {
            return Err(Error::Protocol("provider_time_budget_exceeded"));
        }
        Ok(())
    }

    fn scan(
        &self,
        request: &PiiRequest<'_>,
        started: u64,
        timeout_ms: u64,
    ) -> Result<Value, &'static str> {
        if request.stdin().len() > self.config.limits.input_bytes {
            return Err("input_budget_exceeded");
        }
        self.config.check_pins().map_err(|error| error.code())?;
        // Parsing, executable hashing and request encoding consume the same
        // invocation budget. Never launch with a stale pre-preparation timeout.
        let launch_at = now_ms().map_err(|_| "clock_unavailable")?;
        let timeout_ms = started
            .saturating_add(timeout_ms)
            .checked_sub(launch_at)
            .filter(|n| *n > 0)
            .ok_or("deadline_exceeded")?;
        let wire = process::run(
            &self.config,
            &request.args(),
            request.stdin(),
            timeout_ms,
            self.cancellation.as_ref(),
        )
        .map_err(|error| error.code())?;
        self.config.check_pins().map_err(|error| error.code())?;
        request
            .project(wire.exit_code, &wire.stdout)
            .map_err(|error| match error {
                aw_sec_core::Error::NativeFailure => "native_failed",
                aw_sec_core::Error::OutputLimit => "native_output_limit",
                aw_sec_core::Error::IncompleteCoverage => "incomplete_coverage",
                _ => "invalid_native_response",
            })
    }

    fn inspect(&self, invocation: &Value, started: u64) -> Result<Value, &'static str> {
        let input = &invocation["input"];
        let request = PiiRequest::new(&self.registry, input).map_err(|_| "invalid_input")?;
        if invocation["input_digest"]
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
        let output = self.scan(&request, started, timeout_ms)?;
        let bytes = canonical::bytes(&output)
            .map_err(|_| "invalid_response")?
            .len();
        if bytes as u64
            > invocation["budget"]["output_bytes"]
                .as_u64()
                .ok_or("invalid_budget")?
        {
            return Err("output_budget_exceeded");
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
        if result.is_ok() && self.cancellation.is_cancelled() {
            result = Err("provider_cancelled");
        }
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
                        .ok_or_else(|| host_error("invalid_budget"))?
                || completed - started > self.config.limits.timeout_ms)
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
            Ok(output) => {
                receipt["disposition"] = json!("produced");
                receipt["output"] = json!({"schema":invocation["output_schema"],"digest":canonical::document_digest(&output).map_err(|_|host_error("invalid_output"))?,
                    "bytes":canonical::bytes(&output).map_err(|_|host_error("invalid_output"))?.len()});
                Some(output)
            }
            Err(code) => {
                receipt["error_code"] = json!(code);
                None
            }
        };
        self.registry
            .validate_result(invocation, &receipt, output.as_ref())
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
