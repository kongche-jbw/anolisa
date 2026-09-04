//! Versioned, transport-neutral contracts for AW Provider discovery and execution.
//!
//! Capability-specific schemas own payload meaning. These contracts only bind
//! that typed payload to Core identity, policy, lifecycle, and result facts.
//! A Driver may carry the envelopes unchanged or bridge an existing native
//! protocol through a codec selected by Capability schema, never by Provider ID.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    common::{BoundedName, BoundedOpaque, BoundedText, Digest, IdempotencyKey, TargetRef},
    error::ContractError,
    ids::{
        ActorId, AgentSessionId, AgentWorkId, AttemptId, EnvironmentId, ExecutionContextId,
        ProviderBindingId, ProviderInvocationId, RequestId, ToolUseId, TurnId,
    },
};

/// Wire version shared by Provider lifecycle commands and results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderApiVersion {
    /// First public Provider lifecycle and invocation contract.
    #[serde(rename = "providers.agentic-os.sh/v1")]
    V1,
}

/// Versioned name of a Capability or JSON payload schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionedSchema {
    /// Stable dotted name independent from a Provider implementation.
    pub id: BoundedName,
    /// Schema revision interpreted within the stable name.
    pub version: u16,
}

/// Content-addressed reference to one language-neutral JSON contract resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaReference {
    /// Stable schema identity and independent revision.
    pub schema: VersionedSchema,
    /// SHA-256 digest of the exact schema resource bytes admitted by Core.
    pub digest: Digest,
}

/// Degree of system authority exercised by one Capability implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthority {
    /// Reports facts without changing execution or recommending a decision.
    Observe,
    /// Produces a candidate that Core may adopt, modify, or ignore.
    Advise,
    /// Returns an allow, deny, approval, or verdict decision at a Core gate.
    Mediate,
    /// Performs a state change or applies a non-bypassable restriction.
    Enforce,
}

/// Public process boundary used by Core to connect to a Provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderDriver {
    /// One process per request with JSON on standard input and output.
    #[serde(rename = "exec-json/v1")]
    ExecJsonV1,
    /// Core-supervised long-lived process using framed standard I/O.
    #[serde(rename = "managed-stdio/v1")]
    ManagedStdioV1,
    /// Existing local daemon reached through a stable local RPC boundary.
    #[serde(rename = "local-service/v1")]
    LocalServiceV1,
    /// Core-managed mount, namespace, or comparable operating-system resource.
    #[serde(rename = "managed-resource/v1")]
    ManagedResourceV1,
}

/// Lifetime over which a Provider instance or binding remains meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderLifecycle {
    /// A fresh Provider process handles one command and then exits.
    OneShot,
    /// The binding follows one Agent session.
    AgentSession,
    /// The binding follows one durable Work object.
    Work,
    /// The binding is shared by one authenticated operating-system user.
    User,
    /// The binding is shared across the governed host.
    Host,
}

/// Scope level at which a Capability may be bound or invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderScopeKind {
    /// Host or remote execution target.
    Host,
    /// Authenticated actor.
    User,
    /// Governed execution context established by an Agent Environment.
    ExecutionContext,
    /// Logical Agent session.
    AgentSession,
    /// Durable user intent owned by the Agentic OS.
    Work,
    /// One execution attempt for durable Work.
    Attempt,
    /// Prompt turn within an Agent session.
    Turn,
    /// Individual tool invocation observed within a turn.
    ToolCall,
}

/// Capability advertised by a Provider implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapabilityDescriptor {
    /// Provider-independent Capability identity and revision.
    pub capability: VersionedSchema,
    /// Authority exercised when this implementation handles the Capability.
    pub authority: ProviderAuthority,
    /// Canonical Capability input contract, independent from native protocol.
    pub input_contract: SchemaReference,
    /// Canonical Capability output contract, independent from native protocol.
    pub output_contract: SchemaReference,
    /// Scope levels at which the implementation supports binding or invocation.
    pub scopes: Vec<ProviderScopeKind>,
}

/// Complete public description used for Provider discovery and admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDescriptor {
    /// Provider protocol implemented by this descriptor.
    pub api_version: ProviderApiVersion,
    /// Stable implementation name, such as `tokenless`.
    pub provider_id: BoundedName,
    /// Provider release version independently from Capability revisions.
    pub provider_version: BoundedName,
    /// Digest of the exact installation manifest bytes admitted by Core.
    pub manifest_digest: Digest,
    /// Public Driver required to connect to the implementation.
    pub driver: ProviderDriver,
    /// Lifetime of the implementation or its Core binding.
    pub lifecycle: ProviderLifecycle,
    /// Versioned Capabilities implemented by this release.
    pub capabilities: Vec<ProviderCapabilityDescriptor>,
}

/// Manifest-declared Provider implementation selected for one invocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSelection {
    /// Stable Provider implementation identity.
    pub provider_id: BoundedName,
    /// Provider release declared by the admitted manifest.
    pub provider_version: BoundedName,
    /// Exact manifest revision admitted by Core.
    pub manifest_digest: Digest,
}

