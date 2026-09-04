//! Provider-independent contracts for security inspection and Tool Call gates.
//!
//! These Capabilities report facts about content that already exists, or return
//! a verdict for a Tool Call that has not run yet. None of them carries matched
//! content: every textual field is a closed enum or a [`SecurityRuleId`], so a
//! finding cannot become a channel for the secret it found.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::{
    common::{BoundedName, BoundedStringError, Digest, DigestError},
    ids::ArtifactId,
    provider::{SchemaReference, VersionedSchema},
};

/// Stable identity of the content-inspection Capability.
pub const SECURITY_CONTENT_INSPECT_CAPABILITY_ID: &str = "security.content.inspect";
/// Current revision of the content-inspection Capability.
pub const SECURITY_CONTENT_INSPECT_CAPABILITY_VERSION: u16 = 1;
/// Stable identity of the canonical content-inspection input schema.
pub const SECURITY_CONTENT_INSPECT_INPUT_SCHEMA_ID: &str = "security.content.inspect.input";
/// Current revision of the canonical content-inspection input schema.
pub const SECURITY_CONTENT_INSPECT_INPUT_SCHEMA_VERSION: u16 = 1;
/// SHA-256 of the current canonical content-inspection input schema resource.
pub const SECURITY_CONTENT_INSPECT_INPUT_SCHEMA_SHA256: &str =
    "836231087cc27186746e4316e92dd842053b71ebc1e9f392d99f521adbdfcec4";
/// Stable identity of the canonical content-inspection output schema.
pub const SECURITY_CONTENT_INSPECT_OUTPUT_SCHEMA_ID: &str = "security.content.inspect.output";
/// Current revision of the canonical content-inspection output schema.
pub const SECURITY_CONTENT_INSPECT_OUTPUT_SCHEMA_VERSION: u16 = 1;
/// SHA-256 of the current canonical content-inspection output schema resource.
pub const SECURITY_CONTENT_INSPECT_OUTPUT_SCHEMA_SHA256: &str =
    "4a7a0771081fba792a889d8a1182b7ade08e4a7864c93dd5badb46f259e83bd5";

/// Stable identity of the code-inspection Capability.
pub const SECURITY_CODE_INSPECT_CAPABILITY_ID: &str = "security.code.inspect";
/// Current revision of the code-inspection Capability.
pub const SECURITY_CODE_INSPECT_CAPABILITY_VERSION: u16 = 1;
/// Stable identity of the canonical code-inspection input schema.
pub const SECURITY_CODE_INSPECT_INPUT_SCHEMA_ID: &str = "security.code.inspect.input";
/// Current revision of the canonical code-inspection input schema.
pub const SECURITY_CODE_INSPECT_INPUT_SCHEMA_VERSION: u16 = 1;
/// SHA-256 of the current canonical code-inspection input schema resource.
pub const SECURITY_CODE_INSPECT_INPUT_SCHEMA_SHA256: &str =
    "c5614f6e464a401621a7d8d1e1c8245edc504919ffde6c7c20d01799bd961c1e";
/// Stable identity of the canonical code-inspection output schema.
pub const SECURITY_CODE_INSPECT_OUTPUT_SCHEMA_ID: &str = "security.code.inspect.output";
/// Current revision of the canonical code-inspection output schema.
pub const SECURITY_CODE_INSPECT_OUTPUT_SCHEMA_VERSION: u16 = 1;
/// SHA-256 of the current canonical code-inspection output schema resource.
pub const SECURITY_CODE_INSPECT_OUTPUT_SCHEMA_SHA256: &str =
    "d41ea55912472845fab80695e5bc78a2cb84996bccaf49264b8b4c99064308f7";

/// Stable identity of the command-inspection Capability.
pub const SECURITY_COMMAND_INSPECT_CAPABILITY_ID: &str = "security.command.inspect";
/// Current revision of the command-inspection Capability.
pub const SECURITY_COMMAND_INSPECT_CAPABILITY_VERSION: u16 = 1;
/// Stable identity of the canonical command-inspection input schema.
pub const SECURITY_COMMAND_INSPECT_INPUT_SCHEMA_ID: &str = "security.command.inspect.input";
/// Current revision of the canonical command-inspection input schema.
pub const SECURITY_COMMAND_INSPECT_INPUT_SCHEMA_VERSION: u16 = 1;
/// SHA-256 of the current canonical command-inspection input schema resource.
pub const SECURITY_COMMAND_INSPECT_INPUT_SCHEMA_SHA256: &str =
    "546e5fc3b98ecc800160a70e0ad8a3e02be6cee02dbe8fa6ff6987be8eb511de";
