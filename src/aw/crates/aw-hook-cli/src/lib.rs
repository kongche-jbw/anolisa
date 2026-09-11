//! Opt-in native hook composition; security inspection is observational only.

mod input;
mod owner;

pub use input::{parse_payload, read_settings, read_stdin};
pub use owner::process_identity;

use aw_adapters::{AdaptedExecution, Adapter, CaptureRequest, Host, NativeContext, StepOptions};
use aw_contracts::{canonical, Registry};
use aw_core::{
    journal::FileJournal,
    ports::{Cancellation, Clock, NeverCancel, ProviderHost},
};
use aw_sec_host::{Config, SecHost};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

/// Maximum encoded settings or native hook input admitted by this entrypoint.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;

/// Operator-owned configuration, outside model-controlled tool arguments.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Runtime snapshot authenticated by the launcher and checked against scope.
    pub runtime: Value,
    /// Trusted scope; observed session/tool/turn IDs fill only omitted fields.
    pub scope: Value,
    /// Live Agent process that must be an ancestor of this hook.
    pub agent_pid: u32,
    /// Linux process start ticks, preventing reuse of the configured PID.
    pub agent_start_ticks: u64,
    /// Launcher-owned Qoder turn; only an explicitly bounded single turn is supported.
    pub qoder_single_turn_id: Option<String>,
    /// Absolute private directory for the existing Core journal.
    pub journal: PathBuf,
    /// Explicitly admitted native executable, environment and resource limits.
    pub provider: Config,
    /// Include native low-confidence findings when explicitly selected by policy.
    pub include_low_confidence: bool,
}

/// Non-sensitive failure categories; never serialize downstream errors or payloads.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Input or settings did not match the supported boundary.
    #[error("invalid hook input or settings")]
    Input,
    /// The hook is not bound to the same live Agent incarnation.
    #[error("hook owner identity mismatch")]
    Identity,
    /// The native provider could not be admitted.
    #[error("native inspection provider unavailable")]
    Provider,
    /// Core preparation, execution or journal verification failed.
    #[error("hook inspection could not be recorded")]
    Execution,
    /// A trusted wall clock could not be read.
    #[error("hook clock unavailable")]
    Clock,
}

/// Verified execution and a native observation response; no adoption assertion.
pub struct HookResult {
    /// Native hook response, containing no inspection input or evidence snippets.
    pub response: Value,
    /// Key of the durable Core event reservation.
    pub event_key: String,
    /// Core result retains receipt/output and the original native snapshot.
    pub execution: AdaptedExecution,
    /// True only when the inspection produced a verified output.
    pub inspected: bool,
}

struct WallClock;
struct SharedCancellation(Arc<dyn Cancellation + Send + Sync>);
impl Cancellation for SharedCancellation {
    fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}
impl Clock for WallClock {
    fn now_ms(&self) -> u64 {
        // Zero makes the existing Core reject a regressed/unavailable clock.
        wall_time().unwrap_or(0)
    }
}

fn wall_time() -> Result<u64, Error> {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Clock)?;
    u64::try_from(time.as_millis()).map_err(|_| Error::Clock)
}

/// Runs one supported hook through the real Adapter, Core, Host and FileJournal.
///
/// The launcher must keep settings and journal outside Agent-writable locations.
/// PID ancestry checks detect misbinding; they are not an isolation boundary
/// against a same-user adversary or an attestation of the supplied runtime.
///
/// # Errors
/// Rejects unsupported events, missing/conflicting identity, stale process owners,
/// inadmissible providers, repeated events and unacknowledged execution records.
pub fn inspect(host: Host, settings: Settings, payload: Value) -> Result<HookResult, Error> {
    inspect_with_cancellation(host, settings, payload, Arc::new(NeverCancel))
}