/// Core-owned identities attached to one Provider invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionScope {
    /// Host or remote environment affected by the invocation.
    pub target: TargetRef,
    /// Agent Environment that established the execution context.
    pub environment_id: EnvironmentId,
    /// Governed context shared by all events in this execution.
    pub execution_context_id: ExecutionContextId,
    /// Authenticated actor on whose behalf the invocation executes.
    pub actor_id: ActorId,
    /// Logical Agent session when the invocation belongs to one.
    pub agent_session_id: Option<AgentSessionId>,
    /// Durable Agent Work identity when this invocation belongs to one.
    pub work_id: Option<AgentWorkId>,
    /// Attempt identity when this invocation belongs to managed Work.
    pub attempt_id: Option<AttemptId>,
    /// Prompt turn when known to the Agent Environment.
    pub turn_id: Option<TurnId>,
    /// Tool call when the invocation occurs at a tool boundary.
    pub tool_use_id: Option<ToolUseId>,
}

/// Capability-specific JSON body with an independently versioned schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPayload {
    /// Schema that defines the body rather than an implementation-specific flag set.
    pub schema: VersionedSchema,
    /// SHA-256 digest of [`Self::body`] encoded as Agent Workload canonical JSON v1.
    ///
    /// That encoding recursively sorts object keys, retains array order, and
    /// emits compact UTF-8 JSON so independent Drivers derive one digest.
    pub digest: Digest,
    /// Structured body governed by [`Self::schema`].
    ///
    /// Carrying a schema identity is not proof that a particular Host evaluated
    /// the body against that schema.
    pub body: Value,
}

/// Resource ceilings applied to one Provider invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderInvocationBudget {
    /// Maximum elapsed time before Core must stop waiting for a result.
    pub wall_time_ms: u64,
    /// Maximum encoded output bytes the Driver may accept.
    pub output_bytes: u64,
}

/// One policy-bound invocation of a versioned Capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityInvocation {
    /// Core-owned invocation identity used by receipts and reconciliation.
    pub invocation_id: ProviderInvocationId,
    /// Exact Provider implementation selected by Core.
    pub provider: ProviderSelection,
    /// Capability requested from the selected Provider.
    pub capability: VersionedSchema,
    /// Exact system scope attributed to this invocation.
    pub scope: ExecutionScope,
    /// Scoped Provider binding for stateful lifecycles.
    pub binding_id: Option<ProviderBindingId>,
    /// Caller-scoped replay key retained across transport retries.
    pub idempotency_key: IdempotencyKey,
    /// Policy revision under which Core admitted the invocation.
    pub policy_revision: u64,
    /// Absolute Unix timestamp after which Core rejects the result.
    pub deadline_at_ms: u64,
    /// Resource ceilings enforced by the Driver and Core.
    pub budget: ProviderInvocationBudget,
    /// Capability-specific typed input carried directly by `exec-json`.
    pub input: ProviderPayload,
}

/// Provider binding returned after Core establishes a stateful lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBinding {
    /// Core-owned binding identity.
    pub binding_id: ProviderBindingId,
    /// Provider implementation bound to the scope.
    pub provider_id: BoundedName,
    /// Exact scope governed by the binding.
    pub scope: ExecutionScope,
    /// Monotonic generation used to reject output from restarted instances.
    pub generation: u64,
}

/// Provider availability projected into the Runtime Capability Graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealthState {
    /// Package or executable is present but has not passed admission.
    Installed,
    /// Manifest and policy admission succeeded but readiness is not established.
    Admitted,
    /// Provider is ready to accept its advertised Capabilities.
    Ready,
    /// Some advertised guarantee is unavailable and callers must see degradation.
    Degraded,
    /// Provider cannot currently serve an invocation.
    Unavailable,
}

/// One sampled Provider health result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderHealth {
    /// Current projected state.
    pub state: ProviderHealthState,
    /// Unix timestamp at which the state was sampled.
    pub checked_at_ms: u64,
    /// Optional safe explanation for degraded or unavailable state.
    pub summary: Option<BoundedText>,
}

/// Stable terminal classification recorded by Core for an invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderDisposition {
    /// Provider produced a candidate that Core has not yet adopted or delivered.
    Produced,
    /// Provider performed and settled a real state change or external effect.
    ///
    /// Candidate selection and model delivery are separate Core facts and
    /// must never rewrite a `Produced` Provider receipt into this state.
    EffectApplied,
    /// Policy or Provider safely chose not to change the input or target.
    Bypassed,
    /// Policy denied the invocation before its intended effect.
    Denied,
    /// Provider reached a known terminal failure.
    Failed,
    /// Core cannot prove whether the Provider effect settled.
    Uncertain,
}

/// How a numeric Provider measurement was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMeasurementKind {
    /// Derived by a named estimator rather than observed from the target.
    Estimate,
    /// Observed from the governed execution or target system.
    Observed,
    /// Reported by the system that owns billing for the measured resource.
    Billed,
}

/// One numeric measurement produced while serving a Capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderMeter {
    /// Stable measurement identity, such as `context.input_tokens`.
    pub meter_id: BoundedName,
    /// Stable unit, such as `tokens`, `bytes`, or `milliseconds`.
    pub unit: BoundedName,
    /// Whether this value is estimated, observed, or billed.
    pub measurement_kind: ProviderMeasurementKind,
    /// Named algorithm or source, such as `heuristic-v1`, when applicable.
    pub method: Option<BoundedName>,
    /// Non-negative measured value.
    pub value: u64,
}

/// Reference to separately governed evidence produced by a Provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEvidenceRef {
    /// Evidence category used by retention and presentation policy.
    pub kind: BoundedName,
    /// Opaque locator resolved outside the contracts crate.
    pub reference: BoundedOpaque,
    /// Digest of the referenced evidence when available.
    pub digest: Option<Digest>,
}

