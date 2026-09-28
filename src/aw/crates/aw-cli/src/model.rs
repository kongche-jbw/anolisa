//! Native-mode configuration admission; structured Providers remain a separate contract.
use crate::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub(crate) const MAX_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_TIMEOUT: u64 = 300_000;

#[derive(Clone)]
pub(crate) struct Limits {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub stderr_bytes: usize,
}
#[derive(Clone)]
pub(crate) struct ProcessConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub limits: Limits,
}
#[derive(Clone)]
pub(crate) struct Configuration {
    pub value: Value,
    pub revision: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Hook {
    pub event: String,
    pub provider: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequential: Option<bool>,
}
impl Configuration {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take((aw_config::MAX_DOCUMENT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        let parsed = aw_config::Validator::new()?.parse(&bytes)?;
        Ok(Self {
            value: parsed.as_value().clone(),
            revision: format!("{:x}", Sha256::digest(&bytes)),
        })
    }
    pub fn agent(&self, id: &str) -> Result<&Value, Error> {
        self.value["spec"]["agents"]
            .get(id)
            .ok_or_else(|| "unknown Agent ID".into())
    }
    pub fn hooks(&self, agent: &str) -> Result<Vec<Hook>, Error> {
        let adapter = self.agent(agent)?["adapter"]
            .as_str()
            .ok_or("missing adapter")?;
        let mut result = Vec::new();
        let events = self.value["spec"]["events"]
            .as_object()
            .ok_or("missing events")?;
        for (event, value) in events {
            if value["enabled"] != true {
                continue;
            }
            if !matches!(event.as_str(), "tool.before" | "tool.after") {
                return Err(format!("native lab cannot bind enabled event {event}").into());
            }
            if value.get("budget_ms").is_some() {
                return Err("native hosts own event scheduling; an AW shared event budget is not implemented".into());
            }
            if value.get("guard").is_some() {
                return Err("native scheduling cannot enforce an AW final guard".into());
            }
            if let Some(tools) = value["match"]["tools"].as_array() {
                if tools.len() != 1 || tools[0] != "*" {
                    return Err(
                        "native lab currently binds all tools; exact selectors are not installed"
                            .into(),
                    );
                }
            }
            let mut providers = std::collections::BTreeSet::new();
            for step in value["steps"].as_array().ok_or("missing steps")? {
                if step["enabled"] == false {
                    continue;
                }
                let native = step
                    .get("native")
                    .ok_or("structured Provider execution is not implemented by native lab")?;
                let provider = step["provider"].as_str().ok_or("missing provider")?;
                if !providers.insert(provider) {
                    return Err("native event cannot register the same Provider twice; name separate instances".into());
                }
                let specification = &self.value["spec"]["providers"][provider];
                let timeout = specification["timeout_ms"]
                    .as_u64()
                    .ok_or("missing timeout")?;
                let output = specification["max_output_bytes"]
                    .as_u64()
                    .ok_or("missing output limit")?;
                if timeout > MAX_TIMEOUT || output > MAX_BYTES as u64 {
                    return Err("native execution limit exceeds 300 seconds or 4 MiB".into());
                }
                let sequential = native["sequential"].as_bool();
                if sequential.is_some() && adapter != "qoder" {
                    return Err("native sequential override is Qoder-only".into());
                }
                if adapter == "openclaw" && timeout > 12000 {
                    return Err("OpenClaw lab Provider timeout must fit the native hook window (at most 12000ms)".into());
                }
                if adapter == "qwenpaw" && timeout > 58000 {
                    return Err(
                        "QwenPaw bridge timeout is 60s; Provider must be at most 58000ms".into(),
                    );
                }
                if adapter == "hermes" && timeout > 298000 {
                    return Err(
                        "Hermes native timeout is at most 300s; Provider must be at most 298000ms"
                            .into(),
                    );
                }
                let priority = native["priority"].as_i64();
                let point = native["point"].as_str().map(str::to_owned);
                if priority.is_some() && matches!(adapter, "qoder" | "hermes") {
                    return Err("this native shell-hook framework has no priority field".into());
                }
                if point.as_deref().is_some_and(|p| {
                    adapter != "openclaw"
                        || event != "tool.after"
                        || !matches!(p, "tool_result_persist" | "agent_tool_result")
                }) {
                    return Err("unsupported native point; only OpenClaw tool.after/tool_result_persist or agent_tool_result is installed".into());
                }
                if point.as_deref() == Some("agent_tool_result") && priority.is_some() {
                    return Err(
                        "OpenClaw result middleware follows registration order, not priority"
                            .into(),
                    );
                }
                result.push(Hook {
                    event: event.clone(),
                    provider: provider.into(),
                    id: step["id"].as_str().ok_or("missing step ID")?.into(),
                    priority,
                    point,
                    sequential,
                });
            }
        }
        Ok(result)
    }
    pub fn process(&self, request: &Request) -> Result<(ProcessConfig, u64), Error> {
        if request.input.len() > MAX_BYTES {
            return Err("native input exceeds 4 MiB".into());
        }
        if !self
            .hooks(&request.agent)?
            .iter()
            .any(|h| h.event == request.event && h.provider == request.provider)
        {
            return Err("Provider is not bound to this Agent/event".into());
        }
        let spec = &self.value["spec"]["providers"][&request.provider];
        let argv = strings(&spec["transport"]["argv"])?;
        let mut environment = request.environment.clone();
        if let Some(values) = spec["transport"]["env"].as_object() {
            for (key, value) in values {
                environment.insert(
                    key.clone(),
                    value.as_str().ok_or("invalid environment")?.into(),
                );
            }
        }
        let cwd = PathBuf::from(&request.cwd);
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Err("callback cwd must be an existing absolute directory".into());
        }
        let limit = spec["max_output_bytes"].as_u64().ok_or("missing limit")? as usize;
        Ok((
            ProcessConfig {
                program: argv[0].clone().into(),
                args: argv[1..].to_vec(),
                cwd,
                environment,
                limits: Limits {
                    input_bytes: MAX_BYTES,
                    output_bytes: limit,
                    stderr_bytes: limit,
                },
            },
            spec["timeout_ms"].as_u64().ok_or("missing timeout")?,
        ))
    }
}
pub(crate) fn strings(value: &Value) -> Result<Vec<String>, Error> {
    value
        .as_array()
        .ok_or("expected argv array")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "expected string argument".into())
        })
        .collect()
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub op: String,
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub input: Vec<u8>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Response {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub error: Option<String>,
    pub revision: String,
}
