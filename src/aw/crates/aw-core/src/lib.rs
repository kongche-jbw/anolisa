#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Core-owned execution context and Capability Plan policy.
//!
//! Agent Environments establish a stable execution context here and submit one
//! event without constructing Provider envelopes. Core decides which
//! Capabilities apply to that event, resolves every route before invoking any
//! implementation, and returns typed facts separately from the content-free
//! Provider receipts.

use std::num::TryFromIntError;
use std::time::{SystemTime, SystemTimeError, UNIX_EPOCH};

use aw_contracts::common::{BoundedName, BoundedStringError, Digest, DigestError, TargetRef};
use aw_contracts::context::{
    ContextArtifactOrigin, ContextContractBuildError, ContextProjectionCandidate,
    ToolResultSubmission,
};
use aw_contracts::ids::{
    ActorId, AgentSessionId, AgentWorkId, ArtifactId, AttemptId, EnvironmentId, ExecutionContextId,
    IdError, ToolUseId, TurnId,
};
use aw_contracts::provider::{ExecutionScope, ProviderDisposition, VersionedSchema};
use aw_contracts::security::{
    CodeInspection, CommandInspection, CommandVerdict, ContentInspection, GateDegradation,
    ObservationGapReason, PendingToolCallSubmission, SecurityBoundary, SecurityCodeLanguage,
    SecurityContractBuildError, SecurityOutputValidationError, ToolCallGate, MAX_GATE_REASONS,
    MAX_OBSERVATION_FINDINGS,
};
use aw_provider_host::{canonical_json_v1_bytes, ProviderCatalog, ProviderHostError};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use uuid::Uuid;

mod execute;
mod outcome;
mod plan;

pub use execute::CapabilityPreferences;
pub use outcome::{
    CapabilityObservation, ObservationGap, PreparedProjection, ToolCallDecision, ToolResultOutcome,
};

use execute::schema_label;
use plan::{PlanBoundary, StepKind};

const MAX_TRANSFORM_CHAIN_ITEMS: usize = 64;

/// Caller-owned identities used to establish one governed Agent execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContextSpec {
    /// Host or remote environment in which Agent work is taking place.
    pub target: TargetRef,
    /// Agent Environment presenting work to Core.
    pub environment_id: EnvironmentId,
    /// Actor identity asserted at the caller's Core trust boundary.
    ///
    /// A service boundary must authenticate this assertion before using it for
    /// authorization. An in-process adapter may use it only for correlation.
    pub actor_id: ActorId,
    /// Logical Agent session when the Environment can identify one.
    pub agent_session_id: Option<AgentSessionId>,
    /// Durable Work identity when the execution belongs to managed Work.
    pub work_id: Option<AgentWorkId>,
    /// Attempt identity when the execution belongs to managed Work.
    pub attempt_id: Option<AttemptId>,
    /// Existing Core context propagated by an Agent Environment hook.
    ///
    /// Omit this only when beginning a new execution; Core then allocates the
    /// identity returned by [`AgentExecutionContext::execution_context_id`].
    pub execution_context_id: Option<ExecutionContextId>,
}

/// Stable Core context shared by all observed work in one Agent execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentExecutionContext {
    target: TargetRef,
    environment_id: EnvironmentId,
    execution_context_id: ExecutionContextId,
    actor_id: ActorId,
    agent_session_id: Option<AgentSessionId>,
    work_id: Option<AgentWorkId>,
    attempt_id: Option<AttemptId>,
}

impl AgentExecutionContext {
    /// Returns the governed target associated with this execution.
    #[must_use]
    pub fn target(&self) -> &TargetRef {
        &self.target
    }

    /// Returns the Agent Environment that established the context.
    #[must_use]
    pub fn environment_id(&self) -> &EnvironmentId {
        &self.environment_id
    }

    /// Returns the Core identity propagated across hooks for this execution.
    #[must_use]
    pub fn execution_context_id(&self) -> &ExecutionContextId {
        &self.execution_context_id
    }