/// Runs an inspection with cancellation shared by Core and the native Host.
///
/// # Errors
/// Has the same admission and recording failures as [`inspect`]; cancellation
/// interrupts the native exchange and reclaims its owned process group.
pub fn inspect_with_cancellation(
    host: Host,
    settings: Settings,
    payload: Value,
    cancellation: Arc<dyn Cancellation + Send + Sync>,
) -> Result<HookResult, Error> {
    if !matches!(host, Host::Qoder | Host::Codex)
        || payload["hook_event_name"] != "PostToolUse"
        || !settings.scope.is_object()
        || !settings.journal.is_absolute()
    {
        return Err(Error::Input);
    }
    owner::verify(&settings)?;
    let mut scope = settings.scope.clone();
    for field in ["session_id", "tool_use_id"] {
        bind(&mut scope, field, identity(&payload, field)?)?;
    }
    let turn = match host {
        Host::Codex => identity(&payload, "turn_id")?,
        Host::Qoder => settings
            .qoder_single_turn_id
            .clone()
            .ok_or(Error::Identity)?,
        _ => return Err(Error::Input),
    };
    bind(&mut scope, "turn_id", turn)?;
    let event_id = canonical::document_digest(&json!({
        "scope":scope,"host":host.as_str(),"event":"PostToolUse"
    }))
    .map_err(|_| Error::Input)?;
    let adapter = Adapter::new(host).map_err(|_| Error::Execution)?;
    let captured = adapter
        .capture(CaptureRequest {
            native_event: "PostToolUse".into(),
            payload,
            context: NativeContext {
                scope,
                runtime: settings.runtime.clone(),
                event_id,
            },
        })
        .map_err(|_| Error::Input)?;
    let mut provider = SecHost::new_cancellable(settings.provider.clone(), cancellation.clone())
        .map_err(|_| Error::Provider)?;
    let descriptor = provider
        .descriptor(&settings.provider.provider_id)
        .ok_or(Error::Provider)?;
    let registry = Registry::new().map_err(|_| Error::Execution)?;
    let reference = |name| registry.reference(name).map_err(|_| Error::Execution);
    let plan = json!({
        "plan_id":captured.event_id(),"revision":1,"event_id":captured.event_id(),"scope":captured.scope(),
        "boundary_id":captured.boundary()["boundary_id"],"boundary_revision":captured.boundary()["revision"],
        "boundary":"post_tool","policy_revision":1,"source_digest":captured.artifact()["digest"],
        "steps":[{"step_id":"inspect","capability":"security.content.inspect/v2",
            "input_schema":reference("security-content-inspect-input-v2")?,
            "output_schema":reference("security-content-inspect-output-v2")?,
            "selection":"exactly_one","providers":[{
                "provider_id":descriptor["provider_id"],"provider_version":descriptor["provider_version"],
                "manifest_digest":descriptor["manifest_digest"]}],
            "required":true,"on_failure":"reject_plan","input_source":"boundary_source"}]
    });
    let now = wall_time()?;
    let timeout = settings.provider.limits.timeout_ms;
    let deadline = now.checked_add(timeout).ok_or(Error::Clock)?;
    let options = BTreeMap::from([(
        "inspect".into(),
        StepOptions {
            constraints: json!({"include_low_confidence":settings.include_low_confidence}),
            budget: json!({"input_bytes":MAX_INPUT_BYTES,"output_bytes":settings.provider.limits.output_bytes,
            "wall_time_ms":timeout}),
            deadline_at_ms: deadline,
        },
    )]);
    let prepared = adapter
        .prepare(captured, plan, options, &provider, now)
        .map_err(|_| Error::Execution)?;
    let event_key = prepared.event_key().to_owned();
    let mut journal = FileJournal::new(&settings.journal).map_err(|_| Error::Execution)?;
    // Recheck the live owner after preparation and before any inspection dispatch.
    owner::verify(&settings)?;
    let execution = adapter
        .execute(
            prepared,
            &mut provider,
            &mut journal,
            &WallClock,
            &SharedCancellation(cancellation.clone()),
        )
        .map_err(|_| Error::Execution)?;
    journal
        .read_verified(&event_key, execution.execution().journal_ack())
        .map_err(|_| Error::Execution)?;
    let verdict = execution
        .execution()
        .calls()
        .first()
        .and_then(|call| call.result().output.as_ref())
        .and_then(|output| output["inspection"]["verdict"].as_str());
    let inspected = verdict.is_some()
        && execution.execution().record()["decision"] == "proceed"
        && !cancellation.is_cancelled();
    let response = match verdict.filter(|_| inspected) {
        Some("clean") => json!({}),
        Some("suspicious" | "sensitive") => json!({"systemMessage":
            "AW found sensitive or suspicious content. Observation only; original tool result retained."}),
        _ => unavailable_response(),
    };
    Ok(HookResult {
        response,
        event_key,
        execution,
        inspected,
    })
}

/// Native failure response never permits, blocks or replaces a tool result.
pub fn unavailable_response() -> Value {
    json!({"systemMessage":"AW inspection unavailable; original tool result retained."})
}

fn identity(payload: &Value, field: &str) -> Result<String, Error> {
    payload[field]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(Error::Identity)
}

fn bind(scope: &mut Value, field: &str, value: String) -> Result<(), Error> {
    if scope.get(field).is_some() && scope[field] != value {
        return Err(Error::Identity);
    }
    scope[field] = json!(value);
    Ok(())
}