/// Stable identity of the canonical command-inspection output schema.
pub const SECURITY_COMMAND_INSPECT_OUTPUT_SCHEMA_ID: &str = "security.command.inspect.output";
/// Current revision of the canonical command-inspection output schema.
pub const SECURITY_COMMAND_INSPECT_OUTPUT_SCHEMA_VERSION: u16 = 1;
/// SHA-256 of the current canonical command-inspection output schema resource.
pub const SECURITY_COMMAND_INSPECT_OUTPUT_SCHEMA_SHA256: &str =
    "3618b57a48d32f6d7f1dcbb3ac18594fa48196deaee60131649014345ea9c57b";

/// Maximum UTF-8 byte length of a security rule identity.
pub const MAX_SECURITY_RULE_ID_BYTES: usize = 64;
/// Maximum number of findings Core accepts from one inspection.
pub const MAX_OBSERVATION_FINDINGS: usize = 64;
/// Maximum number of rationale codes Core accepts from one gate verdict.
pub const MAX_GATE_REASONS: usize = 32;

/// Failure returned when a security rule identity is not a stable label.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecurityRuleIdError {
    /// A rule identity must name a concrete rule.
    #[error("security rule id must not be empty")]
    Empty,
    /// Rule identities are capped to keep Ledger records predictable.
    #[error("security rule id exceeds the {MAX_SECURITY_RULE_ID_BYTES}-byte limit")]
    TooLong,
    /// The character set is deliberately narrow; see [`SecurityRuleId`].
    #[error("security rule id must use lowercase ASCII letters, digits, '.', '_', and '-'")]
    InvalidCharacter,
}

/// Stable identity of one security rule that produced a finding.
///
/// The accepted character set is narrower than [`BoundedName`] on purpose. A
/// rule label is the only free-form field an inspection result carries, so
/// restricting it to `[a-z0-9._-]` keeps a Provider from smuggling matched
/// content — an API key, a password, a personal identifier — out through it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SecurityRuleId(String);

impl SecurityRuleId {
    /// Parses a stable lowercase rule identity.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is empty, oversized, or contains any
    /// character outside `[a-z0-9._-]`.
    pub fn parse(value: impl Into<String>) -> Result<Self, SecurityRuleIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SecurityRuleIdError::Empty);
        }
        if value.len() > MAX_SECURITY_RULE_ID_BYTES {
            return Err(SecurityRuleIdError::TooLong);
        }
        if !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        }) {
            return Err(SecurityRuleIdError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the stable rule identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for SecurityRuleId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SecurityRuleId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Environment boundary at which an inspection Capability is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityBoundary {
    /// Before a Tool Call executes, while a gate can still change the outcome.
    PreTool,
    /// After a Tool Call produced a result.
    PostTool,
}

/// Source language a code inspection should assume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityCodeLanguage {
    /// Let the implementation choose from the content.
    Auto,
    /// POSIX or Bash shell.
    Bash,
    /// Python.
    Python,
}

/// Language a code inspection reported having analysed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityDetectedLanguage {
    /// POSIX or Bash shell.
    Bash,
    /// Python.
    Python,
    /// Both shell and Python rules were applied to the same content.
    Mixed,
    /// The implementation could not classify the content.
    Unknown,
}

/// Broad class of a security finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityFindingCategory {
    /// Long-lived secret material such as an API key or private key.
    Secret,
    /// Personal data such as an identity number or contact detail.
    PersonalData,
    /// Interactive credential such as a password or token.
    Credential,
    /// Construct whose execution is intrinsically risky.
    DangerousPattern,
    /// Construct that appears intended to hide its behaviour.
    Obfuscation,
    /// A class the Capability does not model more precisely.
    Other,
}

/// Severity an implementation attached to a finding class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityFindingSeverity {
    /// Recorded for completeness; no action implied.
    Info,
    /// Minor concern.
    Low,
    /// Concern that deserves review.
    Medium,
    /// Serious concern.
    High,
    /// Severe concern.
    Critical,
}