    /// Returns the caller-asserted actor associated with this execution.
    #[must_use]
    pub fn actor_id(&self) -> &ActorId {
        &self.actor_id
    }

    /// Returns the logical Agent session when one was supplied.
    #[must_use]
    pub fn agent_session_id(&self) -> Option<&AgentSessionId> {
        self.agent_session_id.as_ref()
    }

    /// Returns the durable Work identity when one was supplied.
    #[must_use]
    pub fn work_id(&self) -> Option<&AgentWorkId> {
        self.work_id.as_ref()
    }

    /// Returns the managed Work attempt when one was supplied.
    #[must_use]
    pub fn attempt_id(&self) -> Option<&AttemptId> {
        self.attempt_id.as_ref()
    }

    fn tool_scope(&self, turn_id: TurnId, tool_use_id: ToolUseId) -> ExecutionScope {
        ExecutionScope {
            target: self.target.clone(),
            environment_id: self.environment_id.clone(),
            execution_context_id: self.execution_context_id.clone(),
            actor_id: self.actor_id.clone(),
            agent_session_id: self.agent_session_id.clone(),
            work_id: self.work_id.clone(),
            attempt_id: self.attempt_id.clone(),
            turn_id: Some(turn_id),
            tool_use_id: Some(tool_use_id),
        }
    }
}

/// Gate resolution applied when an admitted Mediate implementation fails.
///
/// There is deliberately no fail-open value. A broken or absent scanner must
/// never silently approve a Tool Call, so the missing option is the control.
/// `Ask` is the default because escalating to a human is the branch a terminal
/// Agent Environment can express natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediationFailurePolicy {
    /// Escalate to a human decision at the Environment gate.
    Ask,
    /// Refuse the pending Tool Call.
    Block,
}

impl MediationFailurePolicy {
    fn gate(self) -> ToolCallGate {
        match self {
            Self::Ask => ToolCallGate::Ask,
            Self::Block => ToolCallGate::Block,
        }
    }
}

/// Core policy defaults applied to Provider invocations created for tool output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreConfig {
    /// Policy revision attributed to invocations created by this Core instance.
    pub policy_revision: u64,
    /// Maximum time Core grants one context-preparation Provider invocation.
    pub provider_wall_time_ms: u64,
    /// Maximum canonical output bytes Core accepts from one Provider invocation.
    pub provider_output_bytes: u64,
    /// Allow a Provider to read submitted content before OS controls enforce
    /// its declared network and filesystem permissions.
    ///
    /// Keep this disabled outside an explicit trusted-Provider PoC.
    pub allow_unenforced_providers: bool,
    /// Gate resolution applied when mediation produces no verdict.
    pub mediation_failure: MediationFailurePolicy,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            policy_revision: 1,
            provider_wall_time_ms: 2_000,
            provider_output_bytes: 64 * 1024 * 1024,
            allow_unenforced_providers: false,
            mediation_failure: MediationFailurePolicy::Ask,
        }
    }
}

/// Core policy owner over one admitted Provider catalog.
#[derive(Debug)]
pub struct Core {
    providers: ProviderCatalog,
    config: CoreConfig,
}

impl Core {
    /// Creates Core with production-safe default Provider invocation ceilings.
    #[must_use]
    pub fn new(providers: ProviderCatalog) -> Self {
        Self {
            providers,
            config: CoreConfig::default(),
        }
    }

    /// Creates Core with explicit invocation policy.
    ///
    /// # Errors
    ///
    /// Returns an error when either Provider resource ceiling is zero.
    pub fn with_config(providers: ProviderCatalog, config: CoreConfig) -> Result<Self, CoreError> {
        if config.provider_wall_time_ms == 0 || config.provider_output_bytes == 0 {
            return Err(CoreError::InvalidConfig);
        }
        Ok(Self { providers, config })
    }

