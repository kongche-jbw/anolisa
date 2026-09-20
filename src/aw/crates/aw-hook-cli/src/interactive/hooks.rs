//! Native callbacks bind immutable tool occurrences before invoking an observer.

use super::{evidence, storage, Binding, Error, State};
use aw_contracts::canonical;
use aw_core::ports::Cancellation;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Occurrence {
    pub session_id: String,
    pub attachment: u64,
    pub tool_use_id: String,
    pub tool_name: String,
    pub input_digest: String,
}

/// Records an authenticated native callback and invokes the optional observer.
///
/// Only explicit tool_guard, input_response, stop_response or tool_response
/// configuration returns control responses. Missing pre-tool evidence,
/// duplicates and stale attachments cannot run a handler or update current counts.
/// A reset during a handler call fences its completion into the old attachment.
///
/// # Errors
/// Rejects wrong ancestry/cwd/session, malformed events or inaccessible evidence.
pub fn callback(root: &Path, payload: Value) -> Result<Value, Error> {
    callback_with_cancellation(root, payload, &|| false)
}

/// Runs a callback with the embedding entrypoint's process-owned cancellation.
///
/// # Errors
/// Has the same failures as [`callback`]; cancellation prevents new dispatch and
/// interrupts an active Host exchange before its owned process group is reclaimed.
pub fn callback_with_cancellation(
    root: &Path,
    payload: Value,
    cancelled: &dyn Fn() -> bool,
) -> Result<Value, Error> {
    callback_inner(root, payload, cancelled, "observe")
}

/// Handles the explicitly selected PreToolUse safety entrypoint.
///
/// Unlike optional observation, the embedding must map every returned error to
/// a native block, including malformed input or missing binding/configuration.
/// It still cannot control native behavior when the entire helper is killed.
///
/// # Errors
/// Rejects other events, missing guard configuration and all callback failures.
pub fn guard_callback_with_cancellation(
    root: &Path,
    payload: Value,
    cancelled: &dyn Fn() -> bool,
) -> Result<Value, Error> {
    callback_inner(root, payload, cancelled, "tool")
}

/// Handles the explicitly selected UserPromptSubmit response entrypoint.
///
/// The embedding must map every error, including malformed input or missing
/// binding, to native exit 2. Killing the helper remains outside this guarantee.
///
/// # Errors
/// Rejects other events, missing response authority and invalid callback binding.
pub fn input_callback_with_cancellation(
    root: &Path,
    payload: Value,
    cancelled: &dyn Fn() -> bool,
) -> Result<Value, Error> {
    callback_inner(root, payload, cancelled, "input")
}

/// Handles an explicitly configured main-agent Stop check.
///
/// The embedding maps every error to a native stop response with diagnostics,
/// never exit 2 (which requests further work). Native display is not guaranteed;
/// killing the helper remains outside this contract.
///
/// # Errors
/// Rejects wrong events, missing authority and invalid callback binding.
pub fn stop_callback_with_cancellation(
    root: &Path,
    payload: Value,
    cancelled: &dyn Fn() -> bool,
) -> Result<Value, Error> {
    callback_inner(root, payload, cancelled, "stop")
}

