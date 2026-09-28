//! Translate native tool hooks without granting permissions or inferring tool semantics.

use crate::Error;
use serde_json::{json, Value};

const BLOCK_REASON: &str = "AW policy blocked this tool call";
const REPORT_DIAGNOSTIC: &[u8] =
    b"AW Provider invocation failed; continuing according to on_error=report\n";

/// Preserve the native payload alongside the common tool-event fields.
pub(crate) fn normalize(adapter: &str, event: &str, input: &[u8]) -> Result<Value, Error> {
    check_adapter(adapter)?;
    let before = match event {
        "tool.before" => true,
        "tool.after" => false,
        _ => return Err("unsupported AW tool event".into()),
    };
    let native: Value = serde_json::from_slice(input)?;
    if !native.is_object() {
        return Err("native tool hook payload must be a JSON object".into());
    }

    let (name, arguments, call_id, session_id, result) = match adapter {
        "qoder" => {
            require_marker(
                &native,
                "hook_event_name",
                if before { "PreToolUse" } else { "PostToolUse" },
            )?;
            (
                tool_name(native.get("tool_name"))?,
                tool_input(native.get("tool_input"))?.clone(),
                optional_id(native.get("tool_use_id"))?,
                optional_id(native.get("session_id"))?,
                native.get("tool_response"),
            )
        }
        "openclaw" => {
            require_marker(
                &native,
                "hook",
                if before {
                    "before_tool_call"
                } else {
                    "after_tool_call"
                },
            )?;
            let payload = object_field(&native, "event")?;
            let context = optional_object_field(&native, "context")?;
            let call_id = payload
                .get("toolCallId")
                .filter(|id| !id.is_null())
                .or_else(|| context.and_then(|value| value.get("toolCallId")));
            (
                tool_name(payload.get("toolName"))?,
                tool_input(payload.get("params"))?.clone(),
                optional_id(call_id)?,
                optional_id(context.and_then(|value| value.get("sessionId")))?,
                payload.get("result"),
            )
        }
        "hermes" => {
            require_marker(
                &native,
                "hook_event_name",
                if before {
                    "pre_tool_call"
                } else {
                    "post_tool_call"
                },
            )?;
            let extra = optional_object_field(&native, "extra")?;
            (
                tool_name(native.get("tool_name"))?,
                tool_input(native.get("tool_input"))?.clone(),
                optional_id(extra.and_then(|value| value.get("tool_call_id")))?,
                optional_id(native.get("session_id"))?,
                extra.and_then(|value| value.get("result")),
            )
        }
        "qwenpaw" => {
            require_marker(&native, "event", event)?;
            let call = object_field(&native, "tool_call")?;
            let raw = call
                .get("input")
                .and_then(Value::as_str)
                .ok_or("QwenPaw tool input must be a JSON string")?;
            let arguments: Value = serde_json::from_str(raw)?;
            tool_input(Some(&arguments))?;
            (
                tool_name(call.get("name"))?,
                arguments,
                optional_id(call.get("id"))?,
                Value::Null,
                native.get("tool_response"),
            )
        }
        // check_adapter rejects unsupported adapters before decoding the payload.
        _ => unreachable!("adapter was validated"),
    };

    Ok(json!({
        "name": event,
        "agent": {"adapter": adapter},
        "session_id": session_id,
        "tool": {
            "name": name,
            "native_name": name,
            "call_id": call_id,
            "input": arguments,
            "result": if before { Value::Null } else { result.cloned().unwrap_or(Value::Null) },
        },
        "native": native,
    }))
}