    /// Establishes or resumes one stable Agent execution context.
    ///
    /// When `spec.execution_context_id` is absent, Core allocates a new
    /// identity. A propagated identity is retained exactly so several COSH or
    /// third-party hook calls remain attached to the same execution.
    ///
    /// # Errors
    ///
    /// Returns an error when an Attempt is supplied without its Work identity.
    pub fn establish_execution_context(
        &self,
        spec: SessionContextSpec,
    ) -> Result<AgentExecutionContext, CoreError> {
        if spec.attempt_id.is_some() && spec.work_id.is_none() {
            return Err(CoreError::AttemptWithoutWork);
        }
        Ok(AgentExecutionContext {
            target: spec.target,
            environment_id: spec.environment_id,
            execution_context_id: spec.execution_context_id.unwrap_or_default(),
            actor_id: spec.actor_id,
            agent_session_id: spec.agent_session_id,
            work_id: spec.work_id,
            attempt_id: spec.attempt_id,
        })
    }

    /// Runs the Core PostToolUse Capability Plan for one observed tool result.
    ///
    /// Core allocates the artifact and invocation identities, computes source
    /// and canonical input digests, binds the Tool Call scope, resolves every
    /// planned route against the current Runtime Capability Graph, and only then
    /// invokes each implementation under its deadline and output budget. A
    /// returned candidate remains advice; this method never replaces the Agent's
    /// original tool result.
    ///
    /// # Errors
    ///
    /// Returns an error for an incomplete tool scope, an inapplicable routing
    /// preference, no unique eligible implementation for a single-route step,
    /// malformed Provider output, clock failure, or Host error.
    pub fn observe_tool_result(
        &mut self,
        context: &AgentExecutionContext,
        turn_id: TurnId,
        tool_use_id: ToolUseId,
        submission: ToolResultSubmission,
        preferences: &CapabilityPreferences,
    ) -> Result<ToolResultOutcome, CoreError> {
        if context.agent_session_id.is_none() {
            return Err(CoreError::ToolResultWithoutAgentSession);
        }

        let source_digest = sha256_digest(submission.content.as_bytes())?;
        let artifact_id = context_artifact_id(
            context.execution_context_id(),
            &turn_id,
            &tool_use_id,
            &source_digest,
        )?;
        let scope = context.tool_scope(turn_id, tool_use_id.clone());
        let resolved = self.resolve_plan(plan::post_tool_use_plan()?, preferences)?;

        let mut projection = None;
        let mut observations = Vec::new();
        let mut observation_gaps = Vec::new();
        for resolved_step in resolved {
            let step = resolved_step.step;
            if let Some(route_gap) = resolved_step.gap {
                observation_gaps.push(ObservationGap {
                    capability: step.capability.clone(),
                    reason: route_gap.as_observation_reason(),
                    error: None,
                    receipt: None,
                });
                continue;
            }

            match step.kind {
                // The PostToolUse plan never contains a mediation step; the
                // command has already run by this boundary.
                StepKind::CommandInspection => return Err(CoreError::MissingResolvedRoute),
                StepKind::ContextProjection => {
                    let target = resolved_step
                        .targets
                        .into_iter()
                        .next()
                        .ok_or(CoreError::MissingResolvedRoute)?;
                    let input =
                        context_projection_input(&artifact_id, &source_digest, &submission)?;
                    let result = self.invoke_step(
                        &step,
                        PlanBoundary::PostToolUse,
                        target,
                        scope.clone(),
                        tool_use_id.as_str(),
                        input,
                    )?;
                    let candidate = self.accept_projection_candidate(
                        &step.output_contract.schema,
                        &result,
                        &artifact_id,
                        &source_digest,
                    )?;
                    projection = Some(PreparedProjection {
                        candidate,
                        receipt: result.receipt,
                    });
                }
                StepKind::ContentInspection | StepKind::CodeInspection => {
                    let input =
                        inspection_input(step.kind, &artifact_id, &source_digest, &submission)?;
                    for target in resolved_step.targets {
                        let invoked = self.invoke_step(
                            &step,
                            PlanBoundary::PostToolUse,
                            target,
                            scope.clone(),
                            tool_use_id.as_str(),
                            input.clone(),
                        );
                        match invoked {
                            Ok(result) => match self.accept_observation(&step, &result) {
                                Ok(Some(observation)) => observations.push(observation),
                                Ok(None) => observation_gaps.push(ObservationGap {
                                    capability: step.capability.clone(),
                                    reason: ObservationGapReason::NotProduced,
                                    error: result.receipt.error.clone(),
                                    receipt: Some(result.receipt),
                                }),
                                Err(_) => observation_gaps.push(ObservationGap {
                                    capability: step.capability.clone(),
                                    reason: ObservationGapReason::InvalidOutput,
                                    error: result.receipt.error.clone(),
                                    receipt: Some(result.receipt),
                                }),
                            },
                            // An Observe Capability only reports facts. It must
                            // never decide whether the Advise result reaches a
                            // model, so a Host failure degrades this step alone.
                            Err(_) => observation_gaps.push(ObservationGap {
                                capability: step.capability.clone(),
                                reason: ObservationGapReason::HostFailure,
                                error: None,
                                receipt: None,
                            }),
                        }
                    }
                }
            }
        }

        Ok(ToolResultOutcome {
            source_artifact_id: artifact_id,
            source_digest,
            projection: projection.ok_or(CoreError::MissingResolvedRoute)?,
            observations,
            observation_gaps,
        })
    }