/// Transient Provider output returned to the caller alongside a durable receipt.
///
/// The body may contain model-visible or otherwise sensitive content and must
/// not be copied into the durable Provider receipt or generic event ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderInvocationOutcome {
    /// Typed output available only on the immediate invocation return path.
    pub output: Option<ProviderPayload>,
}

/// Terminal Provider facts recorded for one invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderReceipt {
    /// Core-owned invocation whose result is being recorded.
    pub invocation_id: ProviderInvocationId,
    /// Provider identity under which the facts were recorded.
    pub provider_id: BoundedName,
    /// Provider release declared by the admitted manifest.
    pub provider_version: BoundedName,
    /// Admitted manifest revision used for this invocation.
    pub manifest_digest: Digest,
    /// Stateful binding used by the invocation, when one existed.
    pub binding_id: Option<ProviderBindingId>,
    /// Provider generation used to reject facts from a replaced instance.
    pub provider_generation: Option<u64>,
    /// Capability served by the Provider.
    pub capability: VersionedSchema,
    /// Canonical input schema copied from the accepted invocation.
    pub input_schema: VersionedSchema,
    /// Digest of the canonical input body accepted for this invocation.
    pub input_digest: Digest,
    /// Scope copied from the admitted invocation.
    pub scope: ExecutionScope,
    /// Terminal classification independently interpreted by Core.
    pub disposition: ProviderDisposition,
    /// Schema of transient output returned for this invocation, when one exists.
    pub output_schema: Option<VersionedSchema>,
    /// Digest of transient output without copying its body into the receipt.
    pub output_digest: Option<Digest>,
    /// Canonical encoded size of transient output, when one exists.
    pub output_bytes: Option<u64>,
    /// Safe failure attached to denied, failed, or uncertain results.
    pub error: Option<ContractError>,
    /// Measurements attributed to this invocation.
    pub meters: Vec<ProviderMeter>,
    /// References to evidence retained outside this envelope.
    pub evidence: Vec<ProviderEvidenceRef>,
    /// Unix timestamp at which Provider work began.
    pub started_at_ms: u64,
    /// Unix timestamp at which the Provider reported the terminal fact.
    pub completed_at_ms: u64,
}

/// Immediate invocation result with a transient output and durable safe facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderInvocationResult {
    /// Output carried only to the caller that can adopt or deliver it.
    pub outcome: ProviderInvocationOutcome,
    /// Content-free facts safe for the generic Provider receipt ledger.
    pub receipt: ProviderReceipt,
}

/// Cross-field failure in content-free Provider receipt facts.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderReceiptValidationError {
    /// Only part of the output identity tuple was recorded.
    #[error("output schema, digest, and byte count must be present or absent together")]
    IncompleteOutputIdentity,
    /// A JSON output cannot have an empty canonical encoding.
    #[error("output byte count must be non-zero")]
    ZeroOutputBytes,
    /// A produced candidate did not identify its transient output.
    #[error("{disposition:?} disposition requires output identity")]
    MissingOutputIdentity {
        /// Disposition whose required output identity was absent.
        disposition: ProviderDisposition,
    },
    /// A terminal state that cannot return output claimed one.
    #[error("{disposition:?} disposition must not carry output identity")]
    UnexpectedOutputIdentity {
        /// Disposition that contradicted the output identity.
        disposition: ProviderDisposition,
    },
    /// A failure-like terminal state omitted its safe error.
    #[error("{disposition:?} disposition requires an error")]
    MissingError {
        /// Disposition whose required error was absent.
        disposition: ProviderDisposition,
    },
    /// A successful or bypassed terminal state carried an error.
    #[error("{disposition:?} disposition must not carry an error")]
    UnexpectedError {
        /// Disposition that contradicted the attached error.
        disposition: ProviderDisposition,
    },
    /// Receipt timestamps ran backwards.
    #[error("receipt completion timestamp precedes its start timestamp")]
    CompletionBeforeStart,
    /// A receipt fact did not match the invocation accepted by Core.
    #[error("receipt {field} does not match the accepted invocation")]
    InvocationIdentityMismatch {
        /// Stable field name suitable for a content-free diagnostic.
        field: &'static str,
    },
}

/// Cross-field failure between transient output and its durable receipt.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderInvocationResultValidationError {
    /// Content-free receipt facts contradict one another.
    #[error(transparent)]
    Receipt(#[from] ProviderReceiptValidationError),
    /// The transient output and receipt disagree about whether output exists.
    #[error("transient output presence does not match receipt output identity")]
    OutputPresenceMismatch,
    /// The transient schema differs from the content-free receipt schema.
    #[error("transient output schema does not match the receipt")]
    OutputSchemaMismatch,
    /// The transient digest differs from the content-free receipt digest.
    #[error("transient output digest does not match the receipt")]
    OutputDigestMismatch,
    /// The transient body does not match its own canonical digest.
    #[error("transient output body does not match its payload digest")]
    OutputBodyDigestMismatch,
    /// Canonical output JSON could not be encoded.
    #[error("transient output body cannot be encoded as canonical JSON")]
    OutputEncodingFailed,
    /// Canonical output size cannot be represented by the receipt contract.
    #[error("transient output byte count cannot be represented as u64")]
    OutputSizeOverflow,
    /// The transient canonical byte count differs from its receipt.
    #[error("transient output has {actual} canonical bytes but receipt records {recorded}")]
    OutputBytesMismatch {
        /// Byte count recorded in the receipt.
        recorded: u64,
        /// Actual canonical JSON byte count.
        actual: u64,
    },
}