/// How confident an implementation is that a finding is real.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityFindingConfidence {
    /// Likely to include false positives.
    Low,
    /// Balanced precision and recall.
    Medium,
    /// Precise match.
    High,
}

/// Overall conclusion of one inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityInspectionVerdict {
    /// Nothing was found.
    Clean,
    /// Something was found that warrants attention but is not conclusive.
    Suspicious,
    /// Content that must be treated as sensitive was found.
    Sensitive,
}

/// Gate verdict returned for a pending Tool Call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandVerdict {
    /// The Capability found no reason to stop the Tool Call.
    Allow,
    /// The Capability found a concern that does not justify refusing.
    Warn,
    /// The Capability judges the Tool Call unsafe to run.
    Deny,
}

/// Content-free count of one finding class.
///
/// A finding never carries the matched value, its offset, or its surrounding
/// text. It reports which rule fired, how it is classified, and how often.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityFinding {
    /// Rule that produced the matches.
    pub rule_id: SecurityRuleId,
    /// Broad class of the finding.
    pub category: SecurityFindingCategory,
    /// Severity the implementation attached.
    pub severity: SecurityFindingSeverity,
    /// Confidence the implementation attached.
    pub confidence: SecurityFindingConfidence,
    /// Number of matches attributed to this rule.
    pub count: u32,
}

/// Content-free result of `security.content.inspect/v1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentInspection {
    /// Overall conclusion.
    pub verdict: SecurityInspectionVerdict,
    /// Per-rule counts.
    pub findings: Vec<SecurityFinding>,
    /// Bytes the implementation reported inspecting.
    pub scanned_bytes: u64,
    /// Whether the implementation stopped before the whole artifact.
    pub truncated: bool,
}

impl ContentInspection {
    /// Verifies the cross-field invariants of a produced inspection fact.
    ///
    /// # Errors
    ///
    /// Returns an error when a clean verdict is incomplete or carries
    /// findings, a finding verdict carries no findings, a finding count is
    /// zero, or the result exceeds the canonical finding bound.
    pub fn validate(&self) -> Result<(), SecurityOutputValidationError> {
        validate_observation(
            self.verdict,
            &self.findings,
            self.truncated,
            SecurityObservationKind::Content,
        )
    }
}

/// Content-free result of `security.code.inspect/v1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeInspection {
    /// Overall conclusion.
    pub verdict: SecurityInspectionVerdict,
    /// Per-rule counts.
    pub findings: Vec<SecurityFinding>,
    /// Bytes the implementation reported inspecting.
    pub scanned_bytes: u64,
    /// Whether the implementation stopped before the whole artifact.
    pub truncated: bool,
    /// Language or language set whose rules the implementation applied.
    pub language_detected: SecurityDetectedLanguage,
}

impl CodeInspection {
    /// Verifies the cross-field invariants of a produced inspection fact.
    ///
    /// # Errors
    ///
    /// Returns an error when a clean verdict is incomplete or carries
    /// findings, a finding verdict carries no findings, a finding count is
    /// zero, or the result exceeds the canonical finding bound.
    pub fn validate(&self) -> Result<(), SecurityOutputValidationError> {
        validate_observation(
            self.verdict,
            &self.findings,
            self.truncated,
            SecurityObservationKind::Code,
        )
    }
}

/// Gate verdict and rationale returned by `security.command.inspect/v1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandInspection {
    /// Verdict Core turns into a Tool Call gate.
    pub verdict: CommandVerdict,
    /// Rationale codes safe for operator presentation.
    pub reasons: Vec<SecurityRuleId>,
    /// Per-rule counts.
    pub findings: Vec<SecurityFinding>,
    /// Bytes the implementation reported inspecting.
    pub scanned_bytes: u64,
}

