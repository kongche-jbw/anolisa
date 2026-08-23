//! Wire types shared by Runtime adapters and Memory backend implementations.

use std::{borrow::Cow, collections::HashSet, fmt};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Stable protocol family advertised to Runtime adapters.
pub const MEMORY_PROTOCOL_NAME: &str = "anolisa.agent-memory";

/// Current major wire version.
pub const MEMORY_PROTOCOL_VERSION: u32 = 1;

const MAX_ID_BYTES: usize = 256;
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_ITEMS: usize = 256;
const MAX_CONTEXT_BYTES: u32 = 512 * 1024;
const MAX_CONTEXT_TOKENS: u32 = 128 * 1024;
const MAX_REQUEST_CONTENT_BYTES: usize = 1024 * 1024;

/// Identity established by the Runtime boundary rather than model-provided input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdentityContext {
    /// Optional managed-service tenant; local-only profiles leave it absent.
    pub tenant_id: Option<String>,
    /// Optional organization team within a managed-service tenant.
    pub team_id: Option<String>,
    /// Operating-system or managed-service user identity.
    pub user_id: String,
    /// Runtime or Agent identity within the user scope.
    pub agent_id: String,
    /// Runtime session identity.
    pub session_id: String,
    /// Stable workspace identity, independent from a display path.
    pub workspace_id: String,
}

impl IdentityContext {
    /// Validates required identity fields before a backend sees the request.
    pub fn validate(&self) -> ProtocolResult<()> {
        if let Some(tenant_id) = &self.tenant_id {
            validate_identifier("tenant_id", tenant_id)?;
        }
        if let Some(team_id) = &self.team_id {
            validate_identifier("team_id", team_id)?;
        }
        validate_identifier("user_id", &self.user_id)?;
        validate_identifier("agent_id", &self.agent_id)?;
        validate_identifier("session_id", &self.session_id)?;
        validate_identifier("workspace_id", &self.workspace_id)
    }

    /// Returns a stable key for session-scoped backend state.
    pub fn session_key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.tenant_id.as_deref().unwrap_or(""),
            self.team_id.as_deref().unwrap_or(""),
            self.user_id,
            self.agent_id,
            self.workspace_id,
            self.session_id
        )
    }

    /// Returns a stable key for workspace-scoped backend state.
    pub fn workspace_key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.tenant_id.as_deref().unwrap_or(""),
            self.team_id.as_deref().unwrap_or(""),
            self.user_id,
            self.agent_id,
            self.workspace_id
        )
    }
}

/// Capabilities negotiated independently from a concrete backend product.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MemoryCapability {
    /// Opens and closes Runtime sessions.
    Session,
    /// Captures typed events.
    Capture,
    /// Recalls and materializes bounded context.
    Recall,
    /// Persists resumable task state.
    Checkpoint,
    /// Explains a materialized context view.
    Explain,
    /// Records context admission and usefulness outcomes.
    Outcome,
    /// Removes a memory object from the active backend.
    Forget,
    /// Resolves external normative knowledge.
    Knowledge,
    /// Capability introduced by a newer compatible backend.
    Other(String),
}

impl MemoryCapability {
    /// Returns the stable wire value without losing unknown capability names.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Session => "session",
            Self::Capture => "capture",
            Self::Recall => "recall",
            Self::Checkpoint => "checkpoint",
            Self::Explain => "explain",
            Self::Outcome => "outcome",
            Self::Forget => "forget",
            Self::Knowledge => "knowledge",
            Self::Other(value) => value,
        }
    }
}

impl Serialize for MemoryCapability {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MemoryCapability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "session" => Self::Session,
            "capture" => Self::Capture,
            "recall" => Self::Recall,
            "checkpoint" => Self::Checkpoint,
            "explain" => Self::Explain,
            "outcome" => Self::Outcome,
            "forget" => Self::Forget,
            "knowledge" => Self::Knowledge,
            _ => Self::Other(value),
        })
    }
}

impl JsonSchema for MemoryCapability {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("MemoryCapability")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        String::json_schema(generator)
    }
}

/// Durability acknowledged by a successful mutation response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryDurability {
    /// State exists only for the lifetime of the backend process.
    ProcessLocal,
    /// State is committed to a crash-recoverable backend.
    Durable,
    /// Durability level introduced by a newer compatible backend.
    #[serde(other)]
    Unknown,
}

