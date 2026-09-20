//! Optional result projection bound to the interactive owner, never a synthetic Turn.

use super::{evidence, storage, Binding, Error, State};
use aw_contracts::{canonical, events::Notification, tool_response::ToolResponse};
use aw_core::{
    journal::FileJournal,
    ports::{Cancellation, HostError},
    tool_response::{ToolResponseCommand, ToolResponseHost},
    Core,
};
use aw_host_process::Config;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    pub command: Option<Config>,
    pub tokenless: Option<Config>,
    #[serde(default)]
    pub allow_text_reencoding: bool,
    pub accepted_reversibility: Vec<String>,
}

impl Settings {
    pub(super) fn provider(&self) -> Result<&Config, Error> {
        match (&self.command, &self.tokenless) {
            (Some(command), None) if !self.allow_text_reencoding => Ok(command),
            (None, Some(tokenless)) => Ok(tokenless),
            _ => Err(Error::Profile(
                "exactly one tool response provider required",
            )),
        }
    }
}

pub(super) fn respond(
    core: &Core,
    root: &Path,
    binding: &Binding,
    event: &Notification,
    cancellation: &dyn Cancellation,
    deadline: Instant,
) -> Result<Value, Error> {
    let settings = binding
        .prepared
        .config
        .tool_response
        .as_ref()
        .ok_or(Error::Evidence)?;
    let source = aw_adapters::qoder_result::source(&event.payload).map_err(evidence)?;
    let response = core
        .respond_to_tool(
            event,
            &ToolResponseCommand {
                id: &settings.provider()?.provider_id,
                source: &source,
                deadline,
            },
            &mut Host(settings),
            &mut FileJournal::new(root.join("tool-response-journal")).map_err(evidence)?,
            cancellation,
        )
        .map_err(evidence)?;
    let _lock = storage::lock(root)?;
    let mut state: State = storage::read(&root.join("state.json"))?;
    if cancellation.is_cancelled()
        || Instant::now() >= deadline
        || !state.attached
        || Some(state.attachment) != event.session_epoch
    {
        return Ok(aw_adapters::qoder_result::unavailable());
    }
    Ok(match response {
        Some(ToolResponse::Preserve) => json!({}),
        Some(ToolResponse::Replace(text)) => aw_adapters::qoder_result::replacement(&text),
        None => {
            state.gap = true;
            storage::replace(&root.join("state.json"), &state)?;
            aw_adapters::qoder_result::unavailable()
        }
    })
}

struct Host<'a>(&'a Settings);
impl ToolResponseHost for Host<'_> {
    fn respond(
        &mut self,
        command: &str,
        request: &Value,
        remaining_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<Value, HostError> {
        let failure = || HostError {
            code: "tool_response_command_failed".into(),
        };
        let config = self.0.provider().map_err(|_| failure())?;
        if command != config.provider_id {
            return Err(failure());
        }
        if self.0.tokenless.is_some() {
            return tokenless_response(
                config,
                request,
                self.0.allow_text_reencoding,
                remaining_ms,
                cancellation,
            );
        }
        config.check_pins().map_err(|_| failure())?;
        let output = aw_host_process::run(
            config,
            &[],
            &canonical::bytes(request).map_err(|_| failure())?,
            remaining_ms.min(config.limits.timeout_ms),
            cancellation,
        )
        .map_err(|_| failure())?;
        config.check_pins().map_err(|_| failure())?;
        if output.exit_code != 0 {
            return Err(failure());
        }
        canonical::parse(&output.stdout).map_err(|_| failure())
    }
}

fn tokenless_response(
    config: &Config,
    request: &Value,
    allow_text_reencoding: bool,
    remaining_ms: u64,
    cancellation: &dyn Cancellation,
) -> Result<Value, HostError> {
    let deadline = Instant::now() + Duration::from_millis(remaining_ms);
    let (input, scope) = tokenless_input(request, allow_text_reencoding)?;
    let remaining_ms = deadline
        .saturating_duration_since(Instant::now())
        .as_millis() as u64;
    let candidate =
        aw_tokenless_host::post_tool::project(config, &input, &scope, remaining_ms, cancellation)?;
    Ok(match candidate {
        None => json!({"format":1,"decision":"preserve"}),
        Some(output) => json!({"format":1,"decision":"replace",
            "text":output["candidate"]["content"],"source_digest":output["candidate"]["source_digest"]}),
    })
}