    fn accept_observation(
        &self,
        step: &plan::CapabilityPlanStep,
        result: &aw_contracts::provider::ProviderInvocationResult,
    ) -> Result<Option<CapabilityObservation>, CoreError> {
        if result.receipt.disposition != ProviderDisposition::Produced {
            if receipt_reports_invalid_output(&result.receipt) {
                return Err(CoreError::ProviderOutputRejected);
            }
            return Ok(None);
        }
        let output = result
            .outcome
            .output
            .as_ref()
            .ok_or(CoreError::ProducedWithoutOutput)?;
        if output.schema != step.output_contract.schema {
            return Err(CoreError::UnexpectedOutputSchema {
                actual: schema_label(&output.schema),
            });
        }
        let observation = match step.kind {
            StepKind::ContentInspection => {
                let envelope: ContentInspectionOutput =
                    serde_json::from_value(output.body.clone())?;
                envelope.inspection.validate()?;
                CapabilityObservation {
                    capability: step.capability.clone(),
                    verdict: envelope.inspection.verdict,
                    findings: envelope.inspection.findings,
                    scanned_bytes: envelope.inspection.scanned_bytes,
                    truncated: envelope.inspection.truncated,
                    language_detected: None,
                    receipt: result.receipt.clone(),
                }
            }
            StepKind::CodeInspection => {
                let envelope: CodeInspectionOutput = serde_json::from_value(output.body.clone())?;
                envelope.inspection.validate()?;
                CapabilityObservation {
                    capability: step.capability.clone(),
                    verdict: envelope.inspection.verdict,
                    findings: envelope.inspection.findings,
                    scanned_bytes: envelope.inspection.scanned_bytes,
                    truncated: envelope.inspection.truncated,
                    language_detected: Some(envelope.inspection.language_detected),
                    receipt: result.receipt.clone(),
                }
            }
            // Neither shape reaches this acceptor: the caller matches on the
            // step kind and routes projection and mediation elsewhere.
            StepKind::ContextProjection | StepKind::CommandInspection => {
                return Err(CoreError::MissingResolvedRoute)
            }
        };
        if observation.findings.len() > MAX_OBSERVATION_FINDINGS {
            return Err(CoreError::TooManyFindings);
        }
        Ok(Some(observation))
    }