impl CommandInspection {
    /// Verifies the cross-field invariants of a produced Tool Call verdict.
    ///
    /// # Errors
    ///
    /// Returns an error when an allow verdict carries rationale, a warn or
    /// deny verdict lacks rationale, a finding count is zero, or either list
    /// exceeds its canonical bound.
    pub fn validate(&self) -> Result<(), SecurityOutputValidationError> {
        validate_findings(&self.findings)?;
        if self.reasons.len() > MAX_GATE_REASONS {
            return Err(SecurityOutputValidationError::TooManyReasons {
                actual: self.reasons.len(),
            });
        }

        match self.verdict {
            CommandVerdict::Allow => {
                if !self.reasons.is_empty() {
                    return Err(SecurityOutputValidationError::AllowHasReasons);
                }
                if !self.findings.is_empty() {
                    return Err(SecurityOutputValidationError::AllowHasFindings);
                }
            }
            CommandVerdict::Warn | CommandVerdict::Deny => {
                if self.reasons.is_empty() {
                    return Err(SecurityOutputValidationError::GateVerdictWithoutReasons {
                        verdict: self.verdict,
                    });
                }
                if self.findings.is_empty() {
                    return Err(SecurityOutputValidationError::GateVerdictWithoutFindings {
                        verdict: self.verdict,
                    });
                }
            }
        }

        Ok(())
    }
}

/// Failure returned when a decoded security output contradicts its verdict.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecurityOutputValidationError {
    /// An inspection exceeded the bounded canonical finding list.
    #[error("security output has {actual} findings; maximum is {MAX_OBSERVATION_FINDINGS}")]
    TooManyFindings {
        /// Number of findings supplied by the Provider.
        actual: usize,
    },
    /// A finding claimed to represent no matches.
    #[error("security finding at index {index} has a zero match count")]
    ZeroFindingCount {
        /// Zero-based index of the invalid finding.
        index: usize,
    },
    /// A clean observation carried one or more findings.
    #[error("clean {kind} inspection must not carry findings")]
    CleanHasFindings {
        /// Capability family whose output was invalid.
        kind: SecurityObservationKind,
    },
    /// A clean observation did not cover the complete submitted artifact.
    #[error("clean {kind} inspection must not be truncated")]
    CleanWasTruncated {
        /// Capability family whose output was invalid.
        kind: SecurityObservationKind,
    },
    /// A suspicious or sensitive observation had no supporting finding.
    #[error("{verdict:?} {kind} inspection must carry at least one finding")]
    ObservationVerdictWithoutFindings {
        /// Capability family whose output was invalid.
        kind: SecurityObservationKind,
        /// Unsupported verdict claim.
        verdict: SecurityInspectionVerdict,
    },
    /// A gate result exceeded the bounded canonical reason list.
    #[error("command inspection has {actual} reasons; maximum is {MAX_GATE_REASONS}")]
    TooManyReasons {
        /// Number of reasons supplied by the Provider.
        actual: usize,
    },
    /// An allow verdict carried one or more rationale codes.
    #[error("allow command verdict must not carry reasons")]
    AllowHasReasons,
    /// An allow verdict carried one or more findings.
    #[error("allow command verdict must not carry findings")]
    AllowHasFindings,
    /// A warn or deny verdict had no rationale code.
    #[error("{verdict:?} command verdict must carry at least one reason")]
    GateVerdictWithoutReasons {
        /// Unsupported verdict claim.
        verdict: CommandVerdict,
    },
    /// A warn or deny verdict had no supporting finding.
    #[error("{verdict:?} command verdict must carry at least one finding")]
    GateVerdictWithoutFindings {
        /// Unsupported verdict claim.
        verdict: CommandVerdict,
    },
}

/// Security observation family used to identify invariant failures safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityObservationKind {
    /// `security.content.inspect/v1` output.
    Content,
    /// `security.code.inspect/v1` output.
    Code,
}

impl std::fmt::Display for SecurityObservationKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Content => formatter.write_str("content"),
            Self::Code => formatter.write_str("code"),
        }
    }
}

fn validate_observation(
    verdict: SecurityInspectionVerdict,
    findings: &[SecurityFinding],
    truncated: bool,
    kind: SecurityObservationKind,
) -> Result<(), SecurityOutputValidationError> {
    validate_findings(findings)?;

    match verdict {
        SecurityInspectionVerdict::Clean => {
            if !findings.is_empty() {
                return Err(SecurityOutputValidationError::CleanHasFindings { kind });
            }
            if truncated {
                return Err(SecurityOutputValidationError::CleanWasTruncated { kind });
            }
        }
        SecurityInspectionVerdict::Suspicious | SecurityInspectionVerdict::Sensitive => {
            if findings.is_empty() {
                return Err(
                    SecurityOutputValidationError::ObservationVerdictWithoutFindings {
                        kind,
                        verdict,
                    },
                );
            }
        }
    }

    Ok(())
}