/// An allow is neutral: native permissions and other hooks retain their authority.
pub(crate) fn reply(adapter: &str, blocked: bool) -> Result<(i32, Vec<u8>, Vec<u8>), Error> {
    check_adapter(adapter)?;
    if !blocked {
        return Ok((0, b"{}\n".to_vec(), Vec::new()));
    }
    match adapter {
        "qoder" | "qwenpaw" => Ok((2, Vec::new(), format!("{BLOCK_REASON}\n").into_bytes())),
        "openclaw" | "hermes" => {
            let output = if adapter == "openclaw" {
                json!({"block": true, "blockReason": BLOCK_REASON})
            } else {
                json!({"action": "block", "message": BLOCK_REASON})
            };
            let mut stdout = serde_json::to_vec(&output)?;
            stdout.push(b'\n');
            Ok((0, stdout, Vec::new()))
        }
        // check_adapter rejects unsupported adapters before selecting a response.
        _ => unreachable!("adapter was validated"),
    }
}

/// Translate a configured failure policy before the host can interpret an error as allow.
pub(crate) fn failure(
    adapter: &str,
    event: &str,
    on_error: &str,
) -> Result<(i32, Vec<u8>, Vec<u8>), Error> {
    match (event, on_error) {
        ("tool.before", "block") => reply(adapter, true),
        ("tool.before" | "tool.after", "report") => {
            let (code, stdout, _) = reply(adapter, false)?;
            Ok((code, stdout, REPORT_DIAGNOSTIC.to_vec()))
        }
        _ => Err("unsupported tool-event failure policy".into()),
    }
}

fn check_adapter(adapter: &str) -> Result<(), Error> {
    if matches!(adapter, "qoder" | "openclaw" | "hermes" | "qwenpaw") {
        Ok(())
    } else {
        Err("unsupported native tool adapter".into())
    }
}

fn require_marker(payload: &Value, key: &str, expected: &str) -> Result<(), Error> {
    if payload.get(key).and_then(Value::as_str) != Some(expected) {
        return Err("native hook event does not match the AW binding".into());
    }
    Ok(())
}

fn tool_name(value: Option<&Value>) -> Result<&str, Error> {
    value
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| "native tool name must be a nonempty string".into())
}

fn tool_input(value: Option<&Value>) -> Result<&Value, Error> {
    value
        .filter(|value| value.is_object())
        .ok_or_else(|| "native tool input must be a JSON object".into())
}

fn optional_id(value: Option<&Value>) -> Result<Value, Error> {
    match value {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(_)) => Ok(value.cloned().unwrap_or(Value::Null)),
        _ => Err("native session and tool-call identifiers must be strings or null".into()),
    }
}

fn object_field<'a>(payload: &'a Value, key: &str) -> Result<&'a Value, Error> {
    payload
        .get(key)
        .filter(|value| value.is_object())
        .ok_or_else(|| "native tool hook envelope is missing a JSON object".into())
}