fn callback_inner(
    root: &Path,
    payload: Value,
    cancelled: &dyn Fn() -> bool,
    entry: &str,
) -> Result<Value, Error> {
    let binding: Binding = storage::read(&root.join("binding.json"))?;
    if entry == "tool"
        && (binding.prepared.config.tool_guard.is_none()
            || binding.prepared.config.format != 2
            || payload["hook_event_name"] != "PreToolUse")
    {
        return Err(Error::Profile("configured PreToolUse guard required"));
    }
    if entry == "input"
        && (binding.prepared.config.input_response.is_none()
            || binding.prepared.config.format != 2
            || payload["hook_event_name"] != "UserPromptSubmit")
    {
        return Err(Error::Profile(
            "configured UserPromptSubmit response required",
        ));
    }
    if entry == "stop"
        && (binding.prepared.config.stop_response.is_none()
            || binding.prepared.config.format != 2
            || payload["hook_event_name"] != "Stop")
    {
        return Err(Error::Profile(
            "configured main-agent Stop response required",
        ));
    }
    let stop_response =
        binding.prepared.config.stop_response.is_some() && payload["hook_event_name"] == "Stop";
    let tool_response = binding.prepared.config.tool_response.is_some()
        && payload["hook_event_name"] == "PostToolUse"
        && payload["tool_name"] == "Bash";
    let input_response = binding.prepared.config.input_response.is_some()
        && payload["hook_event_name"] == "UserPromptSubmit";
    let guarded = binding.prepared.config.tool_guard.is_some()
        && payload["hook_event_name"] == "PreToolUse"
        && payload["tool_name"] == "Bash";
    let result = storage::descendant(binding.agent_pid, binding.agent_ticks).and_then(|_| {
        if binding.prepared.config.format == 2 {
            super::notifications::callback(root, &binding, &payload, &CancellationCheck(cancelled))
        } else {
            dispatch(root, &binding, &payload, &CancellationCheck(cancelled))
        }
    });
    if result.is_err() && !guarded {
        let _lock = storage::lock(root)?;
        let mut state: State = storage::read(&root.join("state.json"))?;
        if payload["session_id"].as_str() == state.session_id.as_deref() {
            state.gap = true;
            storage::replace(&root.join("state.json"), &state)?;
        }
    }
    if guarded && result.is_err() {
        return Ok(aw_adapters::qoder_tool::denied());
    }
    if input_response && result.is_err() {
        return Ok(aw_adapters::qoder_input::unavailable());
    }
    if stop_response && result.is_err() {
        return Ok(aw_adapters::qoder_stop::unavailable());
    }
    if tool_response && result.is_err() {
        return Ok(aw_adapters::qoder_result::unavailable());
    }
    result
}

pub(super) struct CancellationCheck<'a>(pub(super) &'a dyn Fn() -> bool);
impl Cancellation for CancellationCheck<'_> {
    fn is_cancelled(&self) -> bool {
        (self.0)()
    }
}