fn validate_findings(findings: &[SecurityFinding]) -> Result<(), SecurityOutputValidationError> {
    if findings.len() > MAX_OBSERVATION_FINDINGS {
        return Err(SecurityOutputValidationError::TooManyFindings {
            actual: findings.len(),
        });
    }
    if let Some(index) = findings.iter().position(|finding| finding.count == 0) {
        return Err(SecurityOutputValidationError::ZeroFindingCount { index });
    }
    Ok(())
}

/// Pending Tool Call an Agent Environment offers to a Mediate Capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingToolCallSubmission {
    /// Command text the Agent proposes to execute.
    pub command: String,
    /// Language the Environment believes the command is written in.
    pub language: SecurityCodeLanguage,
    /// Tool name when the Environment can provide one safely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<BoundedName>,
}

/// Why an Observe Capability produced no usable fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationGapReason {
    /// No admitted implementation satisfies the Capability Contract.
    NoImplementation,
    /// A matching implementation only declares its isolation controls.
    ControlsNotEnforced,
    /// The invocation settled without producing a result.
    NotProduced,
    /// The implementation returned a result Core could not accept.
    InvalidOutput,
    /// The Provider Host could not complete the invocation.
    HostFailure,
    /// The fact could not be recorded durably, so it is not claimed.
    LedgerUnavailable,
}

/// Why a Tool Call gate resolved without an implementation verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDegradation {
    /// No admitted implementation satisfies the Capability Contract.
    NoImplementation,
    /// Several implementations qualify and routing policy named none.
    AmbiguousRoute,
    /// A matching implementation only declares its isolation controls.
    ControlsNotEnforced,
    /// The invocation settled without producing a verdict.
    NotProduced,
    /// The implementation returned a verdict Core could not accept.
    InvalidOutput,
    /// The Provider Host could not complete the invocation.
    HostFailure,
    /// The decision could not be recorded durably.
    LedgerUnavailable,
}

/// Gate outcome Core requires an Agent Environment to honour.
///
/// [`ToolCallGate::NotMediated`] is not an approval. It states that no verdict
/// exists, so the Environment must apply its own default rather than read the
/// absence of an opinion as permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallGate {
    /// No governed verdict was produced for this Tool Call.
    NotMediated,
    /// The Tool Call may proceed.
    Allow,
    /// The Tool Call may proceed and the operator should be told why not to.
    Warn,
    /// A human must decide before the Tool Call proceeds.
    Ask,
    /// The Tool Call must not proceed.
    Block,
}

/// Failure returned while constructing a built-in security Contract reference.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecurityContractBuildError {
    /// A built-in schema name violates the bounded-name invariant.
    #[error(transparent)]
    Name(#[from] BoundedStringError),
    /// A built-in schema digest is not canonical SHA-256 text.
    #[error(transparent)]
    Digest(#[from] DigestError),
}

/// Immutable content offered to an inspection Capability.
///
/// This mirrors the context-projection artifact so one Environment event can
/// submit the same bytes to several Capabilities under one identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectionArtifact {
    /// Core identity of the immutable source artifact.
    pub id: ArtifactId,
    /// SHA-256 of the artifact content.
    pub digest: Digest,
    /// Media type of the artifact content.
    pub media_type: BoundedName,
}

/// Returns the current content-inspection Capability identity.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant violates its bounded
/// representation. Such a failure indicates a build-time defect.
pub fn security_content_inspect_capability() -> Result<VersionedSchema, SecurityContractBuildError>
{
    versioned_schema(
        SECURITY_CONTENT_INSPECT_CAPABILITY_ID,
        SECURITY_CONTENT_INSPECT_CAPABILITY_VERSION,
    )
}

/// Returns the exact current canonical content-inspection input Contract.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant is invalid.
pub fn security_content_inspect_input_contract(
) -> Result<SchemaReference, SecurityContractBuildError> {
    schema_reference(
        SECURITY_CONTENT_INSPECT_INPUT_SCHEMA_ID,
        SECURITY_CONTENT_INSPECT_INPUT_SCHEMA_VERSION,
        SECURITY_CONTENT_INSPECT_INPUT_SCHEMA_SHA256,
    )
}