impl ProviderReceipt {
    /// Verifies that terminal state, output identity, error, and time agree.
    ///
    /// `EffectApplied` may carry an output because an effectful Capability can
    /// also return a transient response. Every other disposition has one
    /// unambiguous output rule.
    ///
    /// # Errors
    ///
    /// Returns an error when a terminal fact contradicts another receipt fact.
    pub fn validate(&self) -> Result<(), ProviderReceiptValidationError> {
        let has_schema = self.output_schema.is_some();
        let has_digest = self.output_digest.is_some();
        let has_bytes = self.output_bytes.is_some();
        if has_schema != has_digest || has_schema != has_bytes {
            return Err(ProviderReceiptValidationError::IncompleteOutputIdentity);
        }
        if self.output_bytes == Some(0) {
            return Err(ProviderReceiptValidationError::ZeroOutputBytes);
        }

        let has_output = has_schema;
        match self.disposition {
            ProviderDisposition::Produced if !has_output => {
                return Err(ProviderReceiptValidationError::MissingOutputIdentity {
                    disposition: self.disposition,
                });
            }
            ProviderDisposition::Bypassed
            | ProviderDisposition::Denied
            | ProviderDisposition::Failed
            | ProviderDisposition::Uncertain
                if has_output =>
            {
                return Err(ProviderReceiptValidationError::UnexpectedOutputIdentity {
                    disposition: self.disposition,
                });
            }
            _ => {}
        }

        let requires_error = matches!(
            self.disposition,
            ProviderDisposition::Denied
                | ProviderDisposition::Failed
                | ProviderDisposition::Uncertain
        );
        if requires_error && self.error.is_none() {
            return Err(ProviderReceiptValidationError::MissingError {
                disposition: self.disposition,
            });
        }
        if !requires_error && self.error.is_some() {
            return Err(ProviderReceiptValidationError::UnexpectedError {
                disposition: self.disposition,
            });
        }
        if self.completed_at_ms < self.started_at_ms {
            return Err(ProviderReceiptValidationError::CompletionBeforeStart);
        }
        Ok(())
    }

    /// Verifies that content-free receipt identity came from one invocation.
    ///
    /// # Errors
    ///
    /// Returns an error for contradictory receipt fields or any identity that
    /// differs from the invocation accepted by Core.
    pub fn validate_for_invocation(
        &self,
        invocation: &CapabilityInvocation,
    ) -> Result<(), ProviderReceiptValidationError> {
        self.validate()?;
        for (matches, field) in [
            (
                self.invocation_id == invocation.invocation_id,
                "invocation_id",
            ),
            (
                self.provider_id == invocation.provider.provider_id,
                "provider_id",
            ),
            (
                self.provider_version == invocation.provider.provider_version,
                "provider_version",
            ),
            (
                self.manifest_digest == invocation.provider.manifest_digest,
                "manifest_digest",
            ),
            (self.binding_id == invocation.binding_id, "binding_id"),
            (self.capability == invocation.capability, "capability"),
            (self.scope == invocation.scope, "scope"),
            (self.input_schema == invocation.input.schema, "input_schema"),
            (self.input_digest == invocation.input.digest, "input_digest"),
        ] {
            if !matches {
                return Err(ProviderReceiptValidationError::InvocationIdentityMismatch { field });
            }
        }
        Ok(())
    }
}

impl ProviderInvocationResult {
    /// Verifies that transient output has the exact identity in its receipt.
    ///
    /// # Errors
    ///
    /// Returns an error for contradictory receipt facts, output presence,
    /// schema, digest, canonical body digest, or canonical byte count.
    pub fn validate(&self) -> Result<(), ProviderInvocationResultValidationError> {
        self.receipt.validate()?;
        let Some(output) = &self.outcome.output else {
            return if self.receipt.output_schema.is_none() {
                Ok(())
            } else {
                Err(ProviderInvocationResultValidationError::OutputPresenceMismatch)
            };
        };
        let (Some(output_schema), Some(output_digest), Some(output_bytes)) = (
            &self.receipt.output_schema,
            &self.receipt.output_digest,
            self.receipt.output_bytes,
        ) else {
            return Err(ProviderInvocationResultValidationError::OutputPresenceMismatch);
        };
        if &output.schema != output_schema {
            return Err(ProviderInvocationResultValidationError::OutputSchemaMismatch);
        }
        if &output.digest != output_digest {
            return Err(ProviderInvocationResultValidationError::OutputDigestMismatch);
        }
        let canonical = crate::canonical::canonical_json_v1_bytes(&output.body)
            .map_err(|_| ProviderInvocationResultValidationError::OutputEncodingFailed)?;
        let actual_digest = format!("{:x}", Sha256::digest(&canonical));
        if output.digest.as_str() != actual_digest {
            return Err(ProviderInvocationResultValidationError::OutputBodyDigestMismatch);
        }
        let actual_bytes = u64::try_from(canonical.len())
            .map_err(|_| ProviderInvocationResultValidationError::OutputSizeOverflow)?;
        if actual_bytes != output_bytes {
            return Err(
                ProviderInvocationResultValidationError::OutputBytesMismatch {
                    recorded: output_bytes,
                    actual: actual_bytes,
                },
            );
        }
        Ok(())
    }

    /// Verifies result consistency and binds its receipt to one invocation.
    ///
    /// # Errors
    ///
    /// Returns an error from [`Self::validate`] or when receipt identity differs
    /// from the invocation accepted by Core.
    pub fn validate_for_invocation(
        &self,
        invocation: &CapabilityInvocation,
    ) -> Result<(), ProviderInvocationResultValidationError> {
        self.validate()?;
        self.receipt.validate_for_invocation(invocation)?;
        Ok(())
    }
}

