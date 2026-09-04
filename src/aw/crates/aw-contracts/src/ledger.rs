#![forbid(unsafe_code)]
//! Versioned Ledger Contracts shared by every writer and reader.
//!
//! The Ledger is the durable append-only record of every AW boundary event
//! worth auditing: plan snapshots, Observe evidence, Mediate credentials,
//! Provider receipts, and their hash chain. This module owns only the
//! schema-shaped types and the event taxonomy. Storage, admission, hash-chain
//! verification, and queries live in `aw-ledger`.
//!
//! Content-freedom rule: Ledger records carry bounded metadata, digests, and
//! IDs only. Raw tool input, tool output, and command text are never stored
//! — readers reconstruct facts from the referenced Artifact and Provider
//! receipts.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::common::{BoundedName, Digest};
use crate::context::ContextReversibility;
use crate::ids::{
    ArtifactId, AttemptId, LedgerCredentialId, LedgerEventId, LedgerEvidenceId, LedgerProjectionId,
    ProviderInvocationId, ToolUseId,
};
use crate::provider::{ProviderDisposition, ProviderReceipt, VersionedSchema};
use crate::security::{
    GateDegradation, ObservationGapReason, SecurityDetectedLanguage, SecurityFinding,
    SecurityFindingCategory, SecurityFindingConfidence, SecurityFindingSeverity,
    SecurityInspectionVerdict, SecurityRuleId, ToolCallGate,
};

/// Schema revision governing [`PostToolUsePlanBody`].
pub const LEDGER_POST_TOOL_USE_PLAN_SCHEMA: &str = "aw.ledger.post_tool_use_plan/v1";

/// Schema revision governing [`PreToolUseGateBody`].
pub const LEDGER_PRE_TOOL_USE_GATE_SCHEMA: &str = "aw.ledger.pre_tool_use_gate/v1";

/// Taxonomy of events the Ledger records.
///
/// Variants are additive: a later release can append a variant without
/// invalidating older records, because every stored event already names its
/// schema revision through `LedgerRecordHeader::schema`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerEventKind {
    /// The plan resolved for one PostToolUse boundary, naming which
    /// Capabilities run and in which order.
    PostToolUsePlan,
    /// The PreToolUse Mediate gate produced a credential (block, ask, allow,
    /// or warn).
    PreToolUseGate,
    /// A Provider invocation completed and its receipt was admitted.
    ProviderInvoked,
    /// An Observe evidence bundle was attached to an existing plan event.
    EvidenceStored,
    /// A Provider receipt was attached to an existing plan event.
    ReceiptStored,
}

/// Header fields shared by every Ledger record regardless of its payload.
///
/// The header commits to the payload digest; the hash chain commits to the
/// previous record's digest. Together they form a tamper-evident sequence that
/// a reader can recompute without the body bytes in memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRecordHeader {
    /// Stable identity of this record.
    pub id: LedgerEventId,
    /// Monotonic, gap-free sequence number within the Ledger.
    pub sequence: u64,
    /// Wall-clock timestamp, milliseconds since the Unix epoch, observed by
    /// the writer at append time.
    pub timestamp_ms: u64,
    /// Which event taxonomy entry this record records.
    pub kind: LedgerEventKind,
    /// Schema revision governing `body`. The hash chain treats this as opaque
    /// text; a reader uses it to pick a deserializer.
    pub schema: String,
    /// Query axes committed as part of the canonical record.
    ///
    /// Storage may duplicate these values in an index table, but that table
    /// must never be treated as an independent source of truth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<LedgerTraceScope>,
    /// Parent link committing to the immediately preceding record. Absent
    /// only on the genesis record at sequence zero. Bundling ID and digest
    /// keeps the header from referencing one without the other.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<LedgerParent>,
    /// Canonical JSON v1 digest of this record's body.
    pub body_digest: Digest,
}

/// A link to the immediately preceding Ledger record.
///
/// Both the identity and the digest of that record travel together so a
/// reader can recompute the hash chain by fetching one parent at a time and
/// verifying the bytes it actually stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerParent {
    /// Identity of the preceding record.
    pub id: LedgerEventId,
    /// Digest of the preceding record's canonical bytes.
    pub digest: Digest,
}

