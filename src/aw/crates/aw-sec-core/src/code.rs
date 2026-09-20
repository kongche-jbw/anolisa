//! Strict mapping of the native Bash scan-code protocol; no command execution.

use serde::Deserialize;

/// Maximum Bash command accepted by the argv-based native interface.
pub const MAX_COMMAND_BYTES: usize = 64 * 1024;

/// Invalid or incomplete native checks never imply permission.
#[derive(Debug, thiserror::Error)]
#[error("invalid or unavailable native code scan")]
pub struct Error;

/// Exact Bash command passed as one argv entry, never interpreted by the Host.
pub struct CodeRequest<'a> {
    command: &'a str,
}
impl<'a> CodeRequest<'a> {
    /// Validates the bounded native Bash input.
    pub fn new(command: &'a str) -> Result<Self, Error> {
        if command.trim().is_empty() || command.len() > MAX_COMMAND_BYTES || command.contains('\0')
        {
            return Err(Error);
        }
        Ok(Self { command })
    }
    /// Native regex scanner arguments; the command is a literal argv value.
    pub fn args(&self) -> [&str; 7] {
        [
            "scan-code",
            "--code",
            self.command,
            "--language",
            "bash",
            "--mode",
            "regex",
        ]
    }
    /// Allows only a successful, internally consistent pass without findings.
    ///
    /// Warn and deny both reject under this explicitly selected strict policy.
    /// The native protocol has no input digest or coverage attestation; the Host
    /// binds the process exchange, without claiming exhaustive threat detection.
    pub fn allows(&self, exit_code: i32, stdout: &[u8]) -> Result<bool, Error> {
        if exit_code != 0 || stdout.len() > crate::MAX_OUTPUT_BYTES {
            return Err(Error);
        }
        let result: Response = serde_json::from_slice(stdout).map_err(|_| Error)?;
        if !result.ok
            || result.language != "bash"
            || result.engine_version.trim().is_empty()
            || result.findings.iter().any(|f| {
                f.rule_id.trim().is_empty() || !matches!(f.severity.as_str(), "warn" | "deny")
            })
        {
            return Err(Error);
        }
        let expected = if result.findings.iter().any(|f| f.severity == "deny") {
            "deny"
        } else if result.findings.is_empty() {
            "pass"
        } else {
            "warn"
        };
        if result.verdict != expected {
            return Err(Error);
        }
        Ok(expected == "pass")
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    ok: bool,
    verdict: String,
    language: String,
    engine_version: String,
    findings: Vec<Finding>,
    #[serde(rename = "summary")]
    _summary: String,
    #[serde(rename = "elapsed_ms")]
    _elapsed_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Finding {
    rule_id: String,
    severity: String,
    #[serde(rename = "desc_zh")]
    _desc_zh: String,
    #[serde(rename = "desc_en")]
    _desc_en: String,
    #[serde(rename = "evidence")]
    _evidence: Vec<String>,
}