/// Returns the exact current canonical content-inspection output Contract.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant is invalid.
pub fn security_content_inspect_output_contract(
) -> Result<SchemaReference, SecurityContractBuildError> {
    schema_reference(
        SECURITY_CONTENT_INSPECT_OUTPUT_SCHEMA_ID,
        SECURITY_CONTENT_INSPECT_OUTPUT_SCHEMA_VERSION,
        SECURITY_CONTENT_INSPECT_OUTPUT_SCHEMA_SHA256,
    )
}

/// Returns the current code-inspection Capability identity.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant is invalid.
pub fn security_code_inspect_capability() -> Result<VersionedSchema, SecurityContractBuildError> {
    versioned_schema(
        SECURITY_CODE_INSPECT_CAPABILITY_ID,
        SECURITY_CODE_INSPECT_CAPABILITY_VERSION,
    )
}

/// Returns the exact current canonical code-inspection input Contract.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant is invalid.
pub fn security_code_inspect_input_contract() -> Result<SchemaReference, SecurityContractBuildError>
{
    schema_reference(
        SECURITY_CODE_INSPECT_INPUT_SCHEMA_ID,
        SECURITY_CODE_INSPECT_INPUT_SCHEMA_VERSION,
        SECURITY_CODE_INSPECT_INPUT_SCHEMA_SHA256,
    )
}

/// Returns the exact current canonical code-inspection output Contract.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant is invalid.
pub fn security_code_inspect_output_contract() -> Result<SchemaReference, SecurityContractBuildError>
{
    schema_reference(
        SECURITY_CODE_INSPECT_OUTPUT_SCHEMA_ID,
        SECURITY_CODE_INSPECT_OUTPUT_SCHEMA_VERSION,
        SECURITY_CODE_INSPECT_OUTPUT_SCHEMA_SHA256,
    )
}

/// Returns the current command-inspection Capability identity.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant is invalid.
pub fn security_command_inspect_capability() -> Result<VersionedSchema, SecurityContractBuildError>
{
    versioned_schema(
        SECURITY_COMMAND_INSPECT_CAPABILITY_ID,
        SECURITY_COMMAND_INSPECT_CAPABILITY_VERSION,
    )
}

/// Returns the exact current canonical command-inspection input Contract.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant is invalid.
pub fn security_command_inspect_input_contract(
) -> Result<SchemaReference, SecurityContractBuildError> {
    schema_reference(
        SECURITY_COMMAND_INSPECT_INPUT_SCHEMA_ID,
        SECURITY_COMMAND_INSPECT_INPUT_SCHEMA_VERSION,
        SECURITY_COMMAND_INSPECT_INPUT_SCHEMA_SHA256,
    )
}

/// Returns the exact current canonical command-inspection output Contract.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant is invalid.
pub fn security_command_inspect_output_contract(
) -> Result<SchemaReference, SecurityContractBuildError> {
    schema_reference(
        SECURITY_COMMAND_INSPECT_OUTPUT_SCHEMA_ID,
        SECURITY_COMMAND_INSPECT_OUTPUT_SCHEMA_VERSION,
        SECURITY_COMMAND_INSPECT_OUTPUT_SCHEMA_SHA256,
    )
}

fn schema_reference(
    id: &str,
    version: u16,
    digest: &str,
) -> Result<SchemaReference, SecurityContractBuildError> {
    Ok(SchemaReference {
        schema: versioned_schema(id, version)?,
        digest: Digest::parse(digest)?,
    })
}

fn versioned_schema(id: &str, version: u16) -> Result<VersionedSchema, SecurityContractBuildError> {
    Ok(VersionedSchema {
        id: BoundedName::new(id)?,
        version,
    })
}

#[cfg(test)]
mod tests {
    use sha2::{Digest as _, Sha256};

    use super::*;

    #[test]
    fn built_in_security_contracts_are_canonical() {
        let content = security_content_inspect_capability().expect("Capability is canonical");
        let code = security_code_inspect_capability().expect("Capability is canonical");
        let command = security_command_inspect_capability().expect("Capability is canonical");

        assert_eq!(content.id.as_str(), SECURITY_CONTENT_INSPECT_CAPABILITY_ID);
        assert_eq!(code.id.as_str(), SECURITY_CODE_INSPECT_CAPABILITY_ID);
        assert_eq!(command.id.as_str(), SECURITY_COMMAND_INSPECT_CAPABILITY_ID);

        for contract in [
            security_content_inspect_input_contract(),
            security_content_inspect_output_contract(),
            security_code_inspect_input_contract(),
            security_code_inspect_output_contract(),
            security_command_inspect_input_contract(),
            security_command_inspect_output_contract(),
        ] {
            contract.expect("compiled-in Contract is canonical");
        }
    }