fn dispatch(
    root: &Path,
    binding: &Binding,
    payload: &Value,
    cancellation: &dyn Cancellation,
) -> Result<Value, Error> {
    if cancellation.is_cancelled() {
        return Err(Error::Profile("callback cancelled"));
    }
    if payload["cwd"].as_str() != binding.prepared.config.cwd()?.to_str()
        || payload
            .get("agent_id")
            .is_some_and(|value| !value.is_null())
    {
        return Err(Error::Profile(
            "callback is outside the main native workspace",
        ));
    }
    let session = identifier(payload, "session_id")?;
    let kind = identifier(payload, "hook_event_name")?;
    let lock = storage::lock(root)?;
    let mut state: State = storage::read(&root.join("state.json"))?;
    if kind == "SessionStart" {
        if let Err(error) = start(&mut state, session, payload) {
            state.gap = true;
            if state.session_id.as_deref() == Some(session) && payload["source"] == "clear" {
                state.attached = false;
            }
            storage::replace(&root.join("state.json"), &state)?;
            return Err(error);
        }
        storage::replace(&root.join("state.json"), &state)?;
        return Ok(json!({}));
    }
    if state.session_id.as_deref() != Some(session) || !state.attached {
        return Err(Error::Profile("callback belongs to an inactive attachment"));
    }
    if kind == "SessionEnd" {
        state.attached = false;
        storage::replace(&root.join("state.json"), &state)?;
        return Ok(json!({}));
    }
    if !matches!(kind, "PreToolUse" | "PostToolUse" | "PostToolUseFailure") {
        return Err(Error::Profile("unsupported native event"));
    }
    let tool = identifier(payload, "tool_use_id")?;
    let name = identifier(payload, "tool_name")?;
    let key = canonical::document_digest(&json!([session, tool])).map_err(evidence)?;
    let call = root.join("calls").join(key);
    if kind == "PreToolUse" {
        if state.occurrences >= 1024 || payload.get("tool_input").is_none() {
            state.gap = true;
            storage::replace(&root.join("state.json"), &state)?;
            return Err(Error::Profile(
                "observation capacity exceeded or input missing",
            ));
        }
        let occurrence = Occurrence {
            session_id: session.into(),
            attachment: state.attachment,
            tool_use_id: tool.into(),
            tool_name: name.into(),
            input_digest: canonical::digest(
                &serde_json::to_vec(&payload["tool_input"]).map_err(evidence)?,
            ),
        };
        storage::directory(&call)?;
        storage::create(&call.join("before.json"), &occurrence)?;
        state.occurrences += 1;
        storage::replace(&root.join("state.json"), &state)?;
        return Ok(json!({}));
    }
    let occurrence: Occurrence = storage::read(&call.join("before.json"))?;
    if occurrence.attachment != state.attachment
        || occurrence.tool_name != name
        || occurrence.session_id != session
        || occurrence.tool_use_id != tool
        || payload.get("tool_input").is_some_and(|input| {
            serde_json::to_vec(input)
                .map(|bytes| canonical::digest(&bytes) != occurrence.input_digest)
                .unwrap_or(true)
        })
    {
        return Err(Error::Profile(
            "tool occurrence changed or attachment retired",
        ));
    }
    // Reserve before dispatch. Failed or interrupted calls are never replayed.
    storage::create(&call.join("claimed.json"), &json!({"native_event":kind}))?;
    drop(lock);
    let event = json!({
        "format":1,"event":"tool.result_observed","evidence":"native_callback",
        "runtime_id":binding.runtime_id,"runtime_generation":1,
        "source_epoch":binding.runtime_id,"session_id":session,
        "attachment":occurrence.attachment,"tool_use_id":tool,"tool_name":name,
        "config_revision":binding.prepared.revision,
        "outcome":if kind == "PostToolUse" { "success" } else { "failure" }
    });
    let handler = binding.prepared.config.legacy_handler()?;
    let observed = (|| {
        handler.check_pins().map_err(evidence)?;
        let output = aw_host_process::run(
            handler,
            &[],
            &serde_json::to_vec(&event).map_err(evidence)?,
            handler.limits.timeout_ms,
            cancellation,
        )
        .map_err(evidence)?;
        handler.check_pins().map_err(evidence)?;
        Ok::<_, Error>(
            !cancellation.is_cancelled()
                && output.exit_code == 0
                && canonical::parse(&output.stdout).map_err(evidence)?
                    == json!({"format":1,"observed":true}),
        )
    })()
    .unwrap_or(false);
    let _lock = storage::lock(root)?;
    let current: State = storage::read(&root.join("state.json"))?;
    let stale = current.attachment != occurrence.attachment || !current.attached;
    storage::create(
        &call.join("completion.json"),
        &json!({"observed":observed,"stale":stale}),
    )?;
    if !observed || stale {
        Ok(
            json!({"systemMessage":"AW optional observation unavailable or attachment changed; native result unchanged."}),
        )
    } else {
        Ok(json!({}))
    }
}

pub(super) fn start(state: &mut State, session: &str, payload: &Value) -> Result<(), Error> {
    let source = identifier(payload, "source")?;
    if !matches!(source, "startup" | "resume" | "clear" | "compact" | "new") {
        return Err(Error::Profile("unknown native session source"));
    }
    if source == "compact" && state.session_id.as_deref() == Some(session) && state.attached {
        state.session_start_observed = true;
        return Ok(());
    }
    if state.retired_sessions.iter().any(|old| old == session) || state.attachment >= 128 {
        return Err(Error::Profile(
            "retired session or attachment capacity exceeded",
        ));
    }
    if let Some(old) = &state.session_id {
        if old == session {
            // Without a new native session identity, late callbacks cannot be
            // distinguished. Do not relabel them as a fresh attachment.
            return Err(Error::Profile(
                "session reset must provide a new native identity",
            ));
        }
        state.retired_sessions.push(old.clone());
    }
    state.session_id = Some(session.into());
    state.session_start_observed = true;
    state.attachment += 1;
    state.attached = true;
    Ok(())
}

fn identifier<'a>(payload: &'a Value, key: &str) -> Result<&'a str, Error> {
    payload[key]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or(Error::Evidence)
}
