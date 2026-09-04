//! Atomic Ledger append orchestration.
//!
//! The `LedgerSink` coordinates the four steps that every boundary
//! recorder (B6–B8) needs: allocate a record ID, read the wall clock,
//! build a candidate record from the current chain tip, admit it, and
//! append the admitted bytes to the store. Callers provide the event
//! kind, schema, body, and optional trace scope; the sink handles
//! sequencing, parent linking, body digest computation, and admission
//! validation.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use aw_contracts::context::{
    CONTEXT_PROJECTION_PREPARE_CAPABILITY_ID, CONTEXT_PROJECTION_PREPARE_CAPABILITY_VERSION,
    CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_ID, CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_ID,
    MAX_TRANSFORM_CHAIN_ITEMS,
};
use aw_contracts::ids::LedgerEventId;
use aw_contracts::ledger::{
    ContextAdoptionBody, ContextAdoptionDecision, ContextAdoptionReason, LedgerEventKind,
    LedgerInvocationRef, LedgerParent, LedgerRecordHeader, LedgerTraceScope, PostToolUsePlanBody,
    PreToolUseGateBody, LEDGER_CONTEXT_ADOPTION_SCHEMA, LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
    LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
};
use aw_contracts::provider::{ProviderDisposition, VersionedSchema};
use aw_contracts::security::{
    GateDegradation, ObservationGapReason, SecurityInspectionVerdict, ToolCallGate,
    MAX_OBSERVATION_FINDINGS, SECURITY_CODE_INSPECT_CAPABILITY_ID,
    SECURITY_CODE_INSPECT_CAPABILITY_VERSION, SECURITY_CODE_INSPECT_INPUT_SCHEMA_ID,
    SECURITY_CODE_INSPECT_OUTPUT_SCHEMA_ID, SECURITY_COMMAND_INSPECT_CAPABILITY_ID,
    SECURITY_COMMAND_INSPECT_CAPABILITY_VERSION, SECURITY_COMMAND_INSPECT_INPUT_SCHEMA_ID,
    SECURITY_COMMAND_INSPECT_OUTPUT_SCHEMA_ID, SECURITY_CONTENT_INSPECT_CAPABILITY_ID,
    SECURITY_CONTENT_INSPECT_CAPABILITY_VERSION, SECURITY_CONTENT_INSPECT_INPUT_SCHEMA_ID,
    SECURITY_CONTENT_INSPECT_OUTPUT_SCHEMA_ID,
};
use serde_json::Value;
use thiserror::Error;

use crate::admission::AdmissionError;
use crate::store::StoreError;
use crate::{admit, AdmittedRecord, CandidateRecord, ChainTip, LedgerStore};

