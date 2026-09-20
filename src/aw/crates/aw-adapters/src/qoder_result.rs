//! Qoder's completed Bash stdout slot; native consumption requires separate evidence.

use crate::{native, Error, Host};
use serde_json::{json, Value};

/// Reuses native extraction while restricting interactive projection to Bash objects.
///
/// # Errors
/// Rejects failed, interrupted, mixed, image and unsupported tool results.
pub fn source(payload: &Value) -> Result<String, Error> {
    if payload["hook_event_name"] != "PostToolUse"
        || payload["tool_name"] != "Bash"
        || !payload["tool_response"].is_object()
    {
        return Err(Error::UnsupportedPayload("completed Bash result required"));
    }
    Ok(native::extract(Host::Qoder, "PostToolUse", payload)?.text)
}

/// Maps already validated text to the existing native replacement slot only.
pub fn replacement(text: &str) -> Value {
    json!({"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":text}})
}

/// Reports an unavailable optional projection without changing the native result.
pub fn unavailable() -> Value {
    json!({"systemMessage":"AW tool result projection unavailable; original result retained."})
}