/// Query used after ambiguity or restart to determine the real invocation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconcileQuery {
    /// Invocation whose real state must be recovered.
    pub invocation_id: ProviderInvocationId,
    /// Provider implementation originally selected by Core.
    pub provider_id: BoundedName,
    /// Provider release declared by the originally admitted manifest.
    pub provider_version: BoundedName,
    /// Admitted manifest used for the original invocation.
    pub manifest_digest: Digest,
    /// Scoped binding and generation used by a stateful Provider.
    pub binding: Option<ProviderBinding>,
    /// Capability originally invoked.
    pub capability: VersionedSchema,
    /// Original system scope used to reject cross-scope results.
    pub scope: ExecutionScope,
    /// Original replay key understood by the Provider.
    pub idempotency_key: IdempotencyKey,
    /// Digest of the original capability-specific input body.
    pub input_digest: Digest,
}

/// Provider answer to a restart or uncertainty query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReconcileResult {
    /// Provider has no record that the invocation began.
    NotFound,
    /// Provider still considers the invocation active.
    Pending {
        /// Optional minimum delay before Core queries again.
        retry_after_ms: Option<u64>,
    },
    /// Provider recovered the same terminal facts as a normal invocation.
    Settled {
        /// Recovered terminal receipt.
        receipt: Box<ProviderReceipt>,
    },
    /// Provider cannot prove whether the invocation settled.
    Uncertain {
        /// Safe reason Core records instead of replaying blindly.
        error: ContractError,
    },
}

/// Event returned by a Provider watch operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEvent {
    /// Monotonic sequence within the watched Provider binding.
    pub sequence: u64,
    /// Unix timestamp recorded by the event producer.
    pub occurred_at_ms: u64,
    /// Stable event category interpreted by Core.
    pub kind: BoundedName,
    /// Optional redacted presentation text.
    pub summary: Option<BoundedText>,
}

/// Lifecycle or invocation command issued by the Core Provider Host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ProviderCommand {
    /// Return the complete descriptor without starting Provider data-plane work.
    Describe,
    /// Sample current installation, admission, and readiness state.
    Health,
    /// Establish a stateful Provider binding for an exact scope.
    Bind {
        /// Core-allocated identity that the Provider must echo.
        binding_id: ProviderBindingId,
        /// Scope attached to the requested binding.
        scope: Box<ExecutionScope>,
    },
    /// Execute one typed Capability invocation.
    Invoke {
        /// Policy-bound invocation metadata and JSON payload.
        invocation: Box<CapabilityInvocation>,
    },
    /// Read Provider events after an optional monotonic cursor.
    Watch {
        /// Stateful binding whose events are requested.
        binding_id: ProviderBindingId,
        /// Last sequence already consumed by Core.
        after_sequence: Option<u64>,
    },
    /// Release resources held by a stateful binding.
    Release {
        /// Binding to release idempotently.
        binding_id: ProviderBindingId,
    },
    /// Recover the real outcome of an ambiguous invocation.
    Reconcile {
        /// Original identity, scope, and input digest.
        query: Box<ReconcileQuery>,
    },
}

/// Lifecycle or invocation result returned to the Core Provider Host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ProviderResult {
    /// Result of [`ProviderCommand::Describe`].
    Described {
        /// Public implementation descriptor.
        descriptor: Box<ProviderDescriptor>,
    },
    /// Result of [`ProviderCommand::Health`].
    Health {
        /// Sampled Provider health.
        health: ProviderHealth,
    },
    /// Result of [`ProviderCommand::Bind`].
    Bound {
        /// Established scope and generation.
        binding: Box<ProviderBinding>,
    },
    /// Result of [`ProviderCommand::Invoke`].
    Invoked {
        /// Transient output paired with content-free terminal facts.
        invocation: Box<ProviderInvocationResult>,
    },
    /// Result of [`ProviderCommand::Watch`].
    Watched {
        /// Events after the requested cursor.
        events: Vec<ProviderEvent>,
        /// Last returned sequence for a subsequent watch call.
        next_sequence: Option<u64>,
    },
    /// Result of [`ProviderCommand::Release`].
    Released {
        /// Binding released or already absent.
        binding_id: ProviderBindingId,
    },
    /// Result of [`ProviderCommand::Reconcile`].
    Reconciled {
        /// Provider's recovered view of the invocation.
        reconciliation: Box<ReconcileResult>,
    },
    /// Failure before an invocation was accepted or for a non-invocation command.
    ///
    /// After Core accepts an invocation ID, every terminal Driver path returns
    /// [`Self::Invoked`] with a `Failed` or `Uncertain` content-free receipt.
    Failed {
        /// Safe bounded error returned to Core.
        error: ContractError,
    },
}

/// Versioned request envelope at the Core Provider Host lifecycle boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCommandEnvelope {
    /// Provider protocol version used by the command.
    pub api_version: ProviderApiVersion,
    /// Core-owned request identity echoed by the result.
    pub request_id: RequestId,
    /// Typed lifecycle or invocation command.
    pub command: ProviderCommand,
}