/// Failure returned by [`LedgerSink::record`].
#[derive(Debug, Error)]
pub enum SinkError {
    /// Admission rejected the candidate.
    #[error("ledger admission rejected: {0}")]
    Admission(#[from] AdmissionError),
    /// The generic sink has no typed writer for this taxonomy entry yet.
    #[error("ledger event kind {kind:?} has no implemented typed writer")]
    UnsupportedEventKind {
        /// Event kind for which no writer contract exists.
        kind: LedgerEventKind,
    },
    /// The event kind was paired with a different body schema.
    #[error("ledger event kind {kind:?} requires schema {expected}, got {actual}")]
    SchemaMismatch {
        /// Event kind whose schema did not match.
        kind: LedgerEventKind,
        /// Schema implemented by this writer.
        expected: &'static str,
        /// Schema supplied by the caller.
        actual: String,
    },
    /// The body did not conform to the implemented typed schema.
    #[error("ledger event kind {kind:?} has an invalid typed body: {source}")]
    InvalidBody {
        /// Event kind whose body failed decoding.
        kind: LedgerEventKind,
        /// Strict typed decoding failure.
        #[source]
        source: serde_json::Error,
    },
    /// A typed PostToolUse plan contradicted its candidate shape.
    #[error("PostToolUse plan violates invariant {invariant}")]
    PostToolUsePlanInvariant {
        /// Stable invariant name suitable for a content-free diagnostic.
        invariant: &'static str,
    },
    /// A typed PreToolUse gate contradicted its decision or invocation.
    #[error("PreToolUse gate violates invariant {invariant}")]
    PreToolUseGateInvariant {
        /// Stable invariant name suitable for a content-free diagnostic.
        invariant: &'static str,
    },
    /// A typed context-adoption body contradicted its own closed fields.
    #[error("context adoption violates invariant {invariant}")]
    ContextAdoptionInvariant {
        /// Stable invariant name suitable for a content-free diagnostic.
        invariant: &'static str,
    },
    /// A context-adoption record referenced no plan in this Ledger.
    #[error("context adoption references an absent PostToolUse plan")]
    ContextAdoptionPlanMissing,
    /// A context-adoption record did not match its referenced plan.
    #[error("context adoption does not match its PostToolUse plan field {field}")]
    ContextAdoptionPlanMismatch {
        /// Stable mismatched field name.
        field: &'static str,
    },
    /// The backing store could not persist the record.
    #[error("ledger store error: {0}")]
    Store(#[from] StoreError),
}

/// Coordinates admission and persistence for one Ledger append.
///
/// A boundary recorder allocates one sink per logical writer (e.g. one
/// per hook invocation) and calls [`Self::record`] for each event. The
/// sink tracks the chain tip internally so successive calls produce a
/// continuous hash chain without the caller managing sequence numbers
/// or parent links.
pub struct LedgerSink {
    store: LedgerStore,
}

impl LedgerSink {
    /// Wraps `store` as the persistence backend. The sink reads the
    /// current chain tip from the store so the first call to
    /// [`Self::record`] produces the correct next record.
    pub fn new(store: LedgerStore) -> Self {
        Self { store }
    }

    /// Admits and persists one record, returning the admitted bytes and
    /// digests.
    ///
    /// # Errors
    ///
    /// Returns [`SinkError::Admission`] when the body violates
    /// content-freedom or any other admission invariant, and
    /// [`SinkError::Store`] when the database refuses the write.
    pub fn record(
        &mut self,
        kind: LedgerEventKind,
        schema: &str,
        body: Value,
        scope: Option<&LedgerTraceScope>,
    ) -> Result<AdmittedRecord, SinkError> {
        let tip = self.store.tip();
        let candidate = build_candidate(&tip, kind, schema, body, scope);
        let admitted = admit(&tip, candidate)?;
        validate_writer_body(kind, schema, &admitted.body, admitted.header.scope.as_ref())?;
        if kind == LedgerEventKind::ContextAdoption {
            let adoption = serde_json::from_value::<ContextAdoptionBody>(admitted.body.clone())
                .map_err(|source| SinkError::InvalidBody { kind, source })?;
            validate_context_adoption(&self.store, &adoption, scope)?;
        }
        self.store.append(&admitted)?;
        Ok(admitted)
    }

    /// Read-only snapshot of the current chain tip.
    pub fn tip(&self) -> ChainTip<'_> {
        self.store.tip()
    }
}

fn build_candidate(
    tip: &ChainTip<'_>,
    kind: LedgerEventKind,
    schema: &str,
    body: Value,
    scope: Option<&LedgerTraceScope>,
) -> CandidateRecord {
    use aw_contracts::canonical::canonical_json_v1_bytes;
    use sha2::{Digest as _, Sha256};

    let body_canonical = canonical_json_v1_bytes(&body).expect("body canonical");
    let body_digest_hex = format!("{:x}", Sha256::digest(&body_canonical));
    let body_digest = aw_contracts::common::Digest::parse(body_digest_hex)
        .expect("sha2 output is always a valid digest");

    let sequence = if tip.id.is_none() {
        0
    } else {
        tip.sequence + 1
    };
    let parent = tip.id.zip(tip.digest).map(|(id, digest)| LedgerParent {
        id: id.clone(),
        digest: digest.clone(),
    });

    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis() as u64;

    CandidateRecord {
        header: LedgerRecordHeader {
            id: LedgerEventId::new(),
            sequence,
            timestamp_ms,
            kind,
            schema: schema.to_owned(),
            scope: scope.cloned(),
            parent,
            body_digest,
        },
        body,
    }
}

fn validate_writer_body(
    kind: LedgerEventKind,
    schema: &str,
    body: &Value,
    scope: Option<&LedgerTraceScope>,
) -> Result<(), SinkError> {
    let expected = match kind {
        LedgerEventKind::PostToolUsePlan => LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
        LedgerEventKind::PreToolUseGate => LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
        LedgerEventKind::ContextAdoption => LEDGER_CONTEXT_ADOPTION_SCHEMA,
        kind => return Err(SinkError::UnsupportedEventKind { kind }),
    };
    if schema != expected {
        return Err(SinkError::SchemaMismatch {
            kind,
            expected,
            actual: schema.to_owned(),
        });
    }

    match kind {
        LedgerEventKind::PostToolUsePlan => {
            let plan = serde_json::from_value::<PostToolUsePlanBody>(body.clone())
                .map_err(|source| SinkError::InvalidBody { kind, source })?;
            validate_post_tool_use_plan(&plan, scope)?;
        }
        LedgerEventKind::PreToolUseGate => {
            let gate = serde_json::from_value::<PreToolUseGateBody>(body.clone())
                .map_err(|source| SinkError::InvalidBody { kind, source })?;
            validate_pre_tool_use_gate(&gate, scope)?;
        }
        LedgerEventKind::ContextAdoption => {
            serde_json::from_value::<ContextAdoptionBody>(body.clone())
                .map_err(|source| SinkError::InvalidBody { kind, source })?;
        }
        _ => unreachable!("unsupported event kinds returned above"),
    }
    Ok(())
}

fn validate_post_tool_use_plan(
    plan: &PostToolUsePlanBody,
    scope: Option<&LedgerTraceScope>,
) -> Result<(), SinkError> {
    let scope = boundary_scope(scope).ok_or(SinkError::PostToolUsePlanInvariant {
        invariant: "header_scope",
    })?;
    let mut covered_observations = [false; 2];
    let mut route_gaps = [false; 2];
    let mut provider_targets = BTreeSet::new();
    let mut invocation_ids = BTreeSet::new();
    for observation in &plan.observations {
        let slot = observation_slot(&observation.capability).ok_or(
            SinkError::PostToolUsePlanInvariant {
                invariant: "observation_capability",
            },
        )?;
        if route_gaps[slot]
            || !provider_targets.insert((slot, observation.invocation.provider_id.clone()))
            || !invocation_ids.insert(observation.invocation.invocation_id.clone())
        {
            return Err(SinkError::PostToolUsePlanInvariant {
                invariant: "observation_plan_uniqueness",
            });
        }
        covered_observations[slot] = true;
        let is_content = schema_is(
            &observation.capability,
            SECURITY_CONTENT_INSPECT_CAPABILITY_ID,
            SECURITY_CONTENT_INSPECT_CAPABILITY_VERSION,
        );
        let is_code = schema_is(
            &observation.capability,
            SECURITY_CODE_INSPECT_CAPABILITY_ID,
            SECURITY_CODE_INSPECT_CAPABILITY_VERSION,
        );
        let contract_matches = if is_content {
            invocation_contract_is(
                &observation.invocation,
                SECURITY_CONTENT_INSPECT_CAPABILITY_ID,
                SECURITY_CONTENT_INSPECT_CAPABILITY_VERSION,
                SECURITY_CONTENT_INSPECT_INPUT_SCHEMA_ID,
                SECURITY_CONTENT_INSPECT_OUTPUT_SCHEMA_ID,
            )
        } else if is_code {
            invocation_contract_is(
                &observation.invocation,
                SECURITY_CODE_INSPECT_CAPABILITY_ID,
                SECURITY_CODE_INSPECT_CAPABILITY_VERSION,
                SECURITY_CODE_INSPECT_INPUT_SCHEMA_ID,
                SECURITY_CODE_INSPECT_OUTPUT_SCHEMA_ID,
            )
        } else {
            false
        };
        if !contract_matches
            || observation.invocation.disposition != ProviderDisposition::Produced
            || validate_invocation_ref(&observation.invocation).is_err()
            || !invocation_matches_scope(&observation.invocation, scope)
        {
            return Err(SinkError::PostToolUsePlanInvariant {
                invariant: "observation_invocation",
            });
        }
        if observation.findings.len() > MAX_OBSERVATION_FINDINGS
            || observation
                .findings
                .iter()
                .any(|finding| finding.count == 0)
        {
            return Err(SinkError::PostToolUsePlanInvariant {
                invariant: "observation_findings",
            });
        }
        match observation.verdict {
            SecurityInspectionVerdict::Clean
                if !observation.findings.is_empty() || observation.truncated =>
            {
                return Err(SinkError::PostToolUsePlanInvariant {
                    invariant: "clean_observation",
                });
            }
            SecurityInspectionVerdict::Suspicious | SecurityInspectionVerdict::Sensitive
                if observation.findings.is_empty() =>
            {
                return Err(SinkError::PostToolUsePlanInvariant {
                    invariant: "finding_observation",
                });
            }
            _ => {}
        }
        if !scan_coverage_is_valid(
            observation.scanned_bytes,
            observation.truncated,
            plan.source_byte_count,
        ) {
            return Err(SinkError::PostToolUsePlanInvariant {
                invariant: "observation_coverage",
            });
        }
        if (is_content && observation.language_detected.is_some())
            || (is_code && observation.language_detected.is_none())
        {
            return Err(SinkError::PostToolUsePlanInvariant {
                invariant: "observation_language",
            });
        }
    }
    for gap in &plan.observation_gaps {
        let slot =
            observation_slot(&gap.capability).ok_or(SinkError::PostToolUsePlanInvariant {
                invariant: "gap_capability",
            })?;
        let contract =
            observation_contract(&gap.capability).ok_or(SinkError::PostToolUsePlanInvariant {
                invariant: "gap_capability",
            })?;
        if !schema_is(&gap.capability, contract.capability_id, contract.version) {
            return Err(SinkError::PostToolUsePlanInvariant {
                invariant: "gap_capability",
            });
        }
        let gap_shape_valid = match gap.reason {
            ObservationGapReason::NoImplementation | ObservationGapReason::ControlsNotEnforced => {
                if covered_observations[slot]
                    || route_gaps[slot]
                    || gap.provider_id.is_some()
                    || gap.invocation.is_some()
                {
                    false
                } else {
                    route_gaps[slot] = true;
                    covered_observations[slot] = true;
                    true
                }
            }
            ObservationGapReason::HostFailure => gap.provider_id.as_ref().is_some_and(|provider| {
                !route_gaps[slot]
                    && gap.invocation.is_none()
                    && provider_targets.insert((slot, provider.clone()))
            }),
            ObservationGapReason::NotProduced | ObservationGapReason::InvalidOutput => {
                gap.provider_id
                    .as_ref()
                    .zip(gap.invocation.as_ref())
                    .is_some_and(|(provider, invocation)| {
                        let disposition_matches = match gap.reason {
                            ObservationGapReason::NotProduced => {
                                invocation.disposition != ProviderDisposition::Produced
                                    && invocation.error_code.as_ref().is_none_or(|code| {
                                        code.as_str() != "provider_invalid_response"
                                    })
                            }
                            ObservationGapReason::InvalidOutput => {
                                invocation.disposition == ProviderDisposition::Produced
                                    || invocation.error_code.as_ref().is_some_and(|code| {
                                        code.as_str() == "provider_invalid_response"
                                    })
                            }
                            _ => unreachable!("matched observation gap reasons above"),
                        };
                        !route_gaps[slot]
                            && provider == &invocation.provider_id
                            && provider_targets.insert((slot, provider.clone()))
                            && invocation_ids.insert(invocation.invocation_id.clone())
                            && invocation_contract_is(
                                invocation,
                                contract.capability_id,
                                contract.version,
                                contract.input_schema_id,
                                contract.output_schema_id,
                            )
                            && validate_invocation_ref(invocation).is_ok()
                            && invocation_matches_scope(invocation, scope)
                            && disposition_matches
                    })
            }
            ObservationGapReason::LedgerUnavailable => false,
        };
        if !gap_shape_valid {
            return Err(SinkError::PostToolUsePlanInvariant {
                invariant: "gap_invocation",
            });
        }
        covered_observations[slot] = true;
    }
    if covered_observations != [true, true] {
        return Err(SinkError::PostToolUsePlanInvariant {
            invariant: "observation_plan_coverage",
        });
    }

    let projection = &plan.projection;
    if !invocation_contract_is(
        &projection.invocation,
        CONTEXT_PROJECTION_PREPARE_CAPABILITY_ID,
        CONTEXT_PROJECTION_PREPARE_CAPABILITY_VERSION,
        CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_ID,
        CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_ID,
    ) || validate_invocation_ref(&projection.invocation).is_err()
        || !invocation_matches_scope(&projection.invocation, scope)
    {
        return Err(SinkError::PostToolUsePlanInvariant {
            invariant: "projection_invocation",
        });
    }
    if !invocation_ids.insert(projection.invocation.invocation_id.clone()) {
        return Err(SinkError::PostToolUsePlanInvariant {
            invariant: "provider_invocation_uniqueness",
        });
    }
    let complete_candidate_shape = projection.candidate_envelope_digest.is_some()
        && projection.candidate_content_digest.is_some()
        && projection.candidate_byte_count.is_some()
        && projection.reversibility.is_some();
    let empty_candidate_shape = projection.candidate_envelope_digest.is_none()
        && projection.candidate_content_digest.is_none()
        && projection.candidate_byte_count.is_none()
        && projection.reversibility.is_none();
    if (projection.candidate_offered && !complete_candidate_shape)
        || (!projection.candidate_offered && !empty_candidate_shape)
    {
        return Err(SinkError::PostToolUsePlanInvariant {
            invariant: "candidate_shape",
        });
    }
    if (!projection.candidate_offered && projection.transform_count != 0)
        || projection.transform_count > MAX_TRANSFORM_CHAIN_ITEMS as u64
    {
        return Err(SinkError::PostToolUsePlanInvariant {
            invariant: "absent_candidate_has_no_transforms",
        });
    }
    if projection.candidate_offered
        && (projection.invocation.disposition
            != aw_contracts::provider::ProviderDisposition::Produced
            || projection.invocation.output_digest.as_ref()
                != projection.candidate_envelope_digest.as_ref())
    {
        return Err(SinkError::PostToolUsePlanInvariant {
            invariant: "candidate_matches_invocation_output",
        });
    }
    if !projection.candidate_offered
        && projection.invocation.disposition == ProviderDisposition::Produced
    {
        return Err(SinkError::PostToolUsePlanInvariant {
            invariant: "produced_projection_requires_candidate",
        });
    }
    Ok(())
}

fn validate_pre_tool_use_gate(
    gate: &PreToolUseGateBody,
    scope: Option<&LedgerTraceScope>,
) -> Result<(), SinkError> {
    let scope = boundary_scope(scope).ok_or(SinkError::PreToolUseGateInvariant {
        invariant: "header_scope",
    })?;
    if let Some(invocation) = &gate.invocation {
        if !invocation_contract_is(
            invocation,
            SECURITY_COMMAND_INSPECT_CAPABILITY_ID,
            SECURITY_COMMAND_INSPECT_CAPABILITY_VERSION,
            SECURITY_COMMAND_INSPECT_INPUT_SCHEMA_ID,
            SECURITY_COMMAND_INSPECT_OUTPUT_SCHEMA_ID,
        ) || validate_invocation_ref(invocation).is_err()
            || !invocation_matches_scope(invocation, scope)
        {
            return Err(SinkError::PreToolUseGateInvariant {
                invariant: "invocation",
            });
        }
    }

    match gate.degradation {
        None => {
            let Some(invocation) = &gate.invocation else {
                return Err(SinkError::PreToolUseGateInvariant {
                    invariant: "settled_gate_invocation",
                });
            };
            if invocation.disposition != ProviderDisposition::Produced
                || matches!(gate.gate, ToolCallGate::Ask | ToolCallGate::NotMediated)
                || (gate.gate == ToolCallGate::Allow && !gate.reasons.is_empty())
                || (matches!(gate.gate, ToolCallGate::Warn | ToolCallGate::Block)
                    && gate.reasons.is_empty())
            {
                return Err(SinkError::PreToolUseGateInvariant {
                    invariant: "settled_gate_shape",
                });
            }
        }
        Some(GateDegradation::NoImplementation) => {
            if gate.gate != ToolCallGate::NotMediated
                || gate.invocation.is_some()
                || !gate.reasons.is_empty()
            {
                return Err(SinkError::PreToolUseGateInvariant {
                    invariant: "unmediated_gate_shape",
                });
            }
        }
        Some(
            GateDegradation::AmbiguousRoute
            | GateDegradation::ControlsNotEnforced
            | GateDegradation::HostFailure,
        ) => {
            if !matches!(gate.gate, ToolCallGate::Ask | ToolCallGate::Block)
                || !gate.reasons.is_empty()
                || gate.invocation.is_some()
            {
                return Err(SinkError::PreToolUseGateInvariant {
                    invariant: "degraded_gate_shape",
                });
            }
        }
        Some(GateDegradation::NotProduced) => {
            if !matches!(gate.gate, ToolCallGate::Ask | ToolCallGate::Block)
                || !gate.reasons.is_empty()
                || gate.invocation.as_ref().is_none_or(|invocation| {
                    invocation.disposition == ProviderDisposition::Produced
                        || invocation
                            .error_code
                            .as_ref()
                            .is_some_and(|code| code.as_str() == "provider_invalid_response")
                })
            {
                return Err(SinkError::PreToolUseGateInvariant {
                    invariant: "not_produced_gate_shape",
                });
            }
        }
        Some(GateDegradation::InvalidOutput) => {
            if !matches!(gate.gate, ToolCallGate::Ask | ToolCallGate::Block)
                || !gate.reasons.is_empty()
                || gate.invocation.as_ref().is_none_or(|invocation| {
                    invocation.disposition != ProviderDisposition::Produced
                        && invocation
                            .error_code
                            .as_ref()
                            .is_none_or(|code| code.as_str() != "provider_invalid_response")
                })
            {
                return Err(SinkError::PreToolUseGateInvariant {
                    invariant: "invalid_output_gate_shape",
                });
            }
        }
        Some(GateDegradation::LedgerUnavailable) => {
            return Err(SinkError::PreToolUseGateInvariant {
                invariant: "ledger_unavailable_has_no_writer_protocol",
            });
        }
    }
    Ok(())
}

fn boundary_scope(scope: Option<&LedgerTraceScope>) -> Option<&LedgerTraceScope> {
    scope.filter(|scope| scope.tool_use_id.is_some() && scope.invocation_id.is_none())
}

fn invocation_matches_scope(invocation: &LedgerInvocationRef, scope: &LedgerTraceScope) -> bool {
    invocation.attempt_id == scope.attempt_id && invocation.tool_use_id == scope.tool_use_id
}

fn validate_invocation_ref(invocation: &LedgerInvocationRef) -> Result<(), ()> {
    let output_identity_complete =
        invocation.output_schema.is_some() && invocation.output_digest.is_some();
    let output_identity_empty =
        invocation.output_schema.is_none() && invocation.output_digest.is_none();
    let requires_error = matches!(
        invocation.disposition,
        ProviderDisposition::Denied | ProviderDisposition::Failed | ProviderDisposition::Uncertain
    );
    if invocation.completed_at_ms < invocation.started_at_ms
        || (invocation.disposition == ProviderDisposition::Produced && !output_identity_complete)
        || (invocation.disposition != ProviderDisposition::Produced && !output_identity_empty)
        || requires_error != invocation.error_code.is_some()
    {
        return Err(());
    }
    Ok(())
}

fn observation_slot(capability: &VersionedSchema) -> Option<usize> {
    if schema_is(
        capability,
        SECURITY_CONTENT_INSPECT_CAPABILITY_ID,
        SECURITY_CONTENT_INSPECT_CAPABILITY_VERSION,
    ) {
        Some(0)
    } else if schema_is(
        capability,
        SECURITY_CODE_INSPECT_CAPABILITY_ID,
        SECURITY_CODE_INSPECT_CAPABILITY_VERSION,
    ) {
        Some(1)
    } else {
        None
    }
}

fn schema_is(schema: &VersionedSchema, id: &str, version: u16) -> bool {
    schema.id.as_str() == id && schema.version == version
}

fn invocation_contract_is(
    invocation: &LedgerInvocationRef,
    capability_id: &str,
    version: u16,
    input_schema_id: &str,
    output_schema_id: &str,
) -> bool {
    schema_is(&invocation.capability, capability_id, version)
        && schema_is(&invocation.input_schema, input_schema_id, version)
        && invocation
            .output_schema
            .as_ref()
            .is_none_or(|schema| schema_is(schema, output_schema_id, version))
}

struct ObservationContract {
    capability_id: &'static str,
    version: u16,
    input_schema_id: &'static str,
    output_schema_id: &'static str,
}

fn observation_contract(capability: &VersionedSchema) -> Option<ObservationContract> {
    if schema_is(
        capability,
        SECURITY_CONTENT_INSPECT_CAPABILITY_ID,
        SECURITY_CONTENT_INSPECT_CAPABILITY_VERSION,
    ) {
        Some(ObservationContract {
            capability_id: SECURITY_CONTENT_INSPECT_CAPABILITY_ID,
            version: SECURITY_CONTENT_INSPECT_CAPABILITY_VERSION,
            input_schema_id: SECURITY_CONTENT_INSPECT_INPUT_SCHEMA_ID,
            output_schema_id: SECURITY_CONTENT_INSPECT_OUTPUT_SCHEMA_ID,
        })
    } else if schema_is(
        capability,
        SECURITY_CODE_INSPECT_CAPABILITY_ID,
        SECURITY_CODE_INSPECT_CAPABILITY_VERSION,
    ) {
        Some(ObservationContract {
            capability_id: SECURITY_CODE_INSPECT_CAPABILITY_ID,
            version: SECURITY_CODE_INSPECT_CAPABILITY_VERSION,
            input_schema_id: SECURITY_CODE_INSPECT_INPUT_SCHEMA_ID,
            output_schema_id: SECURITY_CODE_INSPECT_OUTPUT_SCHEMA_ID,
        })
    } else {
        None
    }
}

fn scan_coverage_is_valid(scanned_bytes: u64, truncated: bool, input_bytes: u64) -> bool {
    if truncated {
        scanned_bytes > 0 && scanned_bytes < input_bytes
    } else {
        scanned_bytes == input_bytes
    }
}

fn validate_context_adoption(
    store: &LedgerStore,
    adoption: &ContextAdoptionBody,
    scope: Option<&LedgerTraceScope>,
) -> Result<(), SinkError> {
    match (adoption.decision, adoption.reason) {
        (ContextAdoptionDecision::Adopted, ContextAdoptionReason::LosslessCandidate) => {
            if adoption.candidate_envelope_digest.is_none() {
                return Err(SinkError::ContextAdoptionInvariant {
                    invariant: "adopted_requires_candidate_digest",
                });
            }
            if adoption.effective_byte_count == 0 {
                return Err(SinkError::ContextAdoptionInvariant {
                    invariant: "adopted_candidate_non_empty",
                });
            }
        }
        (ContextAdoptionDecision::Preserved, ContextAdoptionReason::NoCandidate) => {
            if adoption.candidate_envelope_digest.is_some() {
                return Err(SinkError::ContextAdoptionInvariant {
                    invariant: "preserved_forbids_candidate_digest",
                });
            }
            if adoption.effective_digest != adoption.source_digest {
                return Err(SinkError::ContextAdoptionInvariant {
                    invariant: "preserved_requires_source_digest",
                });
            }
        }
        (
            ContextAdoptionDecision::Preserved,
            ContextAdoptionReason::EmptyCandidate | ContextAdoptionReason::CandidateNotLossless,
        ) => {
            if adoption.candidate_envelope_digest.is_none() {
                return Err(SinkError::ContextAdoptionInvariant {
                    invariant: "preserved_candidate_requires_candidate_digest",
                });
            }
            if adoption.effective_digest != adoption.source_digest {
                return Err(SinkError::ContextAdoptionInvariant {
                    invariant: "preserved_requires_source_digest",
                });
            }
        }
        _ => {
            return Err(SinkError::ContextAdoptionInvariant {
                invariant: "decision_reason_pair",
            });
        }
    }
    if adoption.provider_invocations.is_empty() {
        return Err(SinkError::ContextAdoptionInvariant {
            invariant: "provider_invocations_non_empty",
        });
    }

    let plan_record = store
        .record_by_id(&adoption.plan_event_id)?
        .ok_or(SinkError::ContextAdoptionPlanMissing)?;
    if plan_record.header.kind != LedgerEventKind::PostToolUsePlan
        || plan_record.header.schema != LEDGER_POST_TOOL_USE_PLAN_SCHEMA
    {
        return Err(SinkError::ContextAdoptionPlanMismatch { field: "kind" });
    }
    let plan = serde_json::from_slice::<PostToolUsePlanBody>(
        &store.record_body_bytes(&adoption.plan_event_id)?,
    )
    .map_err(|source| SinkError::InvalidBody {
        kind: LedgerEventKind::PostToolUsePlan,
        source,
    })?;
    if plan.source_artifact_id != adoption.source_artifact_id {
        return Err(SinkError::ContextAdoptionPlanMismatch {
            field: "source_artifact_id",
        });
    }
    if plan.source_digest != adoption.source_digest {
        return Err(SinkError::ContextAdoptionPlanMismatch {
            field: "source_digest",
        });
    }
    if plan.projection.candidate_envelope_digest != adoption.candidate_envelope_digest {
        return Err(SinkError::ContextAdoptionPlanMismatch {
            field: "candidate_envelope_digest",
        });
    }
    match adoption.reason {
        ContextAdoptionReason::LosslessCandidate => {
            if plan.projection.reversibility
                != Some(aw_contracts::context::ContextReversibility::Lossless)
            {
                return Err(SinkError::ContextAdoptionPlanMismatch {
                    field: "reversibility",
                });
            }
            if plan.projection.candidate_byte_count != Some(adoption.effective_byte_count) {
                return Err(SinkError::ContextAdoptionPlanMismatch {
                    field: "candidate_byte_count",
                });
            }
            if plan.projection.candidate_content_digest.as_ref() != Some(&adoption.effective_digest)
            {
                return Err(SinkError::ContextAdoptionPlanMismatch {
                    field: "candidate_content_digest",
                });
            }
        }
        ContextAdoptionReason::EmptyCandidate => {
            if plan.projection.reversibility.is_none()
                || plan.projection.candidate_byte_count != Some(0)
            {
                return Err(SinkError::ContextAdoptionPlanMismatch {
                    field: "empty_candidate_shape",
                });
            }
        }
        ContextAdoptionReason::CandidateNotLossless => {
            if plan.projection.reversibility.is_none()
                || plan.projection.reversibility
                    == Some(aw_contracts::context::ContextReversibility::Lossless)
                || plan.projection.candidate_byte_count == Some(0)
            {
                return Err(SinkError::ContextAdoptionPlanMismatch {
                    field: "non_lossless_candidate_shape",
                });
            }
        }
        ContextAdoptionReason::NoCandidate => {
            if plan.projection.reversibility.is_some()
                || plan.projection.candidate_byte_count.is_some()
            {
                return Err(SinkError::ContextAdoptionPlanMismatch {
                    field: "absent_candidate_shape",
                });
            }
        }
    }
    if adoption.decision == ContextAdoptionDecision::Preserved
        && adoption.effective_byte_count != plan.source_byte_count
    {
        return Err(SinkError::ContextAdoptionPlanMismatch {
            field: "source_byte_count",
        });
    }

    let expected_invocations = plan
        .observations
        .iter()
        .map(|observation| &observation.invocation)
        .chain(
            plan.observation_gaps
                .iter()
                .filter_map(|gap| gap.invocation.as_ref()),
        )
        .chain(std::iter::once(&plan.projection.invocation))
        .cloned()
        .collect::<Vec<LedgerInvocationRef>>();
    if expected_invocations != adoption.provider_invocations {
        return Err(SinkError::ContextAdoptionPlanMismatch {
            field: "provider_invocations",
        });
    }

    if scope.and_then(|scope| scope.tool_use_id.as_ref()).is_none()
        || scope != plan_record.header.scope.as_ref()
    {
        return Err(SinkError::ContextAdoptionPlanMismatch { field: "scope" });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aw_contracts::common::{BoundedName, Digest};
    use aw_contracts::context::ContextReversibility;
    use aw_contracts::ids::{ArtifactId, AttemptId, ProviderInvocationId, ToolUseId};
    use aw_contracts::ledger::{
        ContextAdoptionBody, ContextAdoptionDecision, ContextAdoptionReason, LedgerObservation,
        LedgerObservationGap, LedgerProjectionOutcome, LedgerRuleFinding, PostToolUsePlanBody,
    };
    use aw_contracts::provider::{ProviderDisposition, VersionedSchema};
    use aw_contracts::security::{
        SecurityFindingCategory, SecurityFindingConfidence, SecurityFindingSeverity,
        SecurityInspectionVerdict,
    };
    use serde_json::json;
    use tempfile::tempdir;

    fn open_sink() -> (LedgerSink, tempfile::TempDir) {
        let dir = tempdir().expect("temp dir");
        let store = LedgerStore::open(dir.path()).expect("store opens");
        (LedgerSink::new(store), dir)
    }

    fn clean_body() -> Value {
        json!({
            "command_digest": empty_digest(),
            "command_byte_count": 0,
            "gate": "not_mediated",
            "reasons": [],
            "degradation": "no_implementation"
        })
    }

    fn empty_digest() -> Digest {
        Digest::parse("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855").unwrap()
    }

    fn zero_digest() -> Digest {
        Digest::parse("0000000000000000000000000000000000000000000000000000000000000000").unwrap()
    }

    fn tool_scope() -> LedgerTraceScope {
        LedgerTraceScope {
            attempt_id: None,
            tool_use_id: Some(ToolUseId::new()),
            invocation_id: None,
        }
    }

    fn versioned(id: &str) -> VersionedSchema {
        VersionedSchema {
            id: BoundedName::new(id).unwrap(),
            version: 1,
        }
    }

    fn invocation() -> LedgerInvocationRef {
        LedgerInvocationRef {
            invocation_id: ProviderInvocationId::new(),
            provider_id: BoundedName::new("projection-fixture").unwrap(),
            provider_version: BoundedName::new("1.0.0").unwrap(),
            manifest_digest: empty_digest(),
            capability: versioned("context.projection.prepare"),
            input_schema: versioned("context.projection.prepare.input"),
            input_digest: empty_digest(),
            attempt_id: None,
            tool_use_id: None,
            disposition: ProviderDisposition::Produced,
            output_schema: Some(versioned("context.projection.prepare.output")),
            output_digest: Some(empty_digest()),
            error_code: None,
            started_at_ms: 10,
            completed_at_ms: 11,
        }
    }

    fn content_observation(tool_use_id: &ToolUseId, provider_id: &str) -> LedgerObservation {
        let capability = versioned(SECURITY_CONTENT_INSPECT_CAPABILITY_ID);
        let mut invocation = invocation();
        invocation.provider_id = BoundedName::new(provider_id).unwrap();
        invocation.capability = capability.clone();
        invocation.input_schema = versioned("security.content.inspect.input");
        invocation.output_schema = Some(versioned("security.content.inspect.output"));
        invocation.tool_use_id = Some(tool_use_id.clone());
        LedgerObservation {
            capability,
            verdict: SecurityInspectionVerdict::Clean,
            findings: Vec::new(),
            scanned_bytes: 6,
            truncated: false,
            language_detected: None,
            invocation,
        }
    }

    fn append_plan(
        sink: &mut LedgerSink,
        tool_use_id: &ToolUseId,
        candidate_offered: bool,
    ) -> (crate::AdmittedRecord, PostToolUsePlanBody) {
        append_plan_with_reversibility(
            sink,
            tool_use_id,
            candidate_offered.then_some(ContextReversibility::Lossless),
            17,
        )
    }

    fn append_plan_with_reversibility(
        sink: &mut LedgerSink,
        tool_use_id: &ToolUseId,
        reversibility: Option<ContextReversibility>,
        candidate_byte_count: u64,
    ) -> (crate::AdmittedRecord, PostToolUsePlanBody) {
        let candidate_offered = reversibility.is_some();
        let mut invocation = invocation();
        invocation.tool_use_id = Some(tool_use_id.clone());
        if !candidate_offered {
            invocation.disposition = ProviderDisposition::Bypassed;
            invocation.output_schema = None;
            invocation.output_digest = None;
        }
        let body = PostToolUsePlanBody {
            source_artifact_id: ArtifactId::new(),
            source_digest: Digest::sha256(b"source"),
            source_byte_count: 6,
            observations: Vec::new(),
            observation_gaps: vec![
                LedgerObservationGap {
                    capability: versioned(SECURITY_CONTENT_INSPECT_CAPABILITY_ID),
                    reason: ObservationGapReason::NoImplementation,
                    provider_id: None,
                    invocation: None,
                },
                LedgerObservationGap {
                    capability: versioned(SECURITY_CODE_INSPECT_CAPABILITY_ID),
                    reason: ObservationGapReason::NoImplementation,
                    provider_id: None,
                    invocation: None,
                },
            ],
            projection: LedgerProjectionOutcome {
                candidate_offered,
                candidate_envelope_digest: candidate_offered.then(|| {
                    invocation
                        .output_digest
                        .clone()
                        .expect("fixture output digest")
                }),
                candidate_content_digest: candidate_offered.then(|| {
                    Digest::sha256(&vec![b'x'; usize::try_from(candidate_byte_count).unwrap()])
                }),
                candidate_byte_count: candidate_offered.then_some(candidate_byte_count),
                transform_count: u64::from(candidate_offered),
                reversibility,
                invocation,
            },
        };
        let record = sink
            .record(
                LedgerEventKind::PostToolUsePlan,
                LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
                serde_json::to_value(&body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id.clone()),
                    invocation_id: None,
                }),
            )
            .unwrap();
        (record, body)
    }