    /// Runs the Core PreToolUse Capability Plan and resolves the Tool Call gate.
    ///
    /// The command has not executed yet, so this is the only boundary where a
    /// Capability can still stop it. A `deny` verdict arrives as a `produced`
    /// disposition carrying a deny value; that is a different fact from a
    /// `denied` disposition, which would mean policy refused the invocation
    /// before it could have any effect.
    ///
    /// A gate is never silently opened. When no implementation verdict exists,
    /// Core reports `NotMediated` for an absent Capability and otherwise applies
    /// the configured mediation failure policy.
    ///
    /// # Errors
    ///
    /// Returns an error for an incomplete tool scope, an inapplicable routing
    /// preference, a duplicate route, or a clock failure. A Provider failure
    /// resolves the gate instead of failing.
    pub fn mediate_tool_call(
        &mut self,
        context: &AgentExecutionContext,
        turn_id: TurnId,
        tool_use_id: ToolUseId,
        submission: PendingToolCallSubmission,
        preferences: &CapabilityPreferences,
    ) -> Result<ToolCallDecision, CoreError> {
        if context.agent_session_id.is_none() {
            return Err(CoreError::ToolResultWithoutAgentSession);
        }

        let scope = context.tool_scope(turn_id, tool_use_id.clone());
        let resolved = self.resolve_plan(plan::pre_tool_use_plan()?, preferences)?;
        let step = resolved
            .into_iter()
            .next()
            .ok_or(CoreError::MissingResolvedRoute)?;

        if let Some(gap) = step.gap {
            let degradation = gap.as_gate_degradation();
            // An absent Capability is not a failure of anything: nothing was
            // installed to hold an opinion. A broken route is a failure, so it
            // takes the configured default instead.
            let gate = if degradation == GateDegradation::NoImplementation {
                ToolCallGate::NotMediated
            } else {
                self.config.mediation_failure.gate()
            };
            return Ok(ToolCallDecision {
                gate,
                reasons: Vec::new(),
                receipt: None,
                degradation: Some(degradation),
            });
        }

        let target = step
            .targets
            .into_iter()
            .next()
            .ok_or(CoreError::MissingResolvedRoute)?;
        let command_digest = sha256_digest(submission.command.as_bytes())?;
        let input = command_inspection_input(&command_digest, &submission)?;
        let result = match self.invoke_step(
            &step.step,
            PlanBoundary::PreToolUse,
            target,
            scope,
            tool_use_id.as_str(),
            input,
        ) {
            Ok(result) => result,
            Err(_) => {
                return Ok(ToolCallDecision {
                    gate: self.config.mediation_failure.gate(),
                    reasons: Vec::new(),
                    receipt: None,
                    degradation: Some(GateDegradation::HostFailure),
                })
            }
        };

        Ok(match self.accept_command_decision(&step.step, &result) {
            Ok(Some(inspection)) => ToolCallDecision {
                gate: match inspection.verdict {
                    CommandVerdict::Allow => ToolCallGate::Allow,
                    CommandVerdict::Warn => ToolCallGate::Warn,
                    CommandVerdict::Deny => ToolCallGate::Block,
                },
                reasons: inspection.reasons,
                receipt: Some(result.receipt),
                degradation: None,
            },
            Ok(None) => ToolCallDecision {
                gate: self.config.mediation_failure.gate(),
                reasons: Vec::new(),
                receipt: Some(result.receipt),
                degradation: Some(GateDegradation::NotProduced),
            },
            Err(_) => ToolCallDecision {
                gate: self.config.mediation_failure.gate(),
                reasons: Vec::new(),
                receipt: Some(result.receipt),
                degradation: Some(GateDegradation::InvalidOutput),
            },
        })
    }

    fn accept_command_decision(
        &self,
        step: &plan::CapabilityPlanStep,
        result: &aw_contracts::provider::ProviderInvocationResult,
    ) -> Result<Option<CommandInspection>, CoreError> {
        if result.receipt.disposition != ProviderDisposition::Produced {
            if receipt_reports_invalid_output(&result.receipt) {
                return Err(CoreError::ProviderOutputRejected);
            }
            return Ok(None);
        }
        let output = result
            .outcome
            .output
            .as_ref()
            .ok_or(CoreError::ProducedWithoutOutput)?;
        if output.schema != step.output_contract.schema {
            return Err(CoreError::UnexpectedOutputSchema {
                actual: schema_label(&output.schema),
            });
        }
        let envelope: CommandInspectionOutput = serde_json::from_value(output.body.clone())?;
        envelope.decision.validate()?;
        if envelope.decision.findings.len() > MAX_OBSERVATION_FINDINGS
            || envelope.decision.reasons.len() > MAX_GATE_REASONS
        {
            return Err(CoreError::TooManyFindings);
        }
        Ok(Some(envelope.decision))
    }