/// Backend identity and supported protocol capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BackendManifest {
    /// Stable backend implementation identifier.
    pub backend_id: String,
    /// User-facing backend name.
    pub display_name: String,
    /// Major protocol version implemented by the backend.
    pub protocol_version: u32,
    /// Explicitly supported capabilities.
    pub capabilities: Vec<MemoryCapability>,
    /// Mutation acknowledgement semantics offered by this backend.
    pub durability: MemoryDurability,
}

/// Runtime properties relevant to memory selection and recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeContext {
    /// Runtime implementation, such as cosh-ng or deepseek-harness.
    pub runtime: String,
    /// Runtime version when known.
    pub runtime_version: Option<String>,
    /// Model route used for the current session when known.
    pub model: Option<String>,
    /// Host platform used to invalidate platform-specific experience.
    pub platform: Option<String>,
}

/// Hard allocation passed to context materialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextBudget {
    /// Maximum tokens admitted from all Memory sources.
    pub max_tokens: u32,
    /// Maximum UTF-8 bytes admitted from all Memory sources.
    pub max_bytes: u32,
    /// Maximum number of admitted items.
    pub max_items: u16,
}

/// Purpose controls source quotas and recovery policy without coupling to one Runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecallPurpose {
    /// Context for a normal user turn.
    Turn,
    /// Context used to restore an interrupted session.
    SessionResume,
    /// Context prepared for another Agent or Runtime.
    Handoff,
}

/// Optional task and target binding used by recall policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecallBinding {
    /// Task whose state or experience should be recalled.
    pub task_id: Option<String>,
    /// Receiving Agent for a handoff recall.
    pub target_agent_id: Option<String>,
}

/// Runtime event category captured by a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEventKind {
    /// Session initialization was observed.
    SessionStarted,
    /// A user prompt was submitted.
    UserPrompt,
    /// A tool completed successfully.
    ToolCompleted,
    /// A tool failed or produced an unknown outcome.
    ToolFailed,
    /// A final model turn was committed by the Runtime.
    TurnCommitted,
    /// Session shutdown was observed.
    SessionStopped,
}

/// Observed result attached to an immutable Runtime event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEventOutcome {
    /// Operation completed successfully.
    Succeeded,
    /// Operation failed with a known outcome.
    Failed,
    /// Completion is not known, such as an interrupted tool call.
    Unknown,
}

/// Bounded event record; large or sensitive payloads remain behind evidence references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryEvent {
    /// Runtime-allocated event identity.
    pub event_id: String,
    /// Event semantic category.
    pub kind: MemoryEventKind,
    /// Stable Runtime or adapter source identifier.
    pub source: String,
    /// Observed operation outcome.
    pub outcome: MemoryEventOutcome,
    /// Millisecond wall-clock observation time.
    pub observed_at_ms: u64,
    /// Redacted bounded summary used for candidate extraction.
    pub summary: String,
    /// Optional opaque reference to canonical evidence.
    pub evidence_ref: Option<String>,
}

/// Resumable task state kept separate from a full conversation transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskState {
    /// Runtime or user-assigned task identity.
    pub task_id: String,
    /// Monotonic task projection revision, starting at one.
    pub revision: u64,
    /// Current task goal.
    pub goal: String,
    /// Next verified action to attempt.
    pub next_action: Option<String>,
    /// Known blockers that must not be invented during recovery.
    pub blockers: Vec<String>,
    /// Millisecond wall-clock update time.
    pub updated_at_ms: u64,
}

/// Reference to evidence owned by another component or external system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    /// Evidence provider identifier.
    pub provider: String,
    /// Canonical opaque resource reference.
    pub uri: String,
    /// Content digest when the provider exposes one.
    pub digest: Option<String>,
    /// Redacted bounded summary.
    pub summary: String,
}

/// Reference to normative knowledge owned by a replaceable provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeRef {
    /// Knowledge provider identifier.
    pub provider: String,
    /// Canonical document identity.
    pub document_id: String,
    /// Provider-defined section selector.
    pub selector: Option<String>,
    /// Source content digest used for staleness checks.
    pub content_digest: Option<String>,
    /// Retrieval time in milliseconds.
    pub retrieved_at_ms: u64,
}

/// Type of information admitted into a model-visible ContextView.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextItemKind {
    /// Resumable task goal, next action, or blocker.
    TaskState,
    /// Verified historical experience.
    Experience,
    /// Observed evidence reference.
    Evidence,
    /// Normative provider-owned knowledge.
    Knowledge,
    /// User or organization policy.
    Policy,
}

/// Authority level applied before an item may enter model context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAuthority {
    /// Automatically extracted proposal that must not act as instruction.
    Candidate,
    /// Evidence-backed or user-approved memory.
    Verified,
    /// Normative source controlled outside the Memory backend.
    Normative,
}

