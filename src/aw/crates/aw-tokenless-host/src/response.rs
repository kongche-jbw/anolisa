//! Translates native declarations without inventing a byte-exact recovery path.

use aw_contracts::{canonical, Registry};
use serde_json::{json, Value};
use tokenless_protocol::{Attribution, Disposition, Recoverability, Response, ResponseEnvelope};

pub(crate) fn translate(
    wire: &[u8],
    artifact: &Value,
    attribution: &Attribution,
    registry: &Registry,
) -> Result<Option<Value>, &'static str> {
    // Check duplicate keys before the native crate parses its own strict shape.
    canonical::parse(wire).map_err(|_| "invalid_native_json")?;
    let envelope =
        ResponseEnvelope::from_json(std::str::from_utf8(wire).map_err(|_| "invalid_native_json")?)
            .map_err(|_| "invalid_native_response")?;
    if envelope.attribution != *attribution {
        return Err("native_attribution_mismatch");
    }
    let Response::PostTool(native) = envelope.response else {
        return Err("native_operation_mismatch");
    };
    match native.disposition {
        Disposition::DryRun
        | Disposition::Passthrough
        | Disposition::NoSavings
        | Disposition::RecoverabilityUnavailable => Ok(None),
        Disposition::Timeout => Err("native_timeout"),
        Disposition::ToolError => Err("native_tool_error"),
        Disposition::Applied => {
            // Native lossless retains task-relevant information, not source bytes.
            // The caller must opt in to AW's weaker, unrecoverable guarantee.
            if native.recoverability == Recoverability::Retrievable || !native.stash_keys.is_empty()
            {
                return Err("unsupported_native_recovery");
            }
            let original = artifact["content"].as_str().ok_or("invalid_content")?;
            if native.output.len() >= original.len() {
                return Ok(None);
            }
            let chain: Vec<_> = native
                .applied_operations
                .iter()
                .map(|operation| operation.wire_str())
                .collect();
            let output = json!({"candidate":{
                "source_artifact_id":artifact["id"],"source_digest":artifact["digest"],
                "content":native.output,"media_type":"text/plain","transform_chain":chain,
                "reversibility":"unrecoverable","recovery":{"mode":"none"}
            }});
            registry
                .validate("context-projection-prepare-output-v2", &output)
                .map_err(|_| "invalid_candidate")?;
            Ok(Some(output))
        }
    }
}