    fn adoption(plan: &crate::AdmittedRecord, body: &PostToolUsePlanBody) -> ContextAdoptionBody {
        ContextAdoptionBody {
            plan_event_id: plan.header.id.clone(),
            source_artifact_id: body.source_artifact_id.clone(),
            source_digest: body.source_digest.clone(),
            candidate_envelope_digest: body.projection.candidate_envelope_digest.clone(),
            effective_digest: body
                .projection
                .candidate_content_digest
                .clone()
                .unwrap_or_else(|| body.source_digest.clone()),
            effective_byte_count: body.projection.candidate_byte_count.unwrap_or(0),
            decision: ContextAdoptionDecision::Adopted,
            reason: ContextAdoptionReason::LosslessCandidate,
            provider_invocations: vec![body.projection.invocation.clone()],
        }
    }

    #[test]
    fn record_produces_a_genesis_event() {
        let (mut sink, _dir) = open_sink();
        let admitted = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
                clean_body(),
                Some(&tool_scope()),
            )
            .expect("genesis recorded");
        assert_eq!(admitted.header.sequence, 0);
        assert_eq!(admitted.header.kind, LedgerEventKind::PreToolUseGate);
        assert!(admitted.header.parent.is_none());
    }

    #[test]
    fn successive_records_form_a_continuous_chain() {
        let (mut sink, _dir) = open_sink();
        let first = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
                clean_body(),
                Some(&tool_scope()),
            )
            .expect("first recorded");
        let second = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
                clean_body(),
                Some(&tool_scope()),
            )
            .expect("second recorded");

        assert_eq!(second.header.sequence, 1);
        let parent = second.header.parent.as_ref().expect("parent present");
        assert_eq!(parent.id, first.header.id);
        assert_eq!(parent.digest, first.record_digest);
    }

    #[test]
    fn content_freedom_is_enforced_at_the_sink() {
        let (mut sink, _dir) = open_sink();
        let bad_body = json!({"command": "rm -rf /"});
        let error = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                "aw.ledger.pre_tool_use_gate/v1",
                bad_body,
                None,
            )
            .unwrap_err();
        assert!(
            matches!(
                error,
                SinkError::Admission(AdmissionError::ContentForbidden { .. })
            ),
            "expected ContentForbidden, got {error:?}"
        );
    }

    #[test]
    fn scope_travels_with_the_record() {
        let (mut sink, _dir) = open_sink();
        let attempt_id = AttemptId::new();
        let scope = LedgerTraceScope {
            attempt_id: Some(attempt_id.clone()),
            tool_use_id: Some(ToolUseId::new()),
            invocation_id: None,
        };
        sink.record(
            LedgerEventKind::PreToolUseGate,
            LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
            clean_body(),
            Some(&scope),
        )
        .expect("recorded with scope");

        // Verify the tip advanced.
        assert_eq!(sink.tip().sequence, 0);
        assert!(sink.tip().id.is_some());
    }

    #[test]
    fn writer_rejects_kind_schema_mismatch() {
        let (mut sink, _dir) = open_sink();
        let error = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
                clean_body(),
                None,
            )
            .unwrap_err();
        assert!(matches!(error, SinkError::SchemaMismatch { .. }));
        assert!(sink.tip().id.is_none());
    }

    #[test]
    fn writer_rejects_unknown_body_fields() {
        let (mut sink, _dir) = open_sink();
        let mut body = clean_body();
        body.as_object_mut()
            .expect("fixture is an object")
            .insert("note".to_owned(), json!("provider text"));
        let error = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
                body,
                Some(&tool_scope()),
            )
            .unwrap_err();
        assert!(matches!(error, SinkError::InvalidBody { .. }));
        assert!(sink.tip().id.is_none());
    }

    #[test]
    fn writer_rejects_a_clean_observation_with_findings() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (_record, mut body) = append_plan(&mut sink, &tool_use_id, true);
        let capability = versioned(SECURITY_CONTENT_INSPECT_CAPABILITY_ID);
        let mut observation_invocation = invocation();
        observation_invocation.capability = capability.clone();
        observation_invocation.input_schema = versioned("security.content.inspect.input");
        observation_invocation.output_schema = Some(versioned("security.content.inspect.output"));
        observation_invocation.tool_use_id = Some(tool_use_id.clone());
        body.observation_gaps
            .retain(|gap| gap.capability.id.as_str() != SECURITY_CONTENT_INSPECT_CAPABILITY_ID);
        body.observations.push(LedgerObservation {
            capability,
            verdict: SecurityInspectionVerdict::Clean,
            findings: vec![LedgerRuleFinding {
                rule_id_digest: empty_digest(),
                category: SecurityFindingCategory::Credential,
                severity: SecurityFindingSeverity::High,
                confidence: SecurityFindingConfidence::High,
                count: 1,
            }],
            scanned_bytes: body.source_byte_count,
            truncated: false,
            language_detected: None,
            invocation: observation_invocation,
        });

        let error = sink
            .record(
                LedgerEventKind::PostToolUsePlan,
                LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
                serde_json::to_value(body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::PostToolUsePlanInvariant {
                invariant: "clean_observation"
            }
        ));
    }

    #[test]
    fn writer_requires_every_planned_observation_to_be_accounted_for() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (_record, mut body) = append_plan(&mut sink, &tool_use_id, true);
        body.observation_gaps
            .retain(|gap| gap.capability.id.as_str() != SECURITY_CODE_INSPECT_CAPABILITY_ID);

        let error = sink
            .record(
                LedgerEventKind::PostToolUsePlan,
                LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
                serde_json::to_value(body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::PostToolUsePlanInvariant {
                invariant: "observation_plan_coverage"
            }
        ));
    }

    #[test]
    fn writer_rejects_a_route_gap_alongside_a_provider_fact() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (_record, mut body) = append_plan(&mut sink, &tool_use_id, true);
        body.observations
            .push(content_observation(&tool_use_id, "scanner-a"));

        let error = sink
            .record(
                LedgerEventKind::PostToolUsePlan,
                LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
                serde_json::to_value(body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::PostToolUsePlanInvariant {
                invariant: "gap_invocation"
            }
        ));
    }

    #[test]
    fn writer_rejects_duplicate_provider_facts_for_one_capability() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (_record, mut body) = append_plan(&mut sink, &tool_use_id, true);
        body.observation_gaps
            .retain(|gap| gap.capability.id.as_str() != SECURITY_CONTENT_INSPECT_CAPABILITY_ID);
        body.observations
            .push(content_observation(&tool_use_id, "scanner-a"));
        body.observations
            .push(content_observation(&tool_use_id, "scanner-a"));

        let error = sink
            .record(
                LedgerEventKind::PostToolUsePlan,
                LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
                serde_json::to_value(body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::PostToolUsePlanInvariant {
                invariant: "observation_plan_uniqueness"
            }
        ));
    }

    #[test]
    fn writer_rejects_an_invocation_from_another_tool_scope() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (_record, mut body) = append_plan(&mut sink, &tool_use_id, true);
        body.projection.invocation.tool_use_id = Some(ToolUseId::new());

        let error = sink
            .record(
                LedgerEventKind::PostToolUsePlan,
                LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
                serde_json::to_value(body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::PostToolUsePlanInvariant {
                invariant: "projection_invocation"
            }
        ));
    }

    #[test]
    fn writer_rejects_a_gap_that_conflicts_with_its_disposition() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (_record, mut body) = append_plan(&mut sink, &tool_use_id, true);
        let gap = body
            .observation_gaps
            .iter_mut()
            .find(|gap| gap.capability.id.as_str() == SECURITY_CONTENT_INSPECT_CAPABILITY_ID)
            .expect("content gap exists");
        let observation = content_observation(&tool_use_id, "scanner-a");
        gap.reason = ObservationGapReason::NotProduced;
        gap.provider_id = Some(observation.invocation.provider_id.clone());
        gap.invocation = Some(observation.invocation);

        let error = sink
            .record(
                LedgerEventKind::PostToolUsePlan,
                LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
                serde_json::to_value(body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::PostToolUsePlanInvariant {
                invariant: "gap_invocation"
            }
        ));
    }

    #[test]
    fn writer_rejects_a_gate_that_conflicts_with_its_degradation() {
        let (mut sink, _dir) = open_sink();
        let mut body = clean_body();
        body["gate"] = json!("allow");

        let error = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
                body,
                Some(&tool_scope()),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::PreToolUseGateInvariant {
                invariant: "unmediated_gate_shape"
            }
        ));
    }

    #[test]
    fn writer_rejects_taxonomy_without_an_implemented_contract() {
        let (mut sink, _dir) = open_sink();
        let error = sink
            .record(
                LedgerEventKind::EvidenceStored,
                "aw.ledger.evidence_stored/v1",
                clean_body(),
                None,
            )
            .unwrap_err();
        assert!(matches!(error, SinkError::UnsupportedEventKind { .. }));
        assert!(sink.tip().id.is_none());
    }

    #[test]
    fn context_adoption_is_bound_to_its_typed_plan() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (plan_record, plan_body) = append_plan(&mut sink, &tool_use_id, true);
        let body = adoption(&plan_record, &plan_body);

        let adopted = sink
            .record(
                LedgerEventKind::ContextAdoption,
                LEDGER_CONTEXT_ADOPTION_SCHEMA,
                serde_json::to_value(&body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .expect("adoption matches its plan");

        assert_eq!(adopted.header.sequence, 1);
        let encoded = serde_json::to_string(&adopted.body).unwrap();
        assert!(!encoded.contains("content"));
        assert!(!encoded.contains("tool_response"));
    }

    #[test]
    fn context_adoption_rejects_a_mismatched_source() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (plan_record, plan_body) = append_plan(&mut sink, &tool_use_id, true);
        let mut body = adoption(&plan_record, &plan_body);
        body.source_artifact_id = ArtifactId::new();

        let error = sink
            .record(
                LedgerEventKind::ContextAdoption,
                LEDGER_CONTEXT_ADOPTION_SCHEMA,
                serde_json::to_value(&body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::ContextAdoptionPlanMismatch {
                field: "source_artifact_id"
            }
        ));
        assert_eq!(sink.tip().id, Some(&plan_record.header.id));
    }

    #[test]
    fn context_adoption_rejects_another_attempt_scope() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (plan_record, plan_body) = append_plan(&mut sink, &tool_use_id, true);
        let body = adoption(&plan_record, &plan_body);

        let error = sink
            .record(
                LedgerEventKind::ContextAdoption,
                LEDGER_CONTEXT_ADOPTION_SCHEMA,
                serde_json::to_value(body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: Some(AttemptId::new()),
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::ContextAdoptionPlanMismatch { field: "scope" }
        ));
    }

    #[test]
    fn writer_rejects_a_candidate_not_bound_to_the_invocation_output() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (_record, mut body) = append_plan(&mut sink, &tool_use_id, true);
        body.projection.candidate_envelope_digest = Some(zero_digest());

        let error = sink
            .record(
                LedgerEventKind::PostToolUsePlan,
                LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
                serde_json::to_value(body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::PostToolUsePlanInvariant {
                invariant: "candidate_matches_invocation_output"
            }
        ));
    }

    #[test]
    fn adoption_rejects_an_envelope_digest_from_another_candidate() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (plan_record, plan_body) = append_plan(&mut sink, &tool_use_id, true);
        let mut body = adoption(&plan_record, &plan_body);
        body.candidate_envelope_digest = Some(zero_digest());

        let error = sink
            .record(
                LedgerEventKind::ContextAdoption,
                LEDGER_CONTEXT_ADOPTION_SCHEMA,
                serde_json::to_value(body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::ContextAdoptionPlanMismatch {
                field: "candidate_envelope_digest"
            }
        ));
    }

    #[test]
    fn adoption_rejects_bytes_not_bound_to_candidate_content() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (plan_record, plan_body) = append_plan(&mut sink, &tool_use_id, true);
        let mut body = adoption(&plan_record, &plan_body);
        body.effective_digest = zero_digest();

        let error = sink
            .record(
                LedgerEventKind::ContextAdoption,
                LEDGER_CONTEXT_ADOPTION_SCHEMA,
                serde_json::to_value(body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::ContextAdoptionPlanMismatch {
                field: "candidate_content_digest"
            }
        ));
    }

    #[test]
    fn context_adoption_rejects_unknown_fields() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (plan_record, plan_body) = append_plan(&mut sink, &tool_use_id, true);
        let mut value = serde_json::to_value(adoption(&plan_record, &plan_body)).unwrap();
        value["provider_note"] = json!("free text");

        let error = sink
            .record(
                LedgerEventKind::ContextAdoption,
                LEDGER_CONTEXT_ADOPTION_SCHEMA,
                value,
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(error, SinkError::InvalidBody { .. }));
        assert_eq!(sink.tip().id, Some(&plan_record.header.id));
    }

    #[test]
    fn preserved_context_commits_the_source_digest() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (plan_record, plan_body) = append_plan(&mut sink, &tool_use_id, false);
        let body = ContextAdoptionBody {
            plan_event_id: plan_record.header.id,
            source_artifact_id: plan_body.source_artifact_id,
            source_digest: plan_body.source_digest.clone(),
            candidate_envelope_digest: None,
            effective_digest: plan_body.source_digest,
            effective_byte_count: plan_body.source_byte_count,
            decision: ContextAdoptionDecision::Preserved,
            reason: ContextAdoptionReason::NoCandidate,
            provider_invocations: vec![plan_body.projection.invocation],
        };

        sink.record(
            LedgerEventKind::ContextAdoption,
            LEDGER_CONTEXT_ADOPTION_SCHEMA,
            serde_json::to_value(body).unwrap(),
            Some(&LedgerTraceScope {
                attempt_id: None,
                tool_use_id: Some(tool_use_id),
                invocation_id: None,
            }),
        )
        .expect("no-candidate plan may preserve even an empty source");
    }

    #[test]
    fn preserved_context_rejects_a_false_source_byte_count() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (plan_record, plan_body) = append_plan(&mut sink, &tool_use_id, false);
        let body = ContextAdoptionBody {
            plan_event_id: plan_record.header.id,
            source_artifact_id: plan_body.source_artifact_id,
            source_digest: plan_body.source_digest.clone(),
            candidate_envelope_digest: None,
            effective_digest: plan_body.source_digest,
            effective_byte_count: plan_body.source_byte_count + 1,
            decision: ContextAdoptionDecision::Preserved,
            reason: ContextAdoptionReason::NoCandidate,
            provider_invocations: vec![plan_body.projection.invocation],
        };

        let error = sink
            .record(
                LedgerEventKind::ContextAdoption,
                LEDGER_CONTEXT_ADOPTION_SCHEMA,
                serde_json::to_value(body).unwrap(),
                Some(&LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                }),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            SinkError::ContextAdoptionPlanMismatch {
                field: "source_byte_count"
            }
        ));
    }

    #[test]
    fn non_lossless_offer_is_preserved_with_its_candidate_digest() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (plan_record, plan_body) = append_plan_with_reversibility(
            &mut sink,
            &tool_use_id,
            Some(ContextReversibility::Retrievable),
            17,
        );
        let mut body = adoption(&plan_record, &plan_body);
        body.effective_digest = body.source_digest.clone();
        body.effective_byte_count = plan_body.source_byte_count;
        body.decision = ContextAdoptionDecision::Preserved;
        body.reason = ContextAdoptionReason::CandidateNotLossless;

        sink.record(
            LedgerEventKind::ContextAdoption,
            LEDGER_CONTEXT_ADOPTION_SCHEMA,
            serde_json::to_value(body).unwrap(),
            Some(&LedgerTraceScope {
                attempt_id: None,
                tool_use_id: Some(tool_use_id),
                invocation_id: None,
            }),
        )
        .expect("a retrievable candidate is evidence, not effective bytes");
    }

    #[test]
    fn empty_offer_is_preserved_with_its_candidate_digest() {
        let (mut sink, _dir) = open_sink();
        let tool_use_id = ToolUseId::new();
        let (plan_record, plan_body) = append_plan_with_reversibility(
            &mut sink,
            &tool_use_id,
            Some(ContextReversibility::Lossless),
            0,
        );
        let mut body = adoption(&plan_record, &plan_body);
        body.effective_digest = body.source_digest.clone();
        body.effective_byte_count = plan_body.source_byte_count;
        body.decision = ContextAdoptionDecision::Preserved;
        body.reason = ContextAdoptionReason::EmptyCandidate;

        sink.record(
            LedgerEventKind::ContextAdoption,
            LEDGER_CONTEXT_ADOPTION_SCHEMA,
            serde_json::to_value(body).unwrap(),
            Some(&LedgerTraceScope {
                attempt_id: None,
                tool_use_id: Some(tool_use_id),
                invocation_id: None,
            }),
        )
        .expect("an empty candidate is evidence, not effective bytes");
    }
}