fn tokenless_input(
    request: &Value,
    allow_text_reencoding: bool,
) -> Result<(Value, Value), HostError> {
    let failure = || HostError {
        code: "invalid_tokenless_tool_response_request".into(),
    };
    let event: Notification =
        serde_json::from_value(request["event"].clone()).map_err(|_| failure())?;
    event.validate().map_err(|_| failure())?;
    let source = aw_adapters::qoder_result::source(&event.payload).map_err(|_| failure())?;
    if request["format"] != 1
        || request["scope"] != "tool.after.respond"
        || request["accepted_reversibility"] != json!(["unrecoverable"])
        || event.event != aw_contracts::events::EventName::ToolAfter
        || event.source != "native_callback"
        || event.native_event.as_deref() != Some("PostToolUse")
        || event.subagent_id.is_some()
        || event.session_id.is_none()
        || event.tool_call_id.is_none()
        || event.payload["session_id"].as_str() != event.session_id.as_deref()
        || event.payload["tool_use_id"].as_str() != event.tool_call_id.as_deref()
        || event
            .payload
            .get("agent_id")
            .is_some_and(|id| !id.is_null())
        || request["source"]["text"] != source
        || request["source"]["digest"] != canonical::digest(source.as_bytes())
    {
        return Err(failure());
    }
    let input = json!({"boundary":"post_tool","artifact":{
        "id":event.event_key().map_err(|_| failure())?,"content":source,
        "digest":request["source"]["digest"],"media_type":"text/plain",
        "origin":"command_output","tool_name":"Bash"
    },"constraints":{"accepted_reversibility":["unrecoverable"],"allow_text_reencoding":allow_text_reencoding}});
    // Runtime identity is the authenticated root Agent identity in this profile.
    // Native Tokenless attribution has no Turn or runtime-generation field.
    let scope = json!({"actor_id":event.runtime_id,"runtime_id":event.runtime_id,
        "runtime_generation":event.runtime_generation,"session_id":event.session_id,
        "session_epoch":event.session_epoch,"tool_use_id":event.tool_call_id});
    Ok((input, scope))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Value {
        json!({"format":1,"scope":"tool.after.respond","accepted_reversibility":["unrecoverable"],
            "source":{"text":"original","digest":canonical::digest(b"original")},
            "event":{"format":1,"event":"tool.after","source":"native_callback",
                "native_event":"PostToolUse","runtime_id":"runtime-one","runtime_generation":4,
                "session_id":"session-one","session_epoch":2,"turn_id":null,
                "turn_unknown_reason":"native_callback_does_not_prove_task_acceptance",
                "tool_call_id":"tool-one","subagent_id":null,"occurrence_id":"occurrence-one",
                "config_revision":"trusted-revision","payload":{
                    "hook_event_name":"PostToolUse","session_id":"session-one","tool_use_id":"tool-one",
                    "tool_name":"Bash","tool_response":{"kind":"completed","exitCode":0,"signal":null,
                        "interrupted":false,"isImage":false,"noOutputExpected":false,"stderr":"","stdout":"original"}}}})
    }

    #[test]
    fn tokenless_mapping_preserves_owner_identity_without_inventing_turn() {
        let (input, scope) = tokenless_input(&request(), false).unwrap();
        assert_eq!(scope["actor_id"], "runtime-one");
        assert_eq!(scope["runtime_generation"], 4);
        assert_eq!(scope["session_epoch"], 2);
        assert_eq!(scope["tool_use_id"], "tool-one");
        assert!(scope.get("turn_id").is_none());
        assert_eq!(input["constraints"]["allow_text_reencoding"], false);
    }

    #[test]
    fn tokenless_mapping_rejects_wrong_scope_source_status_or_attribution() {
        for case in 0..11 {
            let mut request = request();
            match case {
                0 => request["scope"] = json!("input.submit.respond"),
                1 => request["source"]["digest"] = json!("0".repeat(64)),
                2 => request["source"]["text"] = json!("foreign"),
                3 => request["event"]["payload"]["tool_response"]["exitCode"] = json!(7),
                4 => request["event"]["payload"]["tool_name"] = json!("Read"),
                5 => request["event"]["payload"]["session_id"] = json!("foreign"),
                6 => request["event"]["payload"]["tool_use_id"] = json!("foreign"),
                7 => request["event"]["payload"]["agent_id"] = json!("child"),
                8 => request["event"]["source"] = json!("runtime_owner"),
                9 => request["event"]["session_epoch"] = Value::Null,
                _ => request["accepted_reversibility"] = json!(["lossless"]),
            }
            assert!(tokenless_input(&request, false).is_err(), "{case}");
        }
    }
}
