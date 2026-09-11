#![forbid(unsafe_code)]
//! Side-effect-free mapping of the native SecCore `scan-pii` JSON protocol.
//!
//! The embedding Host owns process execution, authentication, request/response
//! binding and receipts. This library neither scans text nor enforces policy.

mod protocol;

use aw_contracts::{canonical, Registry};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Reviewed native CLI protocol profile, independent of its implementation language.
pub const PROTOCOL_PROFILE: &str = "agent-sec.scan-pii/v1";
/// Maximum captured native stdout admitted before JSON deserialization.
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// Bounded errors deliberately omit native output and capability content.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// The AW input is malformed, mismatched or outside the post-tool profile.
    #[error("unsupported or invalid PII inspection input")]
    InvalidInput,
    /// Native execution failed; stdout cannot establish successful inspection.
    #[error("native PII execution failed")]
    NativeFailure,
    /// Output exceeds the fixed parsing budget.
    #[error("native PII output exceeds the byte limit")]
    OutputLimit,
    /// JSON shape or reported facts contradict the pinned protocol.
    #[error("invalid native PII response")]
    InvalidResponse,
    /// Scanning or custom-rule execution did not establish complete coverage.
    #[error("native PII coverage is incomplete")]
    IncompleteCoverage,
}

/// Validated input and exact stdin for one post-tool native PII invocation.
pub struct PiiRequest<'a> {
    registry: &'a Registry,
    text: String,
    digest: String,
    include_low: bool,
}

impl<'a> PiiRequest<'a> {
    /// Validates an existing content-inspection input and its exact text digest.
    ///
    /// # Errors
    /// Rejects invalid schema/domain, non-text content, pre-tool boundaries or a
    /// digest that does not match the bytes supplied to native stdin.
    pub fn new(registry: &'a Registry, input: &Value) -> Result<Self, Error> {
        registry
            .validate("security-content-inspect-input-v2", input)
            .map_err(|_| Error::InvalidInput)?;
        let artifact = &input["artifact"];
        if input["boundary"] != "post_tool" || artifact["media_type"] != "text/plain" {
            return Err(Error::InvalidInput);
        }
        let text = artifact["content"].as_str().ok_or(Error::InvalidInput)?;
        let digest = canonical::digest(text.as_bytes());
        if artifact["digest"] != digest {
            return Err(Error::InvalidInput);
        }
        Ok(Self {
            registry,
            text: text.to_owned(),
            digest,
            include_low: input["constraints"]["include_low_confidence"]
                .as_bool()
                .ok_or(Error::InvalidInput)?,
        })
    }

    /// Native arguments for `agent-sec-cli`, with no evidence or redaction flags.
    pub fn args(&self) -> Vec<&'static str> {
        let mut args = vec![
            "scan-pii",
            "--stdin",
            "--format",
            "json",
            "--source",
            "tool_output",
        ];
        if self.include_low {
            args.push("--include-low-confidence");
        }
        args
    }

    /// Exact UTF-8 bytes to deliver to native stdin without newline insertion.
    pub fn stdin(&self) -> &[u8] {
        self.text.as_bytes()
    }

    /// Projects a successful native response into a validated AW inspection.
    ///
    /// Native output supplies no input digest. Byte-count consistency cannot
    /// authenticate a response against same-length replacement input: the Host
    /// must bind stdout to this exact invocation. No receipt is manufactured.
    ///
    /// # Errors
    /// Rejects failed execution, oversized/ambiguous output, inconsistent summary,
    /// unknown finding semantics and incomplete scanning or custom-rule execution.
    pub fn project(&self, exit_code: i32, stdout: &[u8]) -> Result<Value, Error> {
        if exit_code != 0 {
            return Err(Error::NativeFailure);
        }
        if stdout.len() > MAX_OUTPUT_BYTES {
            return Err(Error::OutputLimit);
        }
        let native: protocol::Response =
            serde_json::from_slice(stdout).map_err(|_| Error::InvalidResponse)?;
        if !native.ok || native.verdict == "error" {
            return Err(Error::NativeFailure);
        }
        let summary = &native.summary;
        if summary.source != "tool_output" || summary.total != native.findings.len() as u64 {
            return Err(Error::InvalidResponse);
        }
        if summary.truncated || summary.bytes_scanned != self.text.len() as u64 {
            return Err(Error::IncompleteCoverage);
        }
        let rulesets = summary.custom_rules.rulesets()?;
        let mut types = BTreeMap::new();
        let mut categories = BTreeMap::new();
        let mut severities = BTreeMap::new();
        let mut groups = BTreeMap::<(String, &str, &str, &str), u32>::new();
        let text_chars = self.text.chars().count() as u64;
        let mut denied = false;
        for finding in &native.findings {
            finding.validate(text_chars, self.include_low)?;
            let category = match finding.category.as_str() {
                "personal_data" => "personal_data",
                "credential" => "credential",
                "custom"
                    if summary.custom_rules.status == "loaded"
                        && summary.custom_rules.rule_count > 0 =>
                {
                    "other"
                }
                _ => return Err(Error::InvalidResponse),
            };
            let severity = match finding.severity.as_str() {
                "warn" => "medium",
                "deny" => {
                    denied = true;
                    "high"
                }
                _ => return Err(Error::InvalidResponse),
            };
            let confidence = if finding.confidence < 0.5 {
                "low"
            } else if finding.confidence < 0.8 {
                "medium"
            } else {
                "high"
            };
            *types.entry(finding.kind.clone()).or_insert(0_u64) += 1;
            *categories.entry(finding.category.clone()).or_insert(0_u64) += 1;
            *severities.entry(finding.severity.clone()).or_insert(0_u64) += 1;
            let count = groups
                .entry((finding.kind.clone(), category, severity, confidence))
                .or_default();
            *count = count.checked_add(1).ok_or(Error::InvalidResponse)?;
        }
        if types != summary.by_type
            || categories != summary.by_category
            || severities != summary.by_severity
            || groups.len() > 64
        {
            return Err(Error::InvalidResponse);
        }
        let (native_verdict, verdict) = if denied {
            ("deny", "sensitive")
        } else if !groups.is_empty() {
            ("warn", "suspicious")
        } else {
            ("pass", "clean")
        };
        if native.verdict != native_verdict {
            return Err(Error::InvalidResponse);
        }
        let findings: Vec<_> = groups.into_iter().map(|((rule_id, category, severity, confidence), count)|
            json!({"rule_id":rule_id,"category":category,"severity":severity,"confidence":confidence,"count":count})
        ).collect();
        let output = json!({"inspection": {"verdict":verdict,"findings":findings,"coverage":{
            "input_digest":self.digest,"input_bytes":self.text.len(),"scanned_bytes":summary.bytes_scanned,
            "complete":true,"ruleset_ids":rulesets,"languages":[]
        }}});
        self.registry
            .validate("security-content-inspect-output-v2", &output)
            .map_err(|_| Error::InvalidResponse)?;
        Ok(output)
    }
}