/// Identity-bearing references for the payload bodies stored alongside the
/// Ledger. Each variant pins one body by its typed identity plus the digest
/// the writer committed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LedgerBodyRef {
    /// A Capability plan projection snapshot.
    Projection {
        /// Projection identity.
        id: LedgerProjectionId,
        /// Canonical JSON v1 digest of the projection body.
        digest: Digest,
    },
    /// An Observe evidence bundle.
    Evidence {
        /// Evidence identity.
        id: LedgerEvidenceId,
        /// Canonical JSON v1 digest of the evidence body.
        digest: Digest,
    },
    /// A Mediate gate credential.
    Credential {
        /// Credential identity.
        id: LedgerCredentialId,
        /// Canonical JSON v1 digest of the credential body.
        digest: Digest,
    },
    /// A Provider invocation receipt already recorded by the Host.
    Receipt {
        /// Provider invocation identity.
        invocation: ProviderInvocationId,
        /// Canonical JSON v1 digest of the receipt body.
        digest: Digest,
    },
    /// A source artifact referenced by an Observe or Mediate finding.
    Artifact {
        /// Artifact identity.
        id: ArtifactId,
        /// Canonical JSON v1 digest of the artifact metadata envelope.
        digest: Digest,
    },
}

/// Stable scope keys recorded with a Ledger event so a reader can filter the
/// trace by execution, attempt, or tool call without touching the payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerTraceScope {
    /// Attempt this event contributes to, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<AttemptId>,
    /// Tool use this event is about, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<ToolUseId>,
    /// Provider invocation this event records or references, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invocation_id: Option<ProviderInvocationId>,
}

/// Content-free reference to one Provider invocation a plan step used.
///
/// This is a pointer, not a copy of the receipt. It names the invocation so a
/// reader can fetch the full receipt from the Provider Host, and carries only
/// the fields needed to interpret the plan without that round trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerInvocationRef {
    /// Core-owned invocation whose result the plan step consumed.
    pub invocation_id: ProviderInvocationId,
    /// Provider identity that served the step.
    pub provider_id: BoundedName,
    /// Provider release declared by the admitted manifest.
    pub provider_version: BoundedName,
    /// Digest of the exact Provider manifest admitted for this invocation.
    pub manifest_digest: Digest,
    /// Capability the Provider served.
    pub capability: VersionedSchema,
    /// Canonical input schema accepted by Core.
    pub input_schema: VersionedSchema,
    /// Digest of the canonical input body accepted by Core.
    pub input_digest: Digest,
    /// Terminal classification Core assigned to the invocation.
    pub disposition: ProviderDisposition,
    /// Schema of the transient Provider output, when one existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<VersionedSchema>,
    /// Digest of the transient Provider output, when one existed. The output
    /// body itself is never stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_digest: Option<Digest>,
    /// Unix timestamp at which Provider work began.
    pub started_at_ms: u64,
    /// Unix timestamp at which the Provider reported the terminal fact.
    pub completed_at_ms: u64,
}

impl LedgerInvocationRef {
    /// Projects the content-free subset of `receipt` the Ledger records.
    #[must_use]
    pub fn from_receipt(receipt: &ProviderReceipt) -> Self {
        Self {
            invocation_id: receipt.invocation_id.clone(),
            provider_id: receipt.provider_id.clone(),
            provider_version: receipt.provider_version.clone(),
            manifest_digest: receipt.manifest_digest.clone(),
            capability: receipt.capability.clone(),
            input_schema: receipt.input_schema.clone(),
            input_digest: receipt.input_digest.clone(),
            disposition: receipt.disposition,
            output_schema: receipt.output_schema.clone(),
            output_digest: receipt.output_digest.clone(),
            started_at_ms: receipt.started_at_ms,
            completed_at_ms: receipt.completed_at_ms,
        }
    }
}