/// One admitted, attributable piece of context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextItem {
    /// Stable item identity.
    pub item_id: String,
    /// Source projection revision when the item is revisioned.
    pub revision: Option<u64>,
    /// Semantic item kind.
    pub kind: ContextItemKind,
    /// Model-visible bounded content.
    pub content: String,
    /// Canonical source or provider reference.
    pub source_ref: String,
    /// Trust classification used by admission policy.
    pub authority: MemoryAuthority,
    /// Estimated model token cost.
    pub token_estimate: u32,
    /// Retrieval and admission explanation.
    pub reason: String,
    /// True when the source requires revalidation.
    pub stale: bool,
    /// Backend ranking score before budget admission.
    pub score: f32,
}

/// Bounded model-visible context assembled for one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextView {
    /// Backend-allocated view identity used by explain and feedback.
    pub context_view_id: String,
    /// Trace identity used to correlate provider and admission decisions.
    pub trace_id: String,
    /// Backend snapshot used to select all returned items.
    pub snapshot_revision: u64,
    /// Original bounded query.
    pub query: String,
    /// Items admitted in final injection order.
    pub items: Vec<ContextItem>,
    /// Sum of admitted token estimates.
    pub total_tokens: u32,
    /// Sum of admitted UTF-8 bytes.
    pub total_bytes: u32,
    /// Effective retrieval strategy after any explicit fallback.
    pub effective_strategy: String,
    /// True when a provider or retrieval strategy degraded.
    pub degraded: bool,
    /// True when budget packing omitted otherwise eligible items.
    pub truncated: bool,
    /// Millisecond creation time.
    pub created_at_ms: u64,
}

impl ContextView {
    pub(crate) fn validate_backend_result(
        &self,
        expected_query: &str,
        expected_trace_id: &str,
        budget: ContextBudget,
    ) -> ProtocolResult<()> {
        if self.query != expected_query || self.trace_id != expected_trace_id {
            return Err(ProtocolError::new(
                ProtocolErrorCode::IntegrityFailed,
                "backend context correlation does not match the request",
                false,
            ));
        }
        validate_identifier("context_view_id", &self.context_view_id)?;
        validate_identifier("context.trace_id", &self.trace_id)?;
        validate_identifier("effective_strategy", &self.effective_strategy)?;
        if self.items.len() > usize::from(budget.max_items) {
            return Err(ProtocolError::new(
                ProtocolErrorCode::ResourceExhausted,
                "backend context exceeds the item budget",
                false,
            ));
        }
        let mut item_ids = HashSet::new();
        let mut total_tokens = 0_u32;
        let mut total_bytes = 0_u32;
        for item in &self.items {
            validate_identifier("context.item_id", &item.item_id)?;
            validate_text("context.content", &item.content, MAX_CONTEXT_BYTES as usize)?;
            validate_text("context.source_ref", &item.source_ref, MAX_TEXT_BYTES)?;
            validate_text("context.reason", &item.reason, MAX_TEXT_BYTES)?;
            if !item_ids.insert(&item.item_id) || !item.score.is_finite() {
                return Err(ProtocolError::new(
                    ProtocolErrorCode::IntegrityFailed,
                    "backend context contains duplicate identities or non-finite scores",
                    false,
                ));
            }
            total_tokens = total_tokens.saturating_add(item.token_estimate);
            total_bytes =
                total_bytes.saturating_add(u32::try_from(item.content.len()).unwrap_or(u32::MAX));
        }
        if total_tokens != self.total_tokens || total_bytes != self.total_bytes {
            return Err(ProtocolError::new(
                ProtocolErrorCode::IntegrityFailed,
                "backend context totals do not match returned items",
                false,
            ));
        }
        if total_tokens > budget.max_tokens || total_bytes > budget.max_bytes {
            return Err(ProtocolError::new(
                ProtocolErrorCode::ResourceExhausted,
                "backend context exceeds the token or byte budget",
                false,
            ));
        }
        Ok(())
    }
}

/// Decision for one candidate considered during materialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RecallDecision {
    /// Candidate item identity.
    pub item_id: String,
    /// Whether the candidate entered the final ContextView.
    pub admitted: bool,
    /// Stable decision reason.
    pub reason: String,
    /// Rank before token-budget packing.
    pub rank: u32,
    /// Score reported by the selected backend policy.
    pub score: f32,
}

