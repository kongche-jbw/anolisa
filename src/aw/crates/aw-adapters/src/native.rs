//! Extracts supported text slots without converting arbitrary tool data to code.

use crate::{Error, Host};
use serde_json::Value;

/// Native observations to compare with the caller's trusted runtime binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedCall {
    /// Exact native tool name; no cross-framework aliases are inferred.
    pub tool_name: String,
    /// An observed call identifier, never a source of authority.
    pub tool_use_id: Option<String>,
    /// An observed session identifier, never a source of authority.
    pub session_id: Option<String>,
    /// A Codex turn or OpenClaw run observed for the local turn binding.
    pub turn_id: Option<String>,
    /// Exact command or result text, including its original whitespace.
    pub text: String,
    /// Contract origin; only identified shell output uses `command_output`.
    pub origin: String,
}

/// Extracts a supported native hook payload without modifying it.
///
/// OpenClaw uses the local container `{event, context}` for its two callback
/// arguments. Hermes uses the same local container for its named tool arguments
/// (`event`) and remaining keyword arguments (`context`). Neither container is
/// claimed to be a native wire format. COSH post-tool extraction reads the
/// explicit `tool_response.llmContent` string and preserves `returnDisplay` in
/// the caller's snapshot. Qoder also accepts a completed Bash stdout object. Other hosts accept direct strings only; arbitrary
/// structured, binary and multi-block results remain unsupported.
///
/// # Errors
/// Returns an error for unsupported events, non-shell pre-tool calls, malformed
/// fields, conflicting observations or unsupported result representations.
pub fn extract(host: Host, native_event: &str, payload: &Value) -> Result<ExtractedCall, Error> {
    crate::profiles::boundary(host, native_event)?;
    let pre = matches!(
        native_event,
        "PreToolUse" | "pre_tool_call" | "before_tool_call"
    );
    let (event, context) = match host {
        Host::Hermes | Host::OpenClaw => (
            object_field(payload, "event")?,
            object_field(payload, "context")?,
        ),
        _ => {
            require_object(payload)?;
            if let Some(observed) = optional_string(payload, "hook_event_name")? {
                if observed != native_event {
                    return Err(Error::IdentityMismatch("hook_event_name"));
                }
            }
            (payload, payload)
        }
    };
    let (name_key, input_key) = match host {
        Host::OpenClaw => ("toolName", "params"),
        Host::Hermes => ("tool_name", "args"),
        _ => ("tool_name", "tool_input"),
    };
    let tool_name = required_string(event, name_key)?;
    let (session_id, tool_use_id) = match host {
        Host::OpenClaw => (
            consistent_id(event, context, "sessionId")?,
            consistent_id(event, context, "toolCallId")?,
        ),
        Host::Hermes => (
            optional_string(context, "session_id")?,
            optional_string(context, "tool_call_id")?,
        ),
        _ => (
            optional_string(event, "session_id")?,
            optional_string(event, "tool_use_id")?,
        ),
    };
    let turn_id = match host {
        Host::Codex => optional_string(event, "turn_id")?,
        Host::OpenClaw => consistent_id(event, context, "runId")?,
        _ => None,
    };
    let text = if pre {
        if !is_shell(host, &tool_name) {
            return Err(Error::UnsupportedPayload("unsupported shell tool"));
        }
        let input = event
            .get(input_key)
            .ok_or(Error::UnsupportedPayload(input_key))?;
        // Only Qoder and Qwen Code's existing hooks accept JSON-encoded input.
        let decoded;
        let input = if matches!(host, Host::Qoder | Host::QwenCode) && input.is_string() {
            decoded = serde_json::from_str::<Value>(
                input.as_str().ok_or(Error::UnsupportedPayload(input_key))?,
            )
            .map_err(|_| Error::UnsupportedPayload("invalid encoded tool_input"))?;
            &decoded
        } else {
            input
        };
        require_object(input)?;
        required_string(input, "command")?
    } else {
        // These SDKs supply errors separately from result. A string alone is
        // insufficient to preserve that failed-call state in this initial API.
        if matches!(host, Host::OpenClaw) {
            reject_error(event)?;
        } else if matches!(host, Host::Hermes) {
            reject_error(context)?;
        } else {
            reject_json_hook_failure(event)?;
        }
        let key = match host {
            Host::Hermes | Host::OpenClaw => "result",
            _ => "tool_response",
        };
        if matches!(host, Host::Cosh) {
            text_field(object_field(event, key)?, "llmContent")?
        } else if host == Host::Qoder && event[key].is_object() {
            let response = &event[key];
            // Only the observed successful Bash text slot is supported. Do not
            // turn failed, image, or mixed stdout/stderr results into a clean scan.
            if tool_name != "Bash"
                || response["kind"] != "completed"
                || response["exitCode"] != 0
                || response.get("signal") != Some(&Value::Null)
                || response["interrupted"] != false
                || response["isImage"] != false
                || response["noOutputExpected"] != false
                || response["stderr"] != ""
            {
                return Err(Error::UnsupportedPayload("unsupported Qoder Bash result"));
            }
            text_field(response, "stdout")?
        } else {
            text_field(event, key)?
        }
    };
    let origin = if !pre && is_shell(host, &tool_name) {
        "command_output"
    } else {
        "unspecified"
    };
    Ok(ExtractedCall {
        tool_name,
        tool_use_id,
        session_id,
        turn_id,
        text,
        origin: origin.into(),
    })
}

