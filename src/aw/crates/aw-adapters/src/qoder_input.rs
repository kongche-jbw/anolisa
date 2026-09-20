//! Qoder 1.1.47 UserPromptSubmit responses; no prompt replacement or final guarantee.

use aw_contracts::input_response::InputResponse;
use serde_json::{json, Value};

/// Maps only the explicit public response authority into native Hook output.
pub fn response(response: &InputResponse) -> Value {
    match response {
        InputResponse::Continue(None) => json!({}),
        InputResponse::Continue(Some(context)) => json!({"hookSpecificOutput":{
            "hookEventName":"UserPromptSubmit","additionalContext":context}}),
        InputResponse::Reject(reason) => json!({"decision":"deny","reason":reason}),
    }
}

/// Rejects a failed response without disclosing command output or submitted text.
pub fn unavailable() -> Value {
    json!({"decision":"deny","reason":"AW input response denied or unavailable."})
}