/// Explainable trace for one materialized ContextView.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RecallTrace {
    /// ContextView explained by this trace.
    pub context_view_id: String,
    /// End-to-end trace identity supplied by the trusted Runtime.
    pub trace_id: String,
    /// Trace identity of the request returning this trace.
    pub response_trace_id: String,
    /// Backend that selected candidates.
    pub backend_id: String,
    /// Ordered admission decisions.
    pub decisions: Vec<RecallDecision>,
    /// True when a backend or provider degraded.
    pub degraded: bool,
    /// Safe degradation reason, when present.
    pub degradation_reason: Option<String>,
    /// Runtime-side admission and usefulness report, once available.
    pub outcome_report: Option<RecallOutcomeReport>,
}

impl RecallTrace {
    pub(crate) fn validate_backend_result(
        &self,
        expected_context_view_id: &str,
        expected_trace_id: &str,
    ) -> ProtocolResult<()> {
        if self.context_view_id != expected_context_view_id
            || self.response_trace_id != expected_trace_id
        {
            return Err(ProtocolError::new(
                ProtocolErrorCode::IntegrityFailed,
                "backend recall trace correlation does not match the request",
                false,
            ));
        }
        validate_identifier("trace.trace_id", &self.trace_id)?;
        validate_identifier("trace.backend_id", &self.backend_id)?;
        validate_optional_text(
            "trace.degradation_reason",
            self.degradation_reason.as_deref(),
        )?;
        if self.decisions.len() > MAX_ITEMS {
            return Err(ProtocolError::new(
                ProtocolErrorCode::ResourceExhausted,
                "backend recall trace exceeds the decision limit",
                false,
            ));
        }
        let mut candidate_ids = HashSet::new();
        let mut returned_ids = HashSet::new();
        for decision in &self.decisions {
            validate_identifier("trace.item_id", &decision.item_id)?;
            validate_text("trace.reason", &decision.reason, MAX_TEXT_BYTES)?;
            if decision.rank == 0
                || !decision.score.is_finite()
                || !candidate_ids.insert(decision.item_id.as_str())
            {
                return Err(ProtocolError::new(
                    ProtocolErrorCode::IntegrityFailed,
                    "backend recall trace contains invalid rank, score, or duplicate identity",
                    false,
                ));
            }
            if decision.admitted {
                returned_ids.insert(decision.item_id.as_str());
            }
        }
        if let Some(report) = &self.outcome_report {
            let reported: Vec<&str> = report
                .admitted_item_ids
                .iter()
                .chain(&report.dropped_item_ids)
                .map(String::as_str)
                .collect();
            let unique: HashSet<&str> = reported.iter().copied().collect();
            if unique.len() != reported.len()
                || unique != returned_ids
                || (returned_ids.is_empty() && matches!(report.outcome, FeedbackOutcome::Useful))
            {
                return Err(ProtocolError::new(
                    ProtocolErrorCode::IntegrityFailed,
                    "backend recall outcome is not a complete unique partition",
                    false,
                ));
            }
        }
        Ok(())
    }
}

/// Runtime-side result for candidates returned in a ContextView.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecallOutcomeReport {
    /// Items the Runtime actually admitted into model context.
    pub admitted_item_ids: Vec<String>,
    /// Returned items dropped by the Runtime admission layer.
    pub dropped_item_ids: Vec<String>,
    /// Known usefulness result, or unknown when no sound attribution exists.
    pub outcome: FeedbackOutcome,
}

/// Outcome used to evaluate whether recalled context helped the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackOutcome {
    /// Runtime or user confirmed useful contribution.
    Useful,
    /// Context was irrelevant but not harmful.
    Irrelevant,
    /// Context caused or encouraged an incorrect action.
    Harmful,
    /// No reliable judgment is available.
    Unknown,
}

/// Terminal Runtime session outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionOutcome {
    /// Task or turn completed normally.
    Completed,
    /// Runtime stopped with unfinished work.
    Interrupted,
    /// Runtime or backend failed.
    Failed,
}

/// Memory-owned object category accepted by scoped forget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryObjectKind {
    /// Durable or process-local task checkpoint.
    Task,
    /// Captured Runtime event.
    Event,
    /// Materialized ContextView and its RecallTrace.
    ContextView,
}

/// Trusted correlation and deadline context passed to every backend operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendRequestContext {
    /// Request correlation identity.
    pub request_id: String,
    /// End-to-end tracing identity.
    pub trace_id: String,
    /// Runtime run identity when known.
    pub run_id: Option<String>,
    /// Task identity when known.
    pub task_id: Option<String>,
    /// Turn identity when known.
    pub turn_id: Option<String>,
    /// Absolute request deadline in milliseconds since Unix epoch.
    pub deadline_at_ms: Option<u64>,
    /// Authenticated Runtime identity.
    pub identity: IdentityContext,
}