fn optional_object_field<'a>(payload: &'a Value, key: &str) -> Result<Option<&'a Value>, Error> {
    match payload.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) if value.is_object() => Ok(Some(value)),
        _ => Err("native hook metadata must be a JSON object or null".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(adapter: &str, event: &str, payload: &Value) -> Value {
        normalize(adapter, event, &serde_json::to_vec(payload).unwrap()).unwrap()
    }

    #[test]
    fn qoder_preserves_custom_tool_names_and_native_metadata() {
        let native = json!({
            "hook_event_name": "PreToolUse", "tool_name": "mcp__custom__inspect",
            "tool_use_id": "fc-1", "session_id": "session-1", "permission_mode": "default",
            "tool_input": {"record": {"nested": true}, "items": [1, "two"]},
            "mcp_context": {"server_name": "custom"},
        });
        let output = map("qoder", "tool.before", &native);
        assert_eq!(output["name"], "tool.before");
        assert_eq!(output["agent"]["adapter"], "qoder");
        assert_eq!(output["tool"]["name"], "mcp__custom__inspect");
        assert_eq!(output["tool"]["native_name"], output["tool"]["name"]);
        assert_eq!(output["tool"]["input"], native["tool_input"]);
        assert_eq!(output["tool"]["call_id"], "fc-1");
        assert_eq!(output["session_id"], "session-1");
        assert_eq!(output["tool"]["result"], Value::Null);
        assert!(output["tool"].get("kind").is_none());
        assert_eq!(output["native"], native);
    }

    #[test]
    fn openclaw_uses_tool_event_and_context_identifiers() {
        let native = json!({
            "hook": "before_tool_call",
            "event": {"toolName": "exec", "params": {"command": "true"}, "toolCallId": "call-1"},
            "context": {"sessionId": "session-1", "toolCallId": "context-call", "requester": {"senderId": "u"}},
        });
        let output = map("openclaw", "tool.before", &native);
        assert_eq!(output["tool"]["call_id"], "call-1");
        assert_eq!(output["session_id"], "session-1");
        assert_eq!(output["tool"]["input"], json!({"command": "true"}));
        assert_eq!(output["native"], native);
        let mut context_only = native.clone();
        context_only["event"]
            .as_object_mut()
            .unwrap()
            .remove("toolCallId");
        assert_eq!(
            map("openclaw", "tool.before", &context_only)["tool"]["call_id"],
            "context-call"
        );
    }

    #[test]
    fn hermes_promotes_ids_but_preserves_serialized_result() {
        let native = json!({
            "hook_event_name": "post_tool_call", "tool_name": "terminal",
            "tool_input": {"command": "true"}, "session_id": "session-1",
            "extra": {"tool_call_id": "call-1", "result": "{\"exit_code\":0}", "status": "ok", "duration_ms": 3},
        });
        let output = map("hermes", "tool.after", &native);
        assert_eq!(output["tool"]["call_id"], "call-1");
        assert_eq!(output["tool"]["result"], "{\"exit_code\":0}");
        assert_eq!(output["native"], native);
    }

    #[test]
    fn qwenpaw_parses_arguments_and_preserves_tool_response() {
        let native = json!({
            "event": "tool.after",
            "tool_call": {"type": "tool_call", "id": "call-1", "name": "terminal", "input": "{\"command\":\"true\"}", "state": "allowed"},
            "tool_response": {"state": "success", "content": [{"type": "text", "text": "ok"}], "metadata": {}, "id": "call-1"},
        });
        let output = map("qwenpaw", "tool.after", &native);
        assert_eq!(output["tool"]["input"], json!({"command": "true"}));
        assert_eq!(output["tool"]["result"], native["tool_response"]);
        assert_eq!(output["tool"]["call_id"], "call-1");
        assert_eq!(output["session_id"], Value::Null);
        assert_eq!(output["native"], native);
    }

    #[test]
    fn after_keeps_result_types_including_null_and_error_only_events() {
        for result in [
            Value::Null,
            json!("raw text"),
            json!([1, 2]),
            json!({"stdout": "ok"}),
        ] {
            let native = json!({
                "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {}, "tool_response": result,
            });
            assert_eq!(
                map("qoder", "tool.after", &native)["tool"]["result"],
                result
            );
        }
        let native = json!({
            "hook": "after_tool_call", "event": {"toolName": "exec", "params": {}, "error": "cancelled"},
        });
        let output = map("openclaw", "tool.after", &native);
        assert_eq!(output["tool"]["result"], Value::Null);
        assert_eq!(output["tool"]["call_id"], Value::Null);
        assert_eq!(output["native"]["event"]["error"], "cancelled");
    }

    #[test]
    fn rejects_wrong_event_or_unsupported_native_boundaries() {
        for (adapter, native) in [
            (
                "qoder",
                json!({"hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {}}),
            ),
            (
                "hermes",
                json!({"hook_event_name": "post_tool_call", "tool_name": "terminal", "tool_input": {}}),
            ),
            (
                "qwenpaw",
                json!({"event": "tool.after", "tool_call": {"name": "terminal", "input": "{}"}}),
            ),
            (
                "openclaw",
                json!({"hook": "agent_tool_result", "event": {"toolName": "exec", "params": {}}}),
            ),
        ] {
            assert!(normalize(
                adapter,
                "tool.before",
                &serde_json::to_vec(&native).unwrap()
            )
            .is_err());
        }
        assert!(normalize("qoder", "turn.stop", b"{}").is_err());
        assert!(normalize("other", "tool.before", b"{}").is_err());
    }

    #[test]
    fn rejects_malformed_payload_names_inputs_and_identifiers() {
        for input in [b"null".as_slice(), b"[]", b"{", b"\xff"] {
            assert!(normalize("qoder", "tool.before", input).is_err());
        }
        let native =
            json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {}});
        for (field, value) in [
            ("tool_name", json!(" ")),
            ("tool_name", Value::Null),
            ("tool_input", json!("{}")),
            ("tool_input", Value::Null),
            ("session_id", json!(1)),
            ("tool_use_id", json!({})),
        ] {
            let mut invalid = native.clone();
            invalid[field] = value;
            assert!(normalize(
                "qoder",
                "tool.before",
                &serde_json::to_vec(&invalid).unwrap()
            )
            .is_err());
        }
        let missing = json!({"hook_event_name": "PreToolUse", "tool_name": "Bash"});
        assert!(normalize(
            "qoder",
            "tool.before",
            &serde_json::to_vec(&missing).unwrap()
        )
        .is_err());
    }

    #[test]
    fn qwenpaw_rejects_object_or_invalid_json_as_raw_arguments() {
        for arguments in [json!({}), json!("not-json"), json!("null"), json!("[]")] {
            let native = json!({"event": "tool.before", "tool_call": {"name": "terminal", "input": arguments}});
            assert!(normalize(
                "qwenpaw",
                "tool.before",
                &serde_json::to_vec(&native).unwrap()
            )
            .is_err());
        }
    }

    #[test]
    fn neutral_replies_do_not_auto_grant_native_permissions() {
        for adapter in ["qoder", "openclaw", "hermes", "qwenpaw"] {
            assert_eq!(
                reply(adapter, false).unwrap(),
                (0, b"{}\n".to_vec(), Vec::new())
            );
        }
    }

    #[test]
    fn deny_replies_match_each_native_control_channel() {
        for adapter in ["qoder", "qwenpaw"] {
            assert_eq!(
                reply(adapter, true).unwrap(),
                (2, Vec::new(), format!("{BLOCK_REASON}\n").into_bytes())
            );
        }
        for (adapter, expected) in [
            (
                "openclaw",
                json!({"block": true, "blockReason": BLOCK_REASON}),
            ),
            (
                "hermes",
                json!({"action": "block", "message": BLOCK_REASON}),
            ),
        ] {
            let (code, stdout, stderr) = reply(adapter, true).unwrap();
            assert_eq!(code, 0);
            assert_eq!(serde_json::from_slice::<Value>(&stdout).unwrap(), expected);
            assert!(stderr.is_empty());
        }
        assert!(reply("other", true).is_err());
        assert!(reply("other", false).is_err());
    }

    #[test]
    fn failure_policies_do_not_return_nonblocking_native_error_codes() {
        for adapter in ["qoder", "openclaw", "hermes", "qwenpaw"] {
            assert_eq!(
                failure(adapter, "tool.before", "block").unwrap(),
                reply(adapter, true).unwrap()
            );
            for event in ["tool.before", "tool.after"] {
                let (code, stdout, stderr) = failure(adapter, event, "report").unwrap();
                assert_eq!(code, 0);
                assert_eq!(stdout, b"{}\n");
                assert_eq!(stderr, REPORT_DIAGNOSTIC);
            }
            assert!(failure(adapter, "tool.after", "block").is_err());
            assert!(failure(adapter, "tool.before", "ignore").is_err());
            assert!(failure(adapter, "turn.stop", "report").is_err());
        }
        assert!(failure("other", "tool.before", "report").is_err());
    }
}