    #[test]
    fn canonical_security_schema_resources_match_their_contract_digests() {
        for (bytes, expected) in [
            (
                &include_bytes!("../schemas/security-content-inspect-input-v1.schema.json")[..],
                SECURITY_CONTENT_INSPECT_INPUT_SCHEMA_SHA256,
            ),
            (
                &include_bytes!("../schemas/security-content-inspect-output-v1.schema.json")[..],
                SECURITY_CONTENT_INSPECT_OUTPUT_SCHEMA_SHA256,
            ),
            (
                &include_bytes!("../schemas/security-code-inspect-input-v1.schema.json")[..],
                SECURITY_CODE_INSPECT_INPUT_SCHEMA_SHA256,
            ),
            (
                &include_bytes!("../schemas/security-code-inspect-output-v1.schema.json")[..],
                SECURITY_CODE_INSPECT_OUTPUT_SCHEMA_SHA256,
            ),
            (
                &include_bytes!("../schemas/security-command-inspect-input-v1.schema.json")[..],
                SECURITY_COMMAND_INSPECT_INPUT_SCHEMA_SHA256,
            ),
            (
                &include_bytes!("../schemas/security-command-inspect-output-v1.schema.json")[..],
                SECURITY_COMMAND_INSPECT_OUTPUT_SCHEMA_SHA256,
            ),
        ] {
            assert_eq!(format!("{:x}", Sha256::digest(bytes)), expected);
        }
    }

    #[test]
    fn rule_ids_reject_characters_that_could_carry_matched_content() {
        for rejected in [
            "AKIA1234",
            "aws key",
            "rule/slash",
            "rule:colon",
            "rule=value",
            "パス",
        ] {
            assert_eq!(
                SecurityRuleId::parse(rejected),
                Err(SecurityRuleIdError::InvalidCharacter),
                "{rejected} must be rejected"
            );
        }
        assert_eq!(SecurityRuleId::parse(""), Err(SecurityRuleIdError::Empty));
        assert_eq!(
            SecurityRuleId::parse("a".repeat(MAX_SECURITY_RULE_ID_BYTES + 1)),
            Err(SecurityRuleIdError::TooLong)
        );

        for accepted in ["pii.aws_access_key", "shell-rm-rf", "rule.v2", "a"] {
            SecurityRuleId::parse(accepted).expect("stable label is accepted");
        }
    }

    #[test]
    fn inspection_results_reject_unknown_fields() {
        let smuggled = serde_json::json!({
            "verdict": "sensitive",
            "findings": [{
                "rule_id": "pii.aws_access_key",
                "category": "secret",
                "severity": "critical",
                "confidence": "high",
                "count": 1,
                "match": "AKIAIOSFODNN7EXAMPLE"
            }],
            "scanned_bytes": 42,
            "truncated": false
        });

        assert!(serde_json::from_value::<ContentInspection>(smuggled).is_err());
    }