/// Versioned request envelope consumed by any conforming backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryRequestEnvelope {
    /// Requested major protocol version.
    pub protocol_version: u32,
    /// Caller-allocated request correlation identity.
    pub request_id: String,
    /// End-to-end trace identity allocated by the trusted Runtime boundary.
    pub trace_id: String,
    /// Runtime run identity when the host exposes one.
    pub run_id: Option<String>,
    /// Task correlation supplied by the Runtime when known.
    pub task_id: Option<String>,
    /// Turn correlation supplied by the Runtime when known.
    pub turn_id: Option<String>,
    /// Absolute request deadline in milliseconds since Unix epoch.
    pub deadline_at_ms: Option<u64>,
    /// Authenticated Runtime identity.
    pub identity: IdentityContext,
    /// Typed operation payload.
    pub request: MemoryRequest,
}

impl MemoryRequestEnvelope {
    /// Validates protocol, identity, identifiers, bounds, and operation inputs.
    pub fn validate(&self) -> ProtocolResult<()> {
        if self.protocol_version != MEMORY_PROTOCOL_VERSION {
            return Err(ProtocolError::new(
                ProtocolErrorCode::VersionUnsupported,
                format!(
                    "unsupported memory protocol version {}; expected {}",
                    self.protocol_version, MEMORY_PROTOCOL_VERSION
                ),
                false,
            ));
        }
        validate_identifier("request_id", &self.request_id)?;
        validate_identifier("trace_id", &self.trace_id)?;
        if let Some(run_id) = &self.run_id {
            validate_identifier("run_id", run_id)?;
        }
        if let Some(task_id) = &self.task_id {
            validate_identifier("task_id", task_id)?;
        }
        if let Some(turn_id) = &self.turn_id {
            validate_identifier("turn_id", turn_id)?;
        }
        self.identity.validate()?;
        self.request.validate()?;
        if let MemoryRequest::MaterializeContext {
            purpose: RecallPurpose::Handoff,
            binding,
            ..
        } = &self.request
        {
            if self.task_id.as_deref() != binding.task_id.as_deref() {
                return Err(ProtocolError::invalid(
                    "handoff envelope task_id must match binding.task_id",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn backend_context(&self) -> BackendRequestContext {
        BackendRequestContext {
            request_id: self.request_id.clone(),
            trace_id: self.trace_id.clone(),
            run_id: self.run_id.clone(),
            task_id: self.task_id.clone(),
            turn_id: self.turn_id.clone(),
            deadline_at_ms: self.deadline_at_ms,
            identity: self.identity.clone(),
        }
    }
}

/// Operation carried by a request envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "operation",
    content = "input",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MemoryRequest {
    /// Negotiates required and optional capabilities.
    Negotiate {
        /// Capabilities required by the caller.
        required: Vec<MemoryCapability>,
    },
    /// Opens or idempotently resumes a Runtime session.
    OpenSession {
        /// Runtime properties used for routing and invalidation.
        runtime: RuntimeContext,
    },
    /// Appends one bounded event.
    AppendEvent {
        /// Stable retry key independent from transport request identity.
        idempotency_key: String,
        /// Event to persist or project.
        event: MemoryEvent,
    },
    /// Materializes one token-bounded ContextView.
    MaterializeContext {
        /// Recall purpose selected by the Runtime adapter.
        purpose: RecallPurpose,
        /// Optional task and target binding for selection policy.
        binding: RecallBinding,
        /// Current task or user query.
        query: String,
        /// Hard admission budget.
        budget: ContextBudget,
    },
    /// Persists resumable task state and evidence references.
    CheckpointTask {
        /// Stable retry key independent from transport request identity.
        idempotency_key: String,
        /// Task state to commit.
        task: TaskState,
        /// Previously observed revision, or none when creating the task.
        expected_revision: Option<u64>,
        /// Evidence supporting the state.
        evidence: Vec<EvidenceRef>,
    },
    /// Explains selection and admission for an existing view.
    ExplainContext {
        /// ContextView identity returned by materialization.
        context_view_id: String,
    },
    /// Records an explicit or inferred usefulness outcome.
    ReportRecallOutcome {
        /// Stable retry key independent from transport request identity.
        idempotency_key: String,
        /// ContextView being evaluated.
        context_view_id: String,
        /// Items that the Runtime actually admitted into model context.
        admitted_item_ids: Vec<String>,
        /// Items the Runtime dropped after backend materialization.
        dropped_item_ids: Vec<String>,
        /// Evaluation outcome.
        outcome: FeedbackOutcome,
    },
    /// Removes a Memory-owned object.
    Forget {
        /// Object category used to select the correct identity scope.
        kind: MemoryObjectKind,
        /// Backend object identity to forget.
        memory_id: String,
    },
    /// Closes the Runtime session without deleting durable task state.
    CloseSession {
        /// Stable retry key independent from transport request identity.
        idempotency_key: String,
        /// Terminal session outcome.
        outcome: SessionOutcome,
    },
}

impl MemoryRequest {
    /// Returns the capability required to execute this request.
    pub fn required_capability(&self) -> Option<MemoryCapability> {
        match self {
            Self::Negotiate { .. } => None,
            Self::OpenSession { .. } | Self::CloseSession { .. } => Some(MemoryCapability::Session),
            Self::AppendEvent { .. } => Some(MemoryCapability::Capture),
            Self::MaterializeContext { .. } => Some(MemoryCapability::Recall),
            Self::CheckpointTask { .. } => Some(MemoryCapability::Checkpoint),
            Self::ExplainContext { .. } => Some(MemoryCapability::Explain),
            Self::ReportRecallOutcome { .. } => Some(MemoryCapability::Outcome),
            Self::Forget { .. } => Some(MemoryCapability::Forget),
        }
    }

    fn validate(&self) -> ProtocolResult<()> {
        match self {
            Self::Negotiate { required } => validate_items("required", required.len()),
            Self::OpenSession { runtime } => {
                validate_text("runtime", &runtime.runtime, MAX_ID_BYTES)?;
                validate_optional_text("runtime_version", runtime.runtime_version.as_deref())?;
                validate_optional_text("model", runtime.model.as_deref())?;
                validate_optional_text("platform", runtime.platform.as_deref())
            }
            Self::AppendEvent {
                idempotency_key,
                event,
            } => {
                validate_identifier("idempotency_key", idempotency_key)?;
                validate_identifier("event_id", &event.event_id)?;
                validate_identifier("event.source", &event.source)?;
                validate_text("event.summary", &event.summary, MAX_TEXT_BYTES)?;
                validate_optional_text("event.evidence_ref", event.evidence_ref.as_deref())
            }
            Self::MaterializeContext {
                purpose,
                binding,
                query,
                budget,
            } => {
                validate_text("query", query, MAX_TEXT_BYTES)?;
                if budget.max_tokens == 0 || budget.max_bytes == 0 || budget.max_items == 0 {
                    return Err(ProtocolError::invalid(
                        "context budget requires non-zero max_tokens, max_bytes, and max_items",
                    ));
                }
                if usize::from(budget.max_items) > MAX_ITEMS {
                    return Err(ProtocolError::invalid(format!(
                        "context budget max_items exceeds {MAX_ITEMS}"
                    )));
                }
                if budget.max_bytes > MAX_CONTEXT_BYTES || budget.max_tokens > MAX_CONTEXT_TOKENS {
                    return Err(ProtocolError::new(
                        ProtocolErrorCode::ResourceExhausted,
                        "context budget exceeds protocol limits",
                        false,
                    ));
                }
                validate_optional_identifier("binding.task_id", binding.task_id.as_deref())?;
                validate_optional_identifier(
                    "binding.target_agent_id",
                    binding.target_agent_id.as_deref(),
                )?;
                if matches!(purpose, RecallPurpose::Handoff) && binding.target_agent_id.is_none() {
                    return Err(ProtocolError::invalid(
                        "handoff recall requires binding.target_agent_id",
                    ));
                }
                if matches!(purpose, RecallPurpose::Handoff) && binding.task_id.is_none() {
                    return Err(ProtocolError::invalid(
                        "handoff recall requires binding.task_id",
                    ));
                }
                Ok(())
            }
            Self::CheckpointTask {
                idempotency_key,
                task,
                expected_revision,
                evidence,
            } => {
                validate_identifier("idempotency_key", idempotency_key)?;
                validate_identifier("task_id", &task.task_id)?;
                let required_revision = match expected_revision {
                    None => 1,
                    Some(revision) => revision.checked_add(1).ok_or_else(|| {
                        ProtocolError::invalid("expected_revision cannot advance beyond u64::MAX")
                    })?,
                };
                if task.revision != required_revision {
                    return Err(ProtocolError::invalid(format!(
                        "task revision must be {required_revision} for the supplied expected_revision"
                    )));
                }
                validate_text("task.goal", &task.goal, MAX_TEXT_BYTES)?;
                validate_optional_text("task.next_action", task.next_action.as_deref())?;
                validate_items("task.blockers", task.blockers.len())?;
                for blocker in &task.blockers {
                    validate_text("task.blocker", blocker, MAX_TEXT_BYTES)?;
                }
                validate_items("evidence", evidence.len())?;
                for item in evidence {
                    validate_identifier("evidence.provider", &item.provider)?;
                    validate_text("evidence.uri", &item.uri, MAX_TEXT_BYTES)?;
                    validate_optional_text("evidence.digest", item.digest.as_deref())?;
                    validate_text("evidence.summary", &item.summary, MAX_TEXT_BYTES)?;
                }
                let content_bytes = task.goal.len()
                    + task.next_action.as_ref().map_or(0, String::len)
                    + task.blockers.iter().map(String::len).sum::<usize>()
                    + evidence
                        .iter()
                        .map(|item| {
                            item.provider.len()
                                + item.uri.len()
                                + item.digest.as_ref().map_or(0, String::len)
                                + item.summary.len()
                        })
                        .sum::<usize>();
                if content_bytes > MAX_REQUEST_CONTENT_BYTES {
                    return Err(ProtocolError::new(
                        ProtocolErrorCode::ResourceExhausted,
                        "task checkpoint content exceeds protocol limits",
                        false,
                    ));
                }
                Ok(())
            }
            Self::ExplainContext { context_view_id } => {
                validate_identifier("context_view_id", context_view_id)?;
                Ok(())
            }
            Self::ReportRecallOutcome {
                idempotency_key,
                context_view_id,
                admitted_item_ids,
                dropped_item_ids,
                ..
            } => {
                validate_identifier("idempotency_key", idempotency_key)?;
                validate_identifier("context_view_id", context_view_id)?;
                validate_items("admitted_item_ids", admitted_item_ids.len())?;
                validate_items("dropped_item_ids", dropped_item_ids.len())?;
                for item_id in admitted_item_ids.iter().chain(dropped_item_ids) {
                    validate_identifier("outcome.item_id", item_id)?;
                }
                Ok(())
            }
            Self::Forget { memory_id, .. } => validate_identifier("memory_id", memory_id),
            Self::CloseSession {
                idempotency_key, ..
            } => validate_identifier("idempotency_key", idempotency_key),
        }
    }
}

/// Successful operation response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", content = "output", rename_all = "snake_case")]
pub enum MemoryResponse {
    /// Negotiated backend manifest.
    Negotiated {
        /// Backend identity and capabilities.
        manifest: BackendManifest,
    },
    /// Session was opened or resumed.
    SessionOpened {
        /// True when existing session state was resumed.
        resumed: bool,
    },
    /// Event was accepted or replayed idempotently.
    EventAccepted {
        /// Accepted Runtime event identity.
        event_id: String,
        /// True when this request replayed an existing event.
        replayed: bool,
        /// Durability reached before this response was emitted.
        durability: MemoryDurability,
    },
    /// Context was materialized within budget.
    ContextMaterialized {
        /// Final model-visible context.
        view: ContextView,
    },
    /// Task checkpoint was committed.
    TaskCheckpointed {
        /// Committed task identity.
        task_id: String,
        /// Committed projection revision.
        revision: u64,
        /// True when this request replayed an existing checkpoint.
        replayed: bool,
        /// Durability reached before this response was emitted.
        durability: MemoryDurability,
    },
    /// Recall trace was returned.
    ContextExplained {
        /// Explainable selection trace.
        trace: RecallTrace,
    },
    /// Feedback was recorded.
    FeedbackRecorded {
        /// True when this request replayed an existing outcome report.
        replayed: bool,
        /// Durability reached before this response was emitted.
        durability: MemoryDurability,
    },
    /// Forget operation completed.
    Forgotten {
        /// True when a matching object was removed.
        deleted: bool,
        /// Durability reached before this response was emitted.
        durability: MemoryDurability,
    },
    /// Session was closed.
    SessionClosed {
        /// True when this request replayed an existing close operation.
        replayed: bool,
    },
}

/// JSON response envelope carrying either an operation result or a safe error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MemoryWireResponse {
    /// Successful protocol response.
    Ok {
        /// Protocol major version used by the backend.
        protocol_version: u32,
        /// Request identity copied from the request envelope.
        request_id: String,
        /// Typed response payload.
        response: MemoryResponse,
    },
    /// Rejected or failed protocol response.
    Error {
        /// Protocol major version used by the backend.
        protocol_version: u32,
        /// Request identity when it passed envelope decoding.
        request_id: String,
        /// Stable safe error.
        error: ProtocolError,
    },
}

impl MemoryWireResponse {
    /// Returns the correlation identity carried by either wire response form.
    pub fn request_id(&self) -> &str {
        match self {
            Self::Ok { request_id, .. } | Self::Error { request_id, .. } => request_id,
        }
    }

    pub(crate) fn success(request_id: String, response: MemoryResponse) -> Self {
        Self::Ok {
            protocol_version: MEMORY_PROTOCOL_VERSION,
            request_id,
            response,
        }
    }

    /// Builds a wire-safe error response without exposing backend internals.
    pub fn error(request_id: impl Into<String>, error: ProtocolError) -> Self {
        Self::Error {
            protocol_version: MEMORY_PROTOCOL_VERSION,
            request_id: request_id.into(),
            error,
        }
    }
}

/// Stable protocol error classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolErrorCode {
    /// Requested protocol major version is unsupported.
    VersionUnsupported,
    /// Request violates a schema or bound.
    InvalidRequest,
    /// Backend does not implement the required capability.
    UnsupportedCapability,
    /// Authenticated principal cannot access the requested scope.
    Unauthorized,
    /// Backend is still recovering and cannot serve the operation yet.
    NotReady,
    /// Absolute request deadline elapsed before admission or completion.
    DeadlineExceeded,
    /// Runtime session has not been opened.
    SessionNotOpen,
    /// Requested object does not exist in the caller scope.
    NotFound,
    /// Idempotency or revision conflict.
    Conflict,
    /// Request exceeds a configured frame, item, byte, or storage limit.
    ResourceExhausted,
    /// Stored or transported content failed an integrity check.
    IntegrityFailed,
    /// Backend or provider is temporarily unavailable.
    Unavailable,
    /// Backend failed without exposing sensitive internals.
    Internal,
    /// Error code introduced by a newer compatible backend.
    #[serde(other)]
    Unknown,
}

/// Safe error returned across the plugin boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProtocolError {
    /// Stable machine-readable classification.
    pub code: ProtocolErrorCode,
    /// Redacted human-readable diagnostic.
    pub safe_message: String,
    /// Whether the caller may retry without changing the request.
    pub retryable: bool,
}

impl ProtocolError {
    /// Builds a protocol error with an explicit safe message and retry policy.
    pub fn new(code: ProtocolErrorCode, safe_message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            safe_message: safe_message.into(),
            retryable,
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(ProtocolErrorCode::InvalidRequest, message, false)
    }

    pub(crate) fn unsupported(capability: MemoryCapability) -> Self {
        Self::new(
            ProtocolErrorCode::UnsupportedCapability,
            format!("backend does not support {}", capability.as_str()),
            false,
        )
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.safe_message)
    }
}

impl std::error::Error for ProtocolError {}

/// Result returned by protocol validation and backend operations.
pub type ProtocolResult<T> = std::result::Result<T, ProtocolError>;

fn validate_identifier(field: &str, value: &str) -> ProtocolResult<()> {
    validate_text(field, value, MAX_ID_BYTES)?;
    if value.chars().any(char::is_control) {
        return Err(ProtocolError::invalid(format!(
            "{field} contains control characters"
        )));
    }
    Ok(())
}

fn validate_optional_text(field: &str, value: Option<&str>) -> ProtocolResult<()> {
    match value {
        Some(value) => validate_text(field, value, MAX_TEXT_BYTES),
        None => Ok(()),
    }
}

fn validate_optional_identifier(field: &str, value: Option<&str>) -> ProtocolResult<()> {
    match value {
        Some(value) => validate_identifier(field, value),
        None => Ok(()),
    }
}

fn validate_text(field: &str, value: &str, max_bytes: usize) -> ProtocolResult<()> {
    if value.trim().is_empty() {
        return Err(ProtocolError::invalid(format!("{field} must not be empty")));
    }
    if value.len() > max_bytes {
        return Err(ProtocolError::invalid(format!(
            "{field} exceeds {max_bytes} bytes"
        )));
    }
    Ok(())
}

fn validate_items(field: &str, len: usize) -> ProtocolResult<()> {
    if len > MAX_ITEMS {
        return Err(ProtocolError::invalid(format!(
            "{field} exceeds {MAX_ITEMS} items"
        )));
    }
    Ok(())
}
