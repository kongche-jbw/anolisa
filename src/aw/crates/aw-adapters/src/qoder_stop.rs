//! Qoder Stop output mapping; native adoption requires separate versioned evidence.

use aw_contracts::stop_response::StopResponse;
use serde_json::{json, Value};

/// Maps stop acceptance or a request to continue; it grants no tool permission.
pub fn response(response: &StopResponse) -> Value {
    match response {
        StopResponse::AllowStop => json!({}),
        StopResponse::Continue(reason) => json!({"decision":"deny","reason":reason}),
    }
}

/// Requests stopping on failure or repeated checking without reporting check success.
///
/// Qoder print mode can omit these diagnostics and still exit successfully.
/// Exit 2 would request more model work for Stop, so it is not an error fallback.
pub fn unavailable() -> Value {
    json!({"continue":false,
        "stopReason":"AW stop check unavailable or continuation limit reached.",
        "systemMessage":"AW stop check unavailable or continuation limit reached; check not passed."})
}
