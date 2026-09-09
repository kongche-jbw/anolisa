//! Maps native scanner declarations without upgrading their coverage guarantees.

use aw_contracts::{canonical, Registry};
use serde_json::{json, Value};

pub(crate) fn translate(
    wire: &[u8],
    artifact: &Value,
    registry: &Registry,
) -> Result<Option<Value>, &'static str> {
    let native = canonical::parse(wire).map_err(|_| "invalid_native_json")?;
    if native["protocol_version"] != 1 || native["operation"] != "content_inspect" {
        return Err("native_protocol_mismatch");
    }
    let bytes = artifact["content"].as_str().ok_or("invalid_content")?.len() as u64;
    let scanned = native["scanned_bytes"]
        .as_u64()
        .ok_or("invalid_native_coverage")?;
    if scanned > bytes {
        return Err("invalid_native_coverage");
    }
    match native["disposition"].as_str() {
        Some("error") => {
            exact_keys(
                &native,
                &[
                    "protocol_version",
                    "operation",
                    "disposition",
                    "findings_total",
                    "scanned_bytes",
                    "error_code",
                ],
            )?;
            if native["findings_total"] != 0 || native["error_code"] != "scanner_failed" {
                return Err("invalid_native_error");
            }
            Err("scanner_failed")
        }
        Some("skipped") => {
            exact_keys(
                &native,
                &[
                    "protocol_version",
                    "operation",
                    "disposition",
                    "findings_total",
                    "scanned_bytes",
                    "skip_reason",
                ],
            )?;
            if native["findings_total"] != 0
                || scanned != 0
                || native["skip_reason"] != "not_applicable"
            {
                return Err("invalid_native_skip");
            }
            Ok(None)
        }
        Some("completed") => {
            exact_keys(
                &native,
                &[
                    "protocol_version",
                    "operation",
                    "disposition",
                    "findings_total",
                    "scanned_bytes",
                    "truncated",
                    "verdict",
                    "findings",
                    "engine",
                ],
            )?;
            if native["engine"] != "pii-regex" {
                return Err("unsupported_native_engine");
            }
            let truncated = native["truncated"]
                .as_bool()
                .ok_or("invalid_native_coverage")?;
            // AW v2 expresses completeness by bytes. Do not erase incompatible
            // native truncation claims, even when the reported byte count is full.
            if truncated == (scanned == bytes) {
                return Err("invalid_native_coverage");
            }
            let findings = native["findings"]
                .as_array()
                .ok_or("invalid_native_findings")?;
            let total = findings.iter().try_fold(0u64, |sum, finding| {
                sum.checked_add(finding["count"].as_u64().ok_or("invalid_native_findings")?)
                    .ok_or("invalid_native_findings")
            })?;
            if native["findings_total"].as_u64() != Some(total) {
                return Err("invalid_native_findings");
            }
            let output = json!({"inspection":{"verdict":native["verdict"],"findings":findings,
                "coverage":{"input_digest":artifact["digest"],"input_bytes":bytes,"scanned_bytes":scanned,"complete":!truncated,
                    "ruleset_ids":["sec-core/pii-regex/native-v1"],"languages":[]}}});
            registry
                .validate("security-content-inspect-output-v2", &output)
                .map_err(|_| "invalid_native_inspection")?;
            Ok(Some(output))
        }
        _ => Err("invalid_native_disposition"),
    }
}

fn exact_keys(value: &Value, keys: &[&str]) -> Result<(), &'static str> {
    let map = value.as_object().ok_or("invalid_native_shape")?;
    if map.len() != keys.len() || keys.iter().any(|key| !map.contains_key(*key)) {
        return Err("invalid_native_shape");
    }
    Ok(())
}
