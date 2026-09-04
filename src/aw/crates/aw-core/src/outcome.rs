//! Core results for one Agent Environment event.
//!
//! Every fact is paired with the receipt that produced it, so a later Ledger
//! writer never has to re-associate a Provider outcome with its invocation.

use aw_contracts::common::Digest;
use aw_contracts::context::ContextProjectionCandidate;
use aw_contracts::error::ContractError;
use aw_contracts::ids::ArtifactId;
use aw_contracts::ledger::{
    security_rule_id_digest, LedgerInvocationRef, LedgerObservation, LedgerObservationGap,
    LedgerProjectionOutcome, LedgerRuleFinding, PostToolUsePlanBody, PreToolUseGateBody,
};
use aw_contracts::provider::{ProviderReceipt, VersionedSchema};
use aw_contracts::security::{
    GateDegradation, ObservationGapReason, SecurityDetectedLanguage, SecurityFinding,
    SecurityFindingSeverity, SecurityInspectionVerdict, SecurityRuleId, ToolCallGate,
};
use serde::Serialize;

/// Advise candidate paired with the receipt for the invocation that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreparedProjection {
    /// Provider proposal available for a later Core adoption decision.
    ///
    /// A bypassed, denied, failed, or uncertain invocation carries no candidate
    /// even when the implementation returned transient output.
    pub candidate: Option<ContextProjectionCandidate>,
    /// Content-free terminal Provider facts safe for persistence and display.
    pub receipt: ProviderReceipt,
}

/// One accepted Observe result, normalized across inspection Capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityObservation {
    /// Capability that produced these facts.
    pub capability: VersionedSchema,
    /// Highest-level conclusion the implementation reported.
    pub verdict: SecurityInspectionVerdict,
    /// Per-rule counts. A finding never carries the value it matched.
    pub findings: Vec<SecurityFinding>,
    /// Bytes the implementation reported inspecting.
    pub scanned_bytes: u64,
    /// Whether the implementation stopped before the whole artifact.
    pub truncated: bool,
    /// Language a code inspection reported analysing, when it classified one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_detected: Option<SecurityDetectedLanguage>,
    /// Content-free receipt for the invocation that produced these facts.
    pub receipt: ProviderReceipt,
}

impl CapabilityObservation {
    /// Returns the most severe severity across the findings, if any.
    #[must_use]
    pub fn peak_severity(&self) -> Option<SecurityFindingSeverity> {
        self.findings.iter().map(|finding| finding.severity).max()
    }

    /// Returns the total number of matches attributed to all findings.
    #[must_use]
    pub fn matched_total(&self) -> u64 {
        self.findings
            .iter()
            .map(|finding| u64::from(finding.count))
            .sum()
    }
}

/// One planned Observe Capability that produced no usable fact.
///
/// A gap is a recorded fact in its own right. Core reports why an observation is
/// absent instead of collapsing every cause into a silent success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObservationGap {
    /// Capability Core planned but could not complete.
    pub capability: VersionedSchema,
    /// Why the observation is absent.
    pub reason: ObservationGapReason,
    /// Provider target for an invocation-level gap. Route-level gaps have no
    /// target because Core selected no implementation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<aw_contracts::common::BoundedName>,
    /// Bounded safe failure, when the invocation reached a settled receipt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ContractError>,
    /// Receipt when Core accepted an invocation that then settled unusable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ProviderReceipt>,
}

/// Core result for one observed tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolResultOutcome {
    /// Core identity allocated to the immutable source artifact.
    pub source_artifact_id: ArtifactId,
    /// SHA-256 of the original tool-result content.
    pub source_digest: Digest,
    /// UTF-8 byte count of the original tool-result content.
    pub source_byte_count: u64,
    /// Result of the single Advise context-projection step.
    pub projection: PreparedProjection,
    /// Content-free Observe facts in deterministic plan order.
    pub observations: Vec<CapabilityObservation>,
    /// Planned Observe Capabilities that produced no fact, and why.
    pub observation_gaps: Vec<ObservationGap>,
}

impl ToolResultOutcome {
    /// Returns every accepted receipt in deterministic outcome-group order.
    ///
    /// Produced Observe facts come first, followed by settled Observe gaps and
    /// then the Advise receipt. Because facts and gaps have separate public
    /// collections, their receipts do not reconstruct an interleaved plan.
    /// A gap that never reached an accepted invocation contributes no receipt.
    #[must_use]
    pub fn receipts(&self) -> Vec<&ProviderReceipt> {
        self.observations
            .iter()
            .map(|observation| &observation.receipt)
            .chain(
                self.observation_gaps
                    .iter()
                    .filter_map(|gap| gap.receipt.as_ref()),
            )
            .chain(std::iter::once(&self.projection.receipt))
            .collect()
    }