/// Content-free durable projection of one Provider rule finding.
///
/// `rule_id_digest` is SHA-256 over the exact UTF-8 bytes of the transient
/// security rule ID. Keeping the digest allows stable correlation with a
/// separately governed rule catalog without giving an arbitrary Provider
/// label a durable text channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRuleFinding {
    /// Stable SHA-256 identity of the transient Provider rule ID.
    pub rule_id_digest: Digest,
    /// Broad closed category of the finding.
    pub category: SecurityFindingCategory,
    /// Closed severity assigned by the Provider.
    pub severity: SecurityFindingSeverity,
    /// Closed confidence assigned by the Provider.
    pub confidence: SecurityFindingConfidence,
    /// Number of matches attributed to the rule.
    pub count: u32,
}

impl From<&SecurityFinding> for LedgerRuleFinding {
    fn from(finding: &SecurityFinding) -> Self {
        Self {
            rule_id_digest: security_rule_id_digest(&finding.rule_id),
            category: finding.category,
            severity: finding.severity,
            confidence: finding.confidence,
            count: finding.count,
        }
    }
}

/// Returns the stable Ledger identity for a transient security rule ID.
///
/// The digest is SHA-256 over the exact UTF-8 bytes returned by
/// [`SecurityRuleId::as_str`].
#[must_use]
pub fn security_rule_id_digest(rule_id: &SecurityRuleId) -> Digest {
    let hex = format!("{:x}", Sha256::digest(rule_id.as_str().as_bytes()));
    // LowerHex over SHA-256's fixed 32 bytes always produces canonical text.
    Digest::parse(hex).expect("SHA-256 output is a canonical digest")
}

/// Content-free record of one Observe step that produced facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerObservation {
    /// Capability that produced these facts.
    pub capability: VersionedSchema,
    /// Highest-level conclusion the implementation reported.
    pub verdict: SecurityInspectionVerdict,
    /// Per-rule counts. A finding never carries the value it matched.
    pub findings: Vec<LedgerRuleFinding>,
    /// Bytes the implementation reported inspecting.
    pub scanned_bytes: u64,
    /// Whether the implementation stopped before the whole artifact.
    pub truncated: bool,
    /// Language a code inspection reported analysing, when it classified one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_detected: Option<SecurityDetectedLanguage>,
    /// Invocation that produced these facts.
    pub invocation: LedgerInvocationRef,
}

/// Content-free record of one planned Observe step that produced no fact.
///
/// A gap is itself a recorded fact. Without it a reader cannot distinguish
/// "nothing was found" from "nobody looked".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerObservationGap {
    /// Capability the plan named but could not complete.
    pub capability: VersionedSchema,
    /// Why the observation is absent.
    pub reason: ObservationGapReason,
    /// Invocation reference when the step reached a settled receipt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invocation: Option<LedgerInvocationRef>,
}

/// Content-free record of the Advise context-projection step.
///
/// The candidate representation is deliberately absent. A projection candidate
/// carries model-visible text, so the Ledger stores its digest and bounded
/// shape metadata and leaves the bytes to the Artifact store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerProjectionOutcome {
    /// Whether the Provider offered a candidate at all.
    pub candidate_offered: bool,
    /// Number of transformations the Provider declared.
    ///
    /// Transformation names are Provider-controlled text, so the durable
    /// projection records only their bounded cardinality.
    pub transform_count: u64,
    /// Recoverability guarantee the candidate declared, when one was offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reversibility: Option<ContextReversibility>,
    /// Invocation that produced the projection step.
    pub invocation: LedgerInvocationRef,
}

/// Body of a [`LedgerEventKind::PostToolUsePlan`] record.
///
/// This is the plan-shaped audit fact for one PostToolUse boundary: which
/// source artifact was inspected, what each Observe Capability concluded,
/// which planned Capabilities produced nothing and why, and what the Advise
/// step offered. The tool response never appears.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostToolUsePlanBody {
    /// Core identity allocated to the immutable source artifact.
    pub source_artifact_id: ArtifactId,
    /// SHA-256 of the original tool-result content.
    pub source_digest: Digest,
    /// Observe facts in deterministic plan order.
    pub observations: Vec<LedgerObservation>,
    /// Planned Observe Capabilities that produced no fact, and why.
    pub observation_gaps: Vec<LedgerObservationGap>,
    /// Result of the single Advise context-projection step.
    pub projection: LedgerProjectionOutcome,
}