/// Versioned result envelope at the Core Provider Host lifecycle boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderResultEnvelope {
    /// Provider protocol version used by the result.
    pub api_version: ProviderApiVersion,
    /// Request identity copied from [`ProviderCommandEnvelope`].
    pub request_id: RequestId,
    /// Typed lifecycle or invocation result.
    pub result: ProviderResult,
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sha2::{Digest as _, Sha256};

    use super::{
        CapabilityInvocation, ExecutionScope, ProviderApiVersion, ProviderAuthority,
        ProviderCapabilityDescriptor, ProviderCommand, ProviderCommandEnvelope, ProviderDescriptor,
        ProviderDisposition, ProviderDriver, ProviderInvocationBudget, ProviderInvocationOutcome,
        ProviderInvocationResult, ProviderInvocationResultValidationError, ProviderLifecycle,
        ProviderPayload, ProviderReceipt, ProviderReceiptValidationError, ProviderResult,
        ProviderResultEnvelope, ProviderScopeKind, ProviderSelection, ReconcileQuery,
        ReconcileResult, SchemaReference, VersionedSchema,
    };
    use crate::{
        canonical::canonical_json_v1_bytes,
        common::{BoundedName, BoundedOpaque, Digest, IdempotencyKey, TargetRef},
        error::{ContractError, ErrorCategory},
        ids::{
            ActorId, AgentSessionId, EnvironmentId, ExecutionContextId, ProviderInvocationId,
            RequestId, ToolUseId, TurnId,
        },
    };

    fn digest(byte: char) -> Digest {
        Digest::parse(byte.to_string().repeat(64)).expect("test digest is canonical")
    }

    fn schema(id: &str) -> VersionedSchema {
        VersionedSchema {
            id: BoundedName::new(id).expect("test schema name is bounded"),
            version: 1,
        }
    }

    fn scope() -> ExecutionScope {
        ExecutionScope {
            target: TargetRef {
                kind: BoundedName::new("host").expect("test target kind is bounded"),
                authority: BoundedName::new("local").expect("test authority is bounded"),
                identifier: BoundedOpaque::new("host-1").expect("test target is bounded"),
            },
            environment_id: EnvironmentId::new(),
            execution_context_id: ExecutionContextId::new(),
            actor_id: ActorId::new(),
            agent_session_id: Some(AgentSessionId::new()),
            work_id: None,
            attempt_id: None,
            turn_id: Some(TurnId::new()),
            tool_use_id: Some(ToolUseId::new()),
        }
    }

    fn invocation() -> CapabilityInvocation {
        CapabilityInvocation {
            invocation_id: ProviderInvocationId::new(),
            provider: ProviderSelection {
                provider_id: BoundedName::new("tokenless").expect("test provider ID is bounded"),
                provider_version: BoundedName::new("0.1.0")
                    .expect("test Provider version is bounded"),
                manifest_digest: digest('c'),
            },
            capability: schema("context.projection.prepare"),
            scope: scope(),
            binding_id: None,
            idempotency_key: IdempotencyKey::new("tool-result-1")
                .expect("test idempotency key is bounded"),
            policy_revision: 7,
            deadline_at_ms: 1_700_000_001_000,
            budget: ProviderInvocationBudget {
                wall_time_ms: 2_000,
                output_bytes: 1_048_576,
            },
            input: ProviderPayload {
                schema: schema("context.projection.prepare.input"),
                digest: digest('a'),
                body: json!({
                    "artifact": {
                        "id": "art_00000000-0000-4000-8000-000000000001",
                        "digest": "a".repeat(64),
                        "content": "large tool result",
                        "media_type": "text/plain",
                        "origin": "command_output"
                    },
                    "boundary": "post_tool",
                    "constraints": {
                        "allow_text_reencoding": true
                    }
                }),
            },
        }
    }

    fn valid_invocation_result() -> (CapabilityInvocation, ProviderInvocationResult) {
        let invocation = invocation();
        let body = json!({"output": "sensitive candidate"});
        let encoded = canonical_json_v1_bytes(&body).expect("test output is canonical JSON");
        let output_digest = Digest::parse(format!("{:x}", Sha256::digest(&encoded)))
            .expect("SHA-256 output is canonical");
        let output = ProviderPayload {
            schema: schema("context.projection.prepare.output"),
            digest: output_digest.clone(),
            body,
        };
        let receipt = ProviderReceipt {
            invocation_id: invocation.invocation_id.clone(),
            provider_id: invocation.provider.provider_id.clone(),
            provider_version: invocation.provider.provider_version.clone(),
            manifest_digest: invocation.provider.manifest_digest.clone(),
            binding_id: invocation.binding_id.clone(),
            provider_generation: None,
            capability: invocation.capability.clone(),
            input_schema: invocation.input.schema.clone(),
            input_digest: invocation.input.digest.clone(),
            scope: invocation.scope.clone(),
            disposition: ProviderDisposition::Produced,
            output_schema: Some(output.schema.clone()),
            output_digest: Some(output_digest),
            output_bytes: Some(encoded.len() as u64),
            error: None,
            meters: Vec::new(),
            evidence: Vec::new(),
            started_at_ms: 1_700_000_000_000,
            completed_at_ms: 1_700_000_000_010,
        };
        (
            invocation,
            ProviderInvocationResult {
                outcome: ProviderInvocationOutcome {
                    output: Some(output),
                },
                receipt,
            },
        )
    }

    #[test]
    fn exec_json_invocation_preserves_typed_payload_and_identity() {
        let invocation = invocation();
        let envelope = ProviderCommandEnvelope {
            api_version: ProviderApiVersion::V1,
            request_id: RequestId::new(),
            command: ProviderCommand::Invoke {
                invocation: Box::new(invocation.clone()),
            },
        };

        let value = serde_json::to_value(&envelope).expect("Provider command serializes");
        assert_eq!(value["api_version"], "providers.agentic-os.sh/v1");
        assert_eq!(value["command"]["command"], "invoke");
        assert_eq!(
            value["command"]["invocation"]["capability"]["id"],
            "context.projection.prepare"
        );
        assert_eq!(
            value["command"]["invocation"]["input"]["body"]["artifact"]["content"],
            "large tool result"
        );

        let decoded: ProviderCommandEnvelope =
            serde_json::from_value(value).expect("Provider command deserializes");
        assert_eq!(decoded, envelope);
        assert_eq!(invocation.input.digest, digest('a'));
    }

    #[test]
    fn provider_wire_rejects_unknown_api_versions() {
        let envelope = ProviderCommandEnvelope {
            api_version: ProviderApiVersion::V1,
            request_id: RequestId::new(),
            command: ProviderCommand::Health,
        };
        let mut value = serde_json::to_value(envelope).expect("Provider command serializes");
        value["api_version"] = json!("providers.agentic-os.sh/v2");

        assert!(serde_json::from_value::<ProviderCommandEnvelope>(value).is_err());
    }

    #[test]
    fn receipt_and_reconcile_share_one_terminal_fact_shape() {
        let invocation = invocation();
        let receipt = ProviderReceipt {
            invocation_id: invocation.invocation_id.clone(),
            provider_id: BoundedName::new("tokenless").expect("test provider ID is bounded"),
            provider_version: BoundedName::new("0.1.0").expect("test version is bounded"),
            manifest_digest: digest('c'),
            binding_id: None,
            provider_generation: Some(1),
            capability: invocation.capability.clone(),
            input_schema: invocation.input.schema.clone(),
            input_digest: invocation.input.digest.clone(),
            scope: invocation.scope.clone(),
            disposition: ProviderDisposition::Produced,
            output_schema: Some(schema("context.projection.prepare.output")),
            output_digest: Some(digest('b')),
            output_bytes: Some(96),
            error: None,
            meters: Vec::new(),
            evidence: Vec::new(),
            started_at_ms: 1_700_000_000_000,
            completed_at_ms: 1_700_000_000_010,
        };
        let result = ProviderResultEnvelope {
            api_version: ProviderApiVersion::V1,
            request_id: RequestId::new(),
            result: ProviderResult::Reconciled {
                reconciliation: Box::new(ReconcileResult::Settled {
                    receipt: Box::new(receipt.clone()),
                }),
            },
        };

        let encoded = serde_json::to_string(&result).expect("Provider result serializes");
        let decoded: ProviderResultEnvelope =
            serde_json::from_str(&encoded).expect("Provider result deserializes");
        assert_eq!(decoded, result);
        assert!(encoded.contains("\"state\":\"settled\""));
        assert!(encoded.contains("\"disposition\":\"produced\""));
        assert!(encoded.contains("\"provider_version\":\"0.1.0\""));
        assert!(encoded.contains(&format!("\"manifest_digest\":\"{}\"", "c".repeat(64))));
        assert!(!encoded.contains("compressed result"));

        let query = ReconcileQuery {
            invocation_id: invocation.invocation_id,
            provider_id: receipt.provider_id,
            provider_version: receipt.provider_version,
            manifest_digest: receipt.manifest_digest,
            binding: None,
            capability: invocation.capability,
            scope: invocation.scope,
            idempotency_key: invocation.idempotency_key,
            input_digest: invocation.input.digest,
        };
        assert_eq!(query.input_digest, digest('a'));
    }

    #[test]
    fn invocation_output_is_transient_and_receipt_is_content_free() {
        let (_, invocation_result) = valid_invocation_result();
        invocation_result
            .validate()
            .expect("wire fixture is internally consistent");
        let result = ProviderResultEnvelope {
            api_version: ProviderApiVersion::V1,
            request_id: RequestId::new(),
            result: ProviderResult::Invoked {
                invocation: Box::new(invocation_result),
            },
        };

        let encoded = serde_json::to_value(result).expect("invocation result serializes");
        assert_eq!(
            encoded["result"]["invocation"]["outcome"]["output"]["body"]["output"],
            "sensitive candidate"
        );
        let receipt_value = &encoded["result"]["invocation"]["receipt"];
        assert!(receipt_value.get("output").is_none());
        assert!(receipt_value["output_bytes"]
            .as_u64()
            .is_some_and(|size| size > 0));
    }

    #[test]
    fn invocation_result_validation_accepts_exact_output_and_input_identity() {
        let (invocation, result) = valid_invocation_result();

        result.validate().expect("result facts agree");
        result
            .validate_for_invocation(&invocation)
            .expect("receipt belongs to the accepted invocation");
        assert_eq!(result.receipt.input_schema, invocation.input.schema);
        assert_eq!(result.receipt.input_digest, invocation.input.digest);
    }

    #[test]
    fn receipt_validation_rejects_terminal_state_contradictions() {
        let (_, result) = valid_invocation_result();

        let mut receipt = result.receipt.clone();
        receipt.output_digest = None;
        assert_eq!(
            receipt.validate(),
            Err(ProviderReceiptValidationError::IncompleteOutputIdentity)
        );

        let mut receipt = result.receipt.clone();
        receipt.output_schema = None;
        receipt.output_digest = None;
        receipt.output_bytes = None;
        assert_eq!(
            receipt.validate(),
            Err(ProviderReceiptValidationError::MissingOutputIdentity {
                disposition: ProviderDisposition::Produced,
            })
        );

        let mut receipt = result.receipt.clone();
        receipt.disposition = ProviderDisposition::Bypassed;
        assert_eq!(
            receipt.validate(),
            Err(ProviderReceiptValidationError::UnexpectedOutputIdentity {
                disposition: ProviderDisposition::Bypassed,
            })
        );

        let mut receipt = result.receipt.clone();
        receipt.disposition = ProviderDisposition::Failed;
        receipt.output_schema = None;
        receipt.output_digest = None;
        receipt.output_bytes = None;
        assert_eq!(
            receipt.validate(),
            Err(ProviderReceiptValidationError::MissingError {
                disposition: ProviderDisposition::Failed,
            })
        );

        let mut receipt = result.receipt.clone();
        receipt.error = Some(
            ContractError::new(
                "unexpected",
                ErrorCategory::Internal,
                false,
                "unexpected error",
            )
            .expect("test error is bounded"),
        );
        assert_eq!(
            receipt.validate(),
            Err(ProviderReceiptValidationError::UnexpectedError {
                disposition: ProviderDisposition::Produced,
            })
        );

        let mut receipt = result.receipt;
        receipt.completed_at_ms = receipt.started_at_ms - 1;
        assert_eq!(
            receipt.validate(),
            Err(ProviderReceiptValidationError::CompletionBeforeStart)
        );
    }

    #[test]
    fn invocation_result_validation_rejects_output_identity_mismatches() {
        let (_, result) = valid_invocation_result();

        let mut missing_output = result.clone();
        missing_output.outcome.output = None;
        assert_eq!(
            missing_output.validate(),
            Err(ProviderInvocationResultValidationError::OutputPresenceMismatch)
        );

        let mut wrong_schema = result.clone();
        wrong_schema.receipt.output_schema = Some(schema("context.projection.other.output"));
        assert_eq!(
            wrong_schema.validate(),
            Err(ProviderInvocationResultValidationError::OutputSchemaMismatch)
        );

        let mut wrong_receipt_digest = result.clone();
        wrong_receipt_digest.receipt.output_digest = Some(digest('f'));
        assert_eq!(
            wrong_receipt_digest.validate(),
            Err(ProviderInvocationResultValidationError::OutputDigestMismatch)
        );

        let mut modified_body = result.clone();
        modified_body
            .outcome
            .output
            .as_mut()
            .expect("valid fixture has output")
            .body = json!({"output": "modified after digest"});
        assert_eq!(
            modified_body.validate(),
            Err(ProviderInvocationResultValidationError::OutputBodyDigestMismatch)
        );

        let mut wrong_size = result;
        wrong_size.receipt.output_bytes = wrong_size.receipt.output_bytes.map(|size| size + 1);
        assert!(matches!(
            wrong_size.validate(),
            Err(ProviderInvocationResultValidationError::OutputBytesMismatch { .. })
        ));
    }

    #[test]
    fn receipt_validation_rejects_cross_invocation_input_identity() {
        let (invocation, result) = valid_invocation_result();

        let mut wrong_schema = result.clone();
        wrong_schema.receipt.input_schema = schema("context.projection.other.input");
        assert_eq!(
            wrong_schema.validate_for_invocation(&invocation),
            Err(ProviderInvocationResultValidationError::Receipt(
                ProviderReceiptValidationError::InvocationIdentityMismatch {
                    field: "input_schema",
                }
            ))
        );

        let mut wrong_digest = result;
        wrong_digest.receipt.input_digest = digest('f');
        assert_eq!(
            wrong_digest.validate_for_invocation(&invocation),
            Err(ProviderInvocationResultValidationError::Receipt(
                ProviderReceiptValidationError::InvocationIdentityMismatch {
                    field: "input_digest",
                }
            ))
        );
    }

    #[test]
    fn descriptor_dimensions_remain_orthogonal() {
        assert_eq!(
            serde_json::to_value(ProviderAuthority::Advise).expect("authority serializes"),
            json!("advise")
        );
        assert_eq!(
            serde_json::to_value(ProviderDriver::ExecJsonV1).expect("Driver serializes"),
            json!("exec-json/v1")
        );
        assert_eq!(
            serde_json::to_value(ProviderLifecycle::OneShot).expect("lifecycle serializes"),
            json!("one_shot")
        );
        assert_eq!(
            serde_json::to_value(ProviderScopeKind::ToolCall).expect("scope serializes"),
            json!("tool_call")
        );

        let descriptor = ProviderDescriptor {
            api_version: ProviderApiVersion::V1,
            provider_id: BoundedName::new("tokenless").expect("test provider ID is bounded"),
            provider_version: BoundedName::new("0.1.0").expect("test version is bounded"),
            manifest_digest: digest('c'),
            driver: ProviderDriver::ExecJsonV1,
            lifecycle: ProviderLifecycle::OneShot,
            capabilities: vec![ProviderCapabilityDescriptor {
                capability: schema("context.projection.prepare"),
                authority: ProviderAuthority::Advise,
                input_contract: SchemaReference {
                    schema: schema("context.projection.prepare.input"),
                    digest: digest('d'),
                },
                output_contract: SchemaReference {
                    schema: schema("context.projection.prepare.output"),
                    digest: digest('e'),
                },
                scopes: vec![ProviderScopeKind::AgentSession, ProviderScopeKind::ToolCall],
            }],
        };
        let encoded = serde_json::to_value(&descriptor).expect("descriptor serializes");
        assert_eq!(encoded["provider_id"], "tokenless");
        assert_eq!(encoded["capabilities"][0]["authority"], "advise");
        assert_eq!(
            serde_json::from_value::<ProviderDescriptor>(encoded).expect("descriptor deserializes"),
            descriptor
        );
    }
}