    /// Returns the most severe severity observed across all inspections.
    #[must_use]
    pub fn peak_severity(&self) -> Option<SecurityFindingSeverity> {
        self.observations
            .iter()
            .filter_map(CapabilityObservation::peak_severity)
            .max()
    }

    /// Returns the total number of matches observed across all inspections.
    #[must_use]
    pub fn matched_total(&self) -> u64 {
        self.observations
            .iter()
            .map(CapabilityObservation::matched_total)
            .sum()
    }

    /// Projects this outcome into the content-free Ledger record body.
    ///
    /// The Advise candidate is reduced to closed metadata and a count. A
    /// candidate carries model-visible representation and Provider-controlled
    /// labels, so copying either into a Ledger record would defeat
    /// content-freedom. Invocation schema and digests identify the exact
    /// transient exchange without retaining those bytes.
    #[must_use]
    pub fn ledger_body(&self) -> PostToolUsePlanBody {
        PostToolUsePlanBody {
            source_artifact_id: self.source_artifact_id.clone(),
            source_digest: self.source_digest.clone(),
            source_byte_count: self.source_byte_count,
            observations: self
                .observations
                .iter()
                .map(|observation| LedgerObservation {
                    capability: observation.capability.clone(),
                    verdict: observation.verdict,
                    findings: observation
                        .findings
                        .iter()
                        .map(LedgerRuleFinding::from)
                        .collect(),
                    scanned_bytes: observation.scanned_bytes,
                    truncated: observation.truncated,
                    language_detected: observation.language_detected,
                    invocation: LedgerInvocationRef::from_receipt(&observation.receipt),
                })
                .collect(),
            observation_gaps: self
                .observation_gaps
                .iter()
                .map(|gap| LedgerObservationGap {
                    capability: gap.capability.clone(),
                    reason: gap.reason,
                    provider_id: gap.provider_id.clone(),
                    invocation: gap.receipt.as_ref().map(LedgerInvocationRef::from_receipt),
                })
                .collect(),
            projection: LedgerProjectionOutcome {
                candidate_offered: self.projection.candidate.is_some(),
                candidate_envelope_digest: self
                    .projection
                    .candidate
                    .as_ref()
                    .and(self.projection.receipt.output_digest.clone()),
                candidate_content_digest: self
                    .projection
                    .candidate
                    .as_ref()
                    .map(|candidate| Digest::sha256(candidate.content.as_bytes())),
                candidate_byte_count: self
                    .projection
                    .candidate
                    .as_ref()
                    .map(|candidate| candidate.content.len() as u64),
                transform_count: self
                    .projection
                    .candidate
                    .as_ref()
                    .map_or(0, |candidate| candidate.transform_chain.len() as u64),
                reversibility: self
                    .projection
                    .candidate
                    .as_ref()
                    .map(|candidate| candidate.reversibility),
                invocation: LedgerInvocationRef::from_receipt(&self.projection.receipt),
            },
        }
    }
}

/// Core gate result for one pending Tool Call.
///
/// [`ToolCallGate::NotMediated`] is not an approval. It states that no governed
/// verdict exists, so an Agent Environment must apply its own default rather
/// than read the absence of an opinion as permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolCallDecision {
    /// SHA-256 of the exact command bytes submitted to Core.
    pub command_digest: Digest,
    /// UTF-8 byte count of the exact command submitted to Core.
    pub command_byte_count: u64,
    /// Gate outcome the Agent Environment must honour.
    pub gate: ToolCallGate,
    /// Rationale codes safe for operator presentation.
    ///
    /// Codes only. The command text never appears here, so a gate notice cannot
    /// echo the argument it refused.
    pub reasons: Vec<SecurityRuleId>,
    /// Content-free receipt when Core accepted a mediation invocation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ProviderReceipt>,
    /// Why the gate resolved without an implementation verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degradation: Option<GateDegradation>,
}

impl ToolCallDecision {
    /// Projects this decision into the content-free Ledger record body.
    ///
    /// Recording a refusal is the point of this record, so stable digests of
    /// its transient rationale codes travel with it. The Provider-controlled
    /// labels themselves remain on the immediate decision path only.
    #[must_use]
    pub fn ledger_body(&self) -> PreToolUseGateBody {
        PreToolUseGateBody {
            command_digest: self.command_digest.clone(),
            command_byte_count: self.command_byte_count,
            gate: self.gate,
            reasons: self.reasons.iter().map(security_rule_id_digest).collect(),
            degradation: self.degradation,
            invocation: self.receipt.as_ref().map(LedgerInvocationRef::from_receipt),
        }
    }
}
