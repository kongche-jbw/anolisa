//! Experimental Qoder PreToolUse candidate and native response mapping.

use crate::{Error, Host};
use serde_json::{json, Value};

/// Captures a Bash tool candidate without inventing permission to execute it.
///
/// # Errors
/// Rejects invalid native payloads, missing cwd and unsupported tools.
pub fn candidate(payload: &Value) -> Result<Value, Error> {
    let call = crate::native::extract(Host::Qoder, "PreToolUse", payload)?;
    let cwd = payload["cwd"]
        .as_str()
        .filter(|s| s.starts_with('/'))
        .ok_or(Error::UnsupportedPayload("cwd"))?;
    let input = if let Some(text) = payload["tool_input"].as_str() {
        aw_contracts::canonical::parse(text.as_bytes())
            .map_err(|_| Error::UnsupportedPayload("tool_input"))?
    } else {
        payload["tool_input"].clone()
    };
    let value = json!({"tool_name":call.tool_name,"tool_input":input,"cwd":cwd});
    validate(&value)?;
    Ok(value)
}

/// Restricts this profile to an immutable Bash context and structured arguments.
pub fn validate(candidate: &Value) -> Result<(), Error> {
    if candidate.as_object().map(|o| o.len()) != Some(3)
        || candidate["tool_name"] != "Bash"
        || !candidate["cwd"]
            .as_str()
            .is_some_and(|s| s.starts_with('/') && !s.contains('\0'))
        || !candidate["tool_input"].is_object()
        || !candidate["tool_input"]["command"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty() && !s.contains('\0') && s.len() <= 65536)
    {
        return Err(Error::UnsupportedPayload("Bash tool candidate"));
    }
    Ok(())
}

/// Returns checked arguments while preserving Qoder's normal permission checks.
///
/// No `permissionDecision=allow` is emitted: a clean scan must not bypass approval.
/// Returning arguments is not evidence that Qoder consumed them unchanged.
pub fn checked(candidate: &Value) -> Result<Value, Error> {
    validate(candidate)?;
    Ok(json!({"hookSpecificOutput":{"hookEventName":"PreToolUse",
        "updatedInput":candidate["tool_input"]}}))
}

/// Native denial with a fixed reason that never exposes command or scan evidence.
pub fn denied() -> Value {
    json!({"hookSpecificOutput":{"hookEventName":"PreToolUse",
        "permissionDecision":"deny",
        "permissionDecisionReason":"AW tool safety check denied or unavailable."}})
}