    fn accept_projection_candidate(
        &self,
        output_schema: &VersionedSchema,
        result: &aw_contracts::provider::ProviderInvocationResult,
        artifact_id: &ArtifactId,
        source_digest: &Digest,
    ) -> Result<Option<ContextProjectionCandidate>, CoreError> {
        if result.receipt.disposition != ProviderDisposition::Produced {
            return Ok(None);
        }
        let output = result
            .outcome
            .output
            .as_ref()
            .ok_or(CoreError::ProducedWithoutOutput)?;
        if output.schema != *output_schema {
            return Err(CoreError::UnexpectedOutputSchema {
                actual: schema_label(&output.schema),
            });
        }
        let envelope: ContextProjectionOutput = serde_json::from_value(output.body.clone())?;
        validate_candidate(&envelope.candidate, artifact_id, source_digest)?;
        Ok(Some(envelope.candidate))
    }
}

/// Failure returned by execution-context or context-preparation policy.
#[derive(Debug, Error)]
pub enum CoreError {
    /// An Attempt cannot exist outside durable Work.
    #[error("an Attempt identity requires an Agent Work identity")]
    AttemptWithoutWork,
    /// Tool-call scope requires a logical Agent session.
    #[error("tool-result preparation requires an Agent session identity")]
    ToolResultWithoutAgentSession,
    /// Core Provider ceilings must be enforceable and non-zero.
    #[error("Provider wall-time and output-byte limits must be non-zero")]
    InvalidConfig,
    /// No admitted implementation satisfies the exact Capability Contract.
    #[error("no ready Provider implements the exact Contract for Capability `{capability}`")]
    CapabilityUnavailable {
        /// Capability that could not be routed.
        capability: String,
    },
    /// A matching Provider would receive content without enforced isolation.
    #[error(
        "matching Providers only declare network and filesystem controls; explicit trusted-Provider opt-in is required"
    )]
    ProviderControlsNotEnforced,
    /// More than one implementation qualifies and policy supplied no preference.
    #[error("multiple Providers implement `{capability}`; select one of: {provider_ids}")]
    AmbiguousCapabilityRoute {
        /// Capability with more than one eligible implementation.
        capability: String,
        /// Deterministically sorted eligible Provider identities.
        provider_ids: String,
    },
    /// The requested implementation does not satisfy current routing policy.
    #[error("preferred Provider `{provider_id}` is not eligible")]
    PreferredProviderUnavailable {
        /// Requested Provider identity.
        provider_id: String,
    },
    /// A routing preference named a Capability the plan does not single-route.
    ///
    /// Narrowing a fan-out Capability by one preference would silently stop the
    /// other installed implementations, so Core rejects the request instead.
    #[error("routing preference for `{capability}` is not applicable; planned: {planned}")]
    PreferenceNotApplicable {
        /// Capability named by the rejected preference.
        capability: String,
        /// Capabilities the current plan contains.
        planned: String,
    },
    /// Two admitted entries claim the same Provider and Capability revision.
    #[error("Provider `{provider_id}` declares `{capability}` more than once")]
    DuplicateCapabilityRoute {
        /// Provider identity that appears twice for one Capability.
        provider_id: String,
        /// Capability claimed more than once.
        capability: String,
    },
    /// A resolved step carried no route where its policy requires one.
    #[error("a resolved Capability step carried no Provider route")]
    MissingResolvedRoute,
    /// Produced disposition requires a transient typed output.
    #[error("Provider reported `produced` without a transient output")]
    ProducedWithoutOutput,
    /// Provider Host rejected a native or canonical output against its contract.
    #[error("Provider output did not satisfy its admitted contract")]
    ProviderOutputRejected,
    /// Provider output used a schema other than the selected canonical Contract.
    #[error("Provider returned unexpected output schema `{actual}`")]
    UnexpectedOutputSchema {
        /// Provider-returned schema label.
        actual: String,
    },
    /// Candidate does not identify the source artifact submitted by Core.
    #[error("Provider candidate does not refer to the submitted source artifact")]
    CandidateSourceMismatch,
    /// Candidate exceeds the canonical transformation-chain bound.
    #[error("Provider candidate transform chain exceeds {MAX_TRANSFORM_CHAIN_ITEMS} items")]
    TransformChainTooLong,
    /// Inspection result exceeds the canonical findings bound.
    #[error("Provider inspection exceeds {MAX_OBSERVATION_FINDINGS} findings")]
    TooManyFindings,
    /// System time precedes the Unix epoch.
    #[error("system clock precedes the Unix epoch")]
    ClockBeforeEpoch(#[source] SystemTimeError),
    /// System time cannot be represented by the public millisecond Contract.
    #[error("system time cannot be represented as u64 milliseconds")]
    ClockOutOfRange(#[source] TryFromIntError),
    /// Deadline arithmetic exceeded the public timestamp range.
    #[error("Provider invocation deadline overflowed")]
    DeadlineOverflow,
    /// A built-in context Contract constant is invalid.
    #[error(transparent)]
    ContextContract(#[from] ContextContractBuildError),
    /// A built-in security Contract constant is invalid.
    #[error(transparent)]
    SecurityContract(#[from] SecurityContractBuildError),
    /// A Provider returned a structurally valid but contradictory security fact.
    #[error(transparent)]
    SecurityOutput(#[from] SecurityOutputValidationError),
    /// A bounded Core value could not be constructed.
    #[error(transparent)]
    BoundedValue(#[from] BoundedStringError),
    /// A computed SHA-256 value violated its canonical representation.
    #[error(transparent)]
    Digest(#[from] DigestError),
    /// A deterministic Core identity could not be represented canonically.
    #[error(transparent)]
    Identity(#[from] IdError),
    /// Canonical input or Provider output JSON could not be encoded or decoded.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Provider discovery or invocation failed.
    #[error(transparent)]
    ProviderHost(#[from] ProviderHostError),
}

fn receipt_reports_invalid_output(receipt: &aw_contracts::provider::ProviderReceipt) -> bool {
    receipt
        .error
        .as_ref()
        .is_some_and(|error| error.code.as_str() == "provider_invalid_response")
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ContextProjectionInput<'a> {
    artifact: ContextArtifactInput<'a>,
    boundary: ContextBoundary,
    constraints: ContextProjectionConstraints,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ContextArtifactInput<'a> {
    id: &'a ArtifactId,
    digest: &'a Digest,
    content: &'a str,
    media_type: &'a BoundedName,
    origin: ContextArtifactOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'a BoundedName>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ContextBoundary {
    PostTool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(deny_unknown_fields)]
struct ContextProjectionConstraints {
    allow_text_reencoding: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextProjectionOutput {
    candidate: ContextProjectionCandidate,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ContentInspectionInput<'a> {
    artifact: ContextArtifactInput<'a>,
    boundary: SecurityBoundary,
    constraints: ContentInspectionConstraints,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(deny_unknown_fields)]
struct ContentInspectionConstraints {
    include_low_confidence: bool,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct CodeInspectionInput<'a> {
    artifact: ContextArtifactInput<'a>,
    boundary: SecurityBoundary,
    constraints: CodeInspectionConstraints,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(deny_unknown_fields)]
struct CodeInspectionConstraints {
    language: SecurityCodeLanguage,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentInspectionOutput {
    inspection: ContentInspection,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeInspectionOutput {
    inspection: CodeInspection,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct CommandInspectionInput<'a> {
    command: PendingCommandInput<'a>,
    boundary: SecurityBoundary,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct PendingCommandInput<'a> {
    content: &'a str,
    digest: &'a Digest,
    language: SecurityCodeLanguage,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'a BoundedName>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandInspectionOutput {
    decision: CommandInspection,
}

/// Builds the canonical input for the command-inspection Capability.
fn command_inspection_input(
    command_digest: &Digest,
    submission: &PendingToolCallSubmission,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(CommandInspectionInput {
        command: PendingCommandInput {
            content: &submission.command,
            digest: command_digest,
            language: submission.language,
            tool_name: submission.tool_name.as_ref(),
        },
        boundary: SecurityBoundary::PreTool,
    })
}

/// Builds the canonical input for one inspection Capability.
///
/// The artifact block is identical to the context-projection input so a single
/// Environment event submits the same bytes to several Capabilities under one
/// artifact identity. Only the constraints differ.
fn inspection_input(
    kind: StepKind,
    artifact_id: &ArtifactId,
    source_digest: &Digest,
    submission: &ToolResultSubmission,
) -> Result<serde_json::Value, serde_json::Error> {
    let artifact = ContextArtifactInput {
        id: artifact_id,
        digest: source_digest,
        content: &submission.content,
        media_type: &submission.media_type,
        origin: submission.origin,
        tool_name: submission.tool_name.as_ref(),
    };
    match kind {
        StepKind::CodeInspection => serde_json::to_value(CodeInspectionInput {
            artifact,
            boundary: SecurityBoundary::PostTool,
            constraints: CodeInspectionConstraints {
                language: SecurityCodeLanguage::Auto,
            },
        }),
        // Projection and mediation never reach this builder; the caller matches
        // on the step kind first. Content inspection is the remaining shape.
        StepKind::ContentInspection | StepKind::ContextProjection | StepKind::CommandInspection => {
            serde_json::to_value(ContentInspectionInput {
                artifact,
                boundary: SecurityBoundary::PostTool,
                constraints: ContentInspectionConstraints {
                    include_low_confidence: false,
                },
            })
        }
    }
}

fn context_projection_input(
    artifact_id: &ArtifactId,
    source_digest: &Digest,
    submission: &ToolResultSubmission,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(ContextProjectionInput {
        artifact: ContextArtifactInput {
            id: artifact_id,
            digest: source_digest,
            content: &submission.content,
            media_type: &submission.media_type,
            origin: submission.origin,
            tool_name: submission.tool_name.as_ref(),
        },
        boundary: ContextBoundary::PostTool,
        constraints: ContextProjectionConstraints {
            allow_text_reencoding: submission.allow_text_reencoding,
        },
    })
}

fn validate_candidate(
    candidate: &ContextProjectionCandidate,
    artifact_id: &ArtifactId,
    source_digest: &Digest,
) -> Result<(), CoreError> {
    if candidate.source_artifact_id != *artifact_id || candidate.source_digest != *source_digest {
        return Err(CoreError::CandidateSourceMismatch);
    }
    if candidate.transform_chain.len() > MAX_TRANSFORM_CHAIN_ITEMS {
        return Err(CoreError::TransformChainTooLong);
    }
    Ok(())
}

fn sha256_digest(bytes: &[u8]) -> Result<Digest, DigestError> {
    Digest::parse(format!("{:x}", Sha256::digest(bytes)))
}

fn context_artifact_id(
    execution_context_id: &ExecutionContextId,
    turn_id: &TurnId,
    tool_use_id: &ToolUseId,
    source_digest: &Digest,
) -> Result<ArtifactId, IdError> {
    let mut hasher = Sha256::new();
    hasher.update(b"agent-workload/context-artifact/v1");
    for value in [
        execution_context_id.as_str(),
        turn_id.as_str(),
        tool_use_id.as_str(),
        source_digest.as_str(),
    ] {
        hasher.update([0]);
        hasher.update(value.as_bytes());
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // UUIDv8 marks this as an application-defined, SHA-256-derived identity.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    ArtifactId::parse(format!("art_{}", Uuid::from_bytes(bytes).hyphenated()))
}

fn unix_time_ms() -> Result<u64, CoreError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(CoreError::ClockBeforeEpoch)?;
    u64::try_from(elapsed.as_millis()).map_err(CoreError::ClockOutOfRange)
}

fn canonical_input_digest(body: &serde_json::Value) -> Result<Digest, CoreError> {
    let canonical = canonical_json_v1_bytes(body)?;
    Ok(sha256_digest(&canonical)?)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