    #[test]
    fn observation_validation_rejects_unsupported_verdict_claims() {
        let clean = ContentInspection {
            verdict: SecurityInspectionVerdict::Clean,
            findings: Vec::new(),
            scanned_bytes: 42,
            truncated: false,
        };
        assert_eq!(clean.validate(), Ok(()));

        let clean_with_finding = ContentInspection {
            findings: vec![finding(1)],
            ..clean.clone()
        };
        assert_eq!(
            clean_with_finding.validate(),
            Err(SecurityOutputValidationError::CleanHasFindings {
                kind: SecurityObservationKind::Content,
            })
        );

        let truncated_clean = ContentInspection {
            truncated: true,
            ..clean
        };
        assert_eq!(
            truncated_clean.validate(),
            Err(SecurityOutputValidationError::CleanWasTruncated {
                kind: SecurityObservationKind::Content,
            })
        );

        let unsupported_suspicious = CodeInspection {
            verdict: SecurityInspectionVerdict::Suspicious,
            findings: Vec::new(),
            scanned_bytes: 42,
            truncated: false,
            language_detected: SecurityDetectedLanguage::Mixed,
        };
        assert_eq!(
            unsupported_suspicious.validate(),
            Err(
                SecurityOutputValidationError::ObservationVerdictWithoutFindings {
                    kind: SecurityObservationKind::Code,
                    verdict: SecurityInspectionVerdict::Suspicious,
                }
            )
        );

        let supported_sensitive = CodeInspection {
            verdict: SecurityInspectionVerdict::Sensitive,
            findings: vec![finding(1)],
            scanned_bytes: 42,
            truncated: true,
            language_detected: SecurityDetectedLanguage::Mixed,
        };
        assert_eq!(supported_sensitive.validate(), Ok(()));
        assert_eq!(
            serde_json::to_value(supported_sensitive)
                .expect("mixed language is serializable")
                .pointer("/language_detected"),
            Some(&serde_json::json!("mixed"))
        );

        let missing_language = serde_json::json!({
            "verdict": "clean",
            "findings": [],
            "scanned_bytes": 42,
            "truncated": false
        });
        assert!(serde_json::from_value::<CodeInspection>(missing_language).is_err());
    }

    #[test]
    fn observation_validation_rejects_invalid_finding_cardinality() {
        let zero_count = ContentInspection {
            verdict: SecurityInspectionVerdict::Sensitive,
            findings: vec![finding(0)],
            scanned_bytes: 42,
            truncated: false,
        };
        assert_eq!(
            zero_count.validate(),
            Err(SecurityOutputValidationError::ZeroFindingCount { index: 0 })
        );

        let oversized = ContentInspection {
            verdict: SecurityInspectionVerdict::Sensitive,
            findings: vec![finding(1); MAX_OBSERVATION_FINDINGS + 1],
            scanned_bytes: 42,
            truncated: false,
        };
        assert_eq!(
            oversized.validate(),
            Err(SecurityOutputValidationError::TooManyFindings {
                actual: MAX_OBSERVATION_FINDINGS + 1,
            })
        );
    }

    #[test]
    fn command_validation_binds_verdicts_to_evidence() {
        let allow = CommandInspection {
            verdict: CommandVerdict::Allow,
            reasons: Vec::new(),
            findings: Vec::new(),
            scanned_bytes: 42,
        };
        assert_eq!(allow.validate(), Ok(()));

        let allow_with_reason = CommandInspection {
            reasons: vec![rule_id()],
            ..allow.clone()
        };
        assert_eq!(
            allow_with_reason.validate(),
            Err(SecurityOutputValidationError::AllowHasReasons)
        );

        let allow_with_finding = CommandInspection {
            findings: vec![finding(1)],
            ..allow
        };
        assert_eq!(
            allow_with_finding.validate(),
            Err(SecurityOutputValidationError::AllowHasFindings)
        );

        let warn_without_reason = CommandInspection {
            verdict: CommandVerdict::Warn,
            reasons: Vec::new(),
            findings: vec![finding(1)],
            scanned_bytes: 42,
        };
        assert_eq!(
            warn_without_reason.validate(),
            Err(SecurityOutputValidationError::GateVerdictWithoutReasons {
                verdict: CommandVerdict::Warn,
            })
        );

        let deny_without_finding = CommandInspection {
            verdict: CommandVerdict::Deny,
            reasons: vec![rule_id()],
            findings: Vec::new(),
            scanned_bytes: 42,
        };
        assert_eq!(
            deny_without_finding.validate(),
            Err(SecurityOutputValidationError::GateVerdictWithoutFindings {
                verdict: CommandVerdict::Deny,
            })
        );

        let deny = CommandInspection {
            findings: vec![finding(1)],
            ..deny_without_finding
        };
        assert_eq!(deny.validate(), Ok(()));
    }

    fn finding(count: u32) -> SecurityFinding {
        SecurityFinding {
            rule_id: rule_id(),
            category: SecurityFindingCategory::DangerousPattern,
            severity: SecurityFindingSeverity::High,
            confidence: SecurityFindingConfidence::High,
            count,
        }
    }

    fn rule_id() -> SecurityRuleId {
        SecurityRuleId::parse("shell.dangerous_pattern")
            .expect("fixture security rule identity is valid")
    }
}
