//! Qoder lifecycle notification mapping; control powers remain unimplemented here.

use crate::Error;
use aw_contracts::events::EventName;
use serde_json::Value;

/// Native registrations for eleven public lifecycle names (two tool outcomes).
pub const NATIVE_EVENTS: [&str; 12] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "SessionEnd",
];

/// Maps documented hook names only; OS and model boundaries cannot be self-reported.
///
/// # Errors
/// Rejects native names without a lifecycle notification mapping.
pub fn event_name(native: &str) -> Result<EventName, Error> {
    use EventName::*;
    Ok(match native {
        "SessionStart" => SessionStart,
        "UserPromptSubmit" => InputSubmit,
        "PreToolUse" => ToolBefore,
        "PostToolUse" | "PostToolUseFailure" => ToolAfter,
        "PermissionRequest" => PermissionRequest,
        "PreCompact" => CompactBefore,
        "PostCompact" => CompactAfter,
        "SubagentStart" => SubagentStart,
        "SubagentStop" => SubagentStop,
        "Stop" => TurnStop,
        "SessionEnd" => SessionEnd,
        _ => return Err(Error::UnsupportedEvent),
    })
}

/// Validates fields needed to interpret a notification, preserving the raw payload.
///
/// Caller authenticates ancestry, cwd and session. Child lifecycle events retain
/// their native Agent ID; other child callbacks require a separately bound profile.
///
/// # Errors
/// Rejects missing semantic fields, unsupported sources and unbound child callbacks.
pub fn validate(payload: &Value) -> Result<EventName, Error> {
    let event = event_name(identifier(payload, "hook_event_name")?)?;
    identifier(payload, "session_id")?;
    identifier(payload, "cwd")?;
    let child = matches!(event, EventName::SubagentStart | EventName::SubagentStop);
    if !child && payload.get("agent_id").is_some_and(|id| !id.is_null()) {
        return Err(Error::UnsupportedPayload(
            "child callback requires its own profile",
        ));
    }
    match event {
        EventName::SessionStart => {
            identifier(payload, "source")?;
        }
        EventName::SessionEnd => {
            identifier(payload, "reason")?;
        }
        EventName::InputSubmit => {
            text(payload, "prompt")?;
        }
        EventName::ToolBefore | EventName::PermissionRequest => {
            identifier(payload, "tool_name")?;
            if !payload
                .get("tool_input")
                .is_some_and(|v| v.is_object() || v.is_string())
            {
                return Err(Error::UnsupportedPayload("missing tool input"));
            }
        }
        EventName::ToolAfter => {
            identifier(payload, "tool_name")?;
            if payload["hook_event_name"] == "PostToolUseFailure" {
                text(payload, "error")?;
            } else if payload.get("tool_response").is_none() {
                return Err(Error::UnsupportedPayload("missing tool result"));
            }
        }
        EventName::CompactBefore | EventName::CompactAfter => {
            identifier(payload, "trigger")?;
            if event == EventName::CompactAfter {
                text(payload, "compact_summary")?;
            }
        }
        EventName::SubagentStart | EventName::SubagentStop => {
            identifier(payload, "agent_id")?;
            identifier(payload, "agent_type")?;
        }
        EventName::TurnStop => {
            if !payload["stop_hook_active"].is_boolean() {
                return Err(Error::UnsupportedPayload("missing stop check state"));
            }
        }
        _ => return Err(Error::UnsupportedEvent),
    }
    if matches!(event, EventName::ToolBefore | EventName::ToolAfter) {
        identifier(payload, "tool_use_id")?;
    }
    if let Some(id) = payload.get("tool_use_id") {
        if !id.is_null() {
            identifier(payload, "tool_use_id")?;
        }
    }
    Ok(event)
}

fn text<'a>(payload: &'a Value, key: &str) -> Result<&'a str, Error> {
    payload[key]
        .as_str()
        .ok_or(Error::UnsupportedPayload("missing text field"))
}

fn identifier<'a>(payload: &'a Value, key: &str) -> Result<&'a str, Error> {
    let value = text(payload, key)?;
    if value.is_empty() || value.len() > 4096 {
        return Err(Error::UnsupportedPayload("invalid native identity"));
    }
    Ok(value)
}