/// Body of a [`LedgerEventKind::PreToolUseGate`] record.
///
/// The gate decision is recorded without the command that triggered it.
/// `reasons` carries only stable digests of transient rule IDs, so a Provider
/// cannot use a rule label as a durable text channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreToolUseGateBody {
    /// Gate outcome Core required the Agent Environment to honour.
    pub gate: ToolCallGate,
    /// SHA-256 identities of transient Provider rationale codes.
    pub reasons: Vec<Digest>,
    /// Why the gate resolved without an implementation verdict, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degradation: Option<GateDegradation>,
    /// Invocation that produced the verdict, when Core accepted one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invocation: Option<LedgerInvocationRef>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::canonical_json_v1_bytes;

    fn empty_digest() -> Digest {
        Digest::parse("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
            .expect("empty SHA-256 parses")
    }

    #[test]
    fn event_kind_round_trips_through_snake_case_json() {
        let cases = [
            (LedgerEventKind::PostToolUsePlan, "\"post_tool_use_plan\""),
            (LedgerEventKind::PreToolUseGate, "\"pre_tool_use_gate\""),
            (LedgerEventKind::ProviderInvoked, "\"provider_invoked\""),
            (LedgerEventKind::EvidenceStored, "\"evidence_stored\""),
            (LedgerEventKind::ReceiptStored, "\"receipt_stored\""),
        ];
        for (kind, expected) in cases {
            let encoded = serde_json::to_string(&kind).expect("kind serializes");
            assert_eq!(encoded, expected);
            let decoded: LedgerEventKind =
                serde_json::from_str(&encoded).expect("kind deserializes");
            assert_eq!(decoded, kind);
        }
    }

    #[test]
    fn unknown_event_kind_is_rejected() {
        let result = serde_json::from_str::<LedgerEventKind>("\"future_variant\"");
        assert!(result.is_err(), "unknown variants must fail closed");
    }

    #[test]
    fn rule_id_digest_has_a_stable_utf8_contract() {
        let rule_id = SecurityRuleId::parse("shell.dangerous_pattern").expect("rule ID parses");
        assert_eq!(
            security_rule_id_digest(&rule_id).as_str(),
            "e2625abf9c98b0fad14078643eb69acfe725ec083556357a09250e862cd697e7"
        );
    }

    #[test]
    fn body_ref_tag_is_stable_and_digests_match() {
        let projection = LedgerBodyRef::Projection {
            id: LedgerProjectionId::new(),
            digest: empty_digest(),
        };
        let encoded = serde_json::to_string(&projection).expect("body ref serializes");
        assert!(
            encoded.contains("\"kind\":\"projection\""),
            "tag must be stable for schema readers: {encoded}"
        );
        let decoded: LedgerBodyRef = serde_json::from_str(&encoded).expect("body ref deserializes");
        assert_eq!(decoded, projection);
    }

    #[test]
    fn record_header_digest_is_over_canonical_bytes() {
        // Construct one header, encode it canonically, and re-decode. The
        // bytes we commit to must be the same bytes a reader re-digests.
        let header = LedgerRecordHeader {
            id: LedgerEventId::new(),
            sequence: 7,
            timestamp_ms: 1_725_300_000_000,
            kind: LedgerEventKind::PreToolUseGate,
            schema: "aw.ledger.pre_tool_use_gate/v1".to_owned(),
            scope: Some(LedgerTraceScope {
                attempt_id: Some(AttemptId::new()),
                tool_use_id: None,
                invocation_id: None,
            }),
            parent: Some(LedgerParent {
                id: LedgerEventId::new(),
                digest: Digest::parse(
                    "0000000000000000000000000000000000000000000000000000000000000000",
                )
                .expect("zero digest parses"),
            }),
            body_digest: empty_digest(),
        };
        let value = serde_json::to_value(&header).expect("header becomes a JSON value");
        let canonical = canonical_json_v1_bytes(&value).expect("canonical encoding succeeds");
        let decoded: LedgerRecordHeader =
            serde_json::from_slice(&canonical).expect("canonical header round-trips");
        assert_eq!(decoded, header);
    }
}
