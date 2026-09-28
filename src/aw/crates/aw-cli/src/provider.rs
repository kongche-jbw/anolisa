//! The common Provider protocol shares one deadline across discovery, validation and invocation.
use crate::{
    mapping,
    model::{Configuration, ProcessConfig, Request},
    process, Error,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const VERSION: &str = "aw-provider/v1alpha1";
static SERIAL: AtomicU64 = AtomicU64::new(1);
static CANCEL_CHECK: AtomicBool = AtomicBool::new(false);

extern "C" fn cancel_check(_: i32) {
    CANCEL_CHECK.store(true, Ordering::Relaxed);
}

struct CheckSignals(Vec<(i32, libc::sigaction)>);

impl CheckSignals {
    fn install() -> Result<Self, Error> {
        CANCEL_CHECK.store(false, Ordering::Relaxed);
        let mut guard = Self(Vec::new());
        for signal in [libc::SIGINT, libc::SIGTERM] {
            // Both structures are initialized; the handler only writes an atomic flag.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = cancel_check as *const () as usize;
            if unsafe { libc::sigaction(signal, &action, &mut old) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            guard.0.push((signal, old));
        }
        Ok(guard)
    }
}

impl Drop for CheckSignals {
    fn drop(&mut self) {
        for (signal, old) in &self.0 {
            // Restore the disposition before the Agent launcher installs its handlers.
            unsafe { libc::sigaction(*signal, old, std::ptr::null_mut()) };
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    name: String,
    events: Vec<String>,
    effects: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Description {
    api_version: String,
    request_id: String,
    status: String,
    operations: Option<Vec<Operation>>,
    error_code: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Validation {
    api_version: String,
    request_id: String,
    status: String,
    error_code: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Effect {
    #[serde(rename = "type")]
    kind: String,
    reason_code: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Invocation {
    api_version: String,
    request_id: String,
    status: String,
    input_digest: Option<String>,
    effects: Option<Vec<Effect>>,
    error_code: Option<String>,
}

fn request(method: &str) -> Result<Value, Error> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(
        json!({"api_version":VERSION,"method":method,"request_id":format!("{}-{now}-{}",std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed))}),
    )
}

fn remaining(deadline: Instant) -> Result<u64, Error> {
    let millis = deadline
        .saturating_duration_since(Instant::now())
        .as_millis();
    if millis == 0 {
        return Err("provider_deadline_exceeded".into());
    }
    Ok(millis as u64)
}

fn exchange<T: DeserializeOwned>(
    process: &ProcessConfig,
    request: &Value,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<T, Error> {
    let bytes = serde_json::to_vec(request)?;
    let output = process::run(process, &[], &bytes, remaining(deadline)?, cancel)?;
    // Provider stderr and free-form error text are neither host responses nor audit metadata.
    if output.exit_code != 0 {
        return Err("provider_nonzero_exit".into());
    }
    remaining(deadline)?;
    serde_json::from_slice(&output.stdout).map_err(|_| "provider_invalid_response".into())
}

fn header(
    version: &str,
    id: &str,
    status: &str,
    error: &Option<String>,
    request: &Value,
) -> Result<(), Error> {
    if version != VERSION || request["request_id"] != id || status != "ok" || error.is_some() {
        return Err("provider_response_rejected".into());
    }
    Ok(())
}

fn admit(
    config: &Configuration,
    callback: &Request,
    process: &ProcessConfig,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let step = config.step(callback)?;
    let describe = request("describe")?;
    let reply: Description = exchange(process, &describe, deadline, cancel)?;
    header(
        &reply.api_version,
        &reply.request_id,
        &reply.status,
        &reply.error_code,
        &describe,
    )?;
    let operations = reply.operations.ok_or("provider_missing_operations")?;
    if operations.is_empty() || operations.len() > 64 {
        return Err("provider_invalid_operations".into());
    }
    let mut names = BTreeSet::new();
    for operation in &operations {
        if operation.name.is_empty() || !names.insert(&operation.name) {
            return Err("provider_duplicate_or_empty_operation".into());
        }
    }
    let operation = operations
        .iter()
        .find(|operation| step["operation"] == operation.name)
        .ok_or("provider_unsupported_operation")?;
    if !operation.events.contains(&callback.event)
        || crate::model::strings(&step["effects"])?
            .iter()
            .any(|effect| !operation.effects.contains(effect))
    {
        return Err("provider_unsupported_event_or_effect".into());
    }
    let mut validate = request("validate_config")?;
    validate["config"] = config.value["spec"]["providers"][&callback.provider]["config"].clone();
    let reply: Validation = exchange(process, &validate, deadline, cancel)?;
    header(
        &reply.api_version,
        &reply.request_id,
        &reply.status,
        &reply.error_code,
        &validate,
    )
}

pub(crate) struct Outcome {
    pub output: process::Output,
    pub disposition: &'static str,
}

pub(crate) fn invoke(
    config: &Configuration,
    callback: &Request,
    cancel: &AtomicBool,
) -> Result<Outcome, Error> {
    let (process, timeout) = config.process(callback)?;
    let deadline = Instant::now() + Duration::from_millis(timeout);
    let adapter = config.agent(&callback.agent)?["adapter"]
        .as_str()
        .ok_or("missing adapter")?;
    let mut event = mapping::normalize(adapter, &callback.event, &callback.input)?;
    event["agent"]["instance_id"] = Value::Null;
    event["agent"]["binding_id"] = json!(callback.agent);
    let step = config.step(callback)?;
    admit(config, callback, &process, deadline, cancel)?;
    let mut input = request("invoke")?;
    input["operation"] = step["operation"].clone();
    input["config_revision"] = json!(config.revision);
    input["allowed_effects"] = step["effects"].clone();
    input["config"] = config.value["spec"]["providers"][&callback.provider]["config"].clone();
    input["event"] = event;
    input["budget_ms"] = json!(remaining(deadline)?);
    // Opaque request binding, not the stricter canonical wire format used by aw-contracts.
    // Hash the exact serialized invocation before adding its digest; private JSON stays lossless.
    let digest = format!("sha256:{:x}", Sha256::digest(serde_json::to_vec(&input)?));
    input["input_digest"] = json!(digest);
    let reply: Invocation = exchange(&process, &input, deadline, cancel)?;
    header(
        &reply.api_version,
        &reply.request_id,
        &reply.status,
        &reply.error_code,
        &input,
    )?;
    if reply.input_digest.as_deref() != Some(&digest) {
        return Err("provider_input_digest_mismatch".into());
    }
    let effects = reply.effects.ok_or("provider_missing_effects")?;
    if effects.len() > 64 {
        return Err("provider_too_many_effects".into());
    }
    let allowed = crate::model::strings(&step["effects"])?;
    let mut blocked = false;
    for effect in effects {
        if !allowed.contains(&effect.kind)
            || !matches!(effect.kind.as_str(), "observe" | "block")
            || (effect.kind == "block" && callback.event != "tool.before")
        {
            return Err("provider_effect_not_admitted".into());
        }
        if effect.reason_code.as_deref().is_some_and(|reason| {
            reason.is_empty()
                || reason.len() > 128
                || !reason
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
        }) {
            return Err("provider_invalid_reason_code".into());
        }
        blocked |= effect.kind == "block";
    }
    let (exit_code, stdout, stderr) = mapping::reply(adapter, blocked)?;
    Ok(Outcome {
        output: process::Output {
            exit_code,
            stdout,
            stderr,
        },
        disposition: if blocked { "block" } else { "observe" },
    })
}

pub(crate) fn check(config: &Configuration, agent: &str) -> Result<(), Error> {
    let _signals = CheckSignals::install()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    for hook in config.hooks(agent)? {
        if hook.on_error.is_none() {
            continue;
        }
        let mut callback = crate::launch::request("invoke");
        callback.agent = agent.into();
        callback.provider = hook.provider;
        callback.event = hook.event;
        callback.cwd = std::env::current_dir()?.to_string_lossy().into();
        callback.environment = std::env::vars().collect();
        let (process, timeout) = config.process(&callback)?;
        admit(
            config,
            &callback,
            &process,
            deadline.min(Instant::now() + Duration::from_millis(timeout)),
            &CANCEL_CHECK,
        )?;
    }
    if CANCEL_CHECK.load(Ordering::Relaxed) {
        return Err("provider_admission_cancelled".into());
    }
    Ok(())
}