fn is_shell(host: Host, tool_name: &str) -> bool {
    match host {
        Host::Qoder | Host::Codex => tool_name == "Bash",
        Host::QwenCode => tool_name == "run_shell_command",
        Host::Cosh => matches!(tool_name, "run_shell_command" | "shell"),
        Host::Hermes => tool_name == "terminal",
        Host::OpenClaw => tool_name == "exec",
    }
}

fn require_object(value: &Value) -> Result<(), Error> {
    if !value.is_object() {
        return Err(Error::UnsupportedPayload("expected an object"));
    }
    Ok(())
}

fn object_field<'a>(value: &'a Value, key: &'static str) -> Result<&'a Value, Error> {
    let field = value.get(key).ok_or(Error::UnsupportedPayload(key))?;
    require_object(field)?;
    Ok(field)
}

fn optional_string(value: &Value, key: &'static str) -> Result<Option<String>, Error> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.clone())),
        _ => Err(Error::UnsupportedPayload(key)),
    }
}

fn required_string(value: &Value, key: &'static str) -> Result<String, Error> {
    optional_string(value, key)?.ok_or(Error::UnsupportedPayload(key))
}

fn text_field(value: &Value, key: &'static str) -> Result<String, Error> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(Error::UnsupportedPayload(key))
}

fn consistent_id(
    event: &Value,
    context: &Value,
    key: &'static str,
) -> Result<Option<String>, Error> {
    let event_id = optional_string(event, key)?;
    let context_id = optional_string(context, key)?;
    if let (Some(left), Some(right)) = (&event_id, &context_id) {
        if left != right {
            return Err(Error::IdentityMismatch(key));
        }
    }
    Ok(event_id.or(context_id))
}

fn reject_error(value: &Value) -> Result<(), Error> {
    match value.get("error") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(text)) if text.is_empty() => Ok(()),
        _ => Err(Error::UnsupportedPayload("failed tool result")),
    }
}

fn reject_json_hook_failure(value: &Value) -> Result<(), Error> {
    match value.get("is_error") {
        None | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => {
            return Err(Error::UnsupportedPayload("failed tool result"));
        }
        _ => return Err(Error::UnsupportedPayload("is_error")),
    }
    match value.get("status") {
        None => Ok(()),
        Some(Value::String(status)) => {
            // The existing common hook compares lowercase status without
            // trimming; these states cannot be preserved as successful text.
            if matches!(status.to_lowercase().as_str(), "interrupted" | "denied") {
                Err(Error::UnsupportedPayload("failed tool result"))
            } else {
                Ok(())
            }
        }
        _ => Err(Error::UnsupportedPayload("status")),
    }
}
