//! Backend capability boundary and deterministic ephemeral conformance implementation.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    BackendManifest, BackendRequestContext, ContextBudget, ContextItem, ContextItemKind,
    ContextView, EvidenceRef, FeedbackOutcome, MEMORY_PROTOCOL_VERSION, MemoryAuthority,
    MemoryCapability, MemoryDurability, MemoryEvent, MemoryObjectKind, MemoryRequest,
    MemoryRequestEnvelope, MemoryResponse, MemoryWireResponse, ProtocolError, ProtocolErrorCode,
    ProtocolResult, RecallBinding, RecallDecision, RecallOutcomeReport, RecallPurpose, RecallTrace,
    RuntimeContext, SessionOutcome, TaskState,
};

/// Implementation-neutral backend interface consumed by wire servers and Runtime adapters.
pub trait MemoryBackend: Send + Sync {
    /// Describes this implementation and its supported capabilities.
    fn manifest(&self) -> BackendManifest;

    /// Opens a Runtime session, returning true when it was already open.
    fn open_session(
        &self,
        _context: &BackendRequestContext,
        _runtime: &RuntimeContext,
    ) -> ProtocolResult<bool> {
        Err(ProtocolError::unsupported(MemoryCapability::Session))
    }

    /// Appends one idempotent Runtime event, returning true for a safe replay.
    fn append_event(
        &self,
        _context: &BackendRequestContext,
        _idempotency_key: &str,
        _event: &MemoryEvent,
    ) -> ProtocolResult<bool> {
        Err(ProtocolError::unsupported(MemoryCapability::Capture))
    }

    /// Builds a bounded model-visible ContextView.
    fn materialize_context(
        &self,
        _context: &BackendRequestContext,
        _purpose: RecallPurpose,
        _binding: &RecallBinding,
        _query: &str,
        _budget: ContextBudget,
    ) -> ProtocolResult<ContextView> {
        Err(ProtocolError::unsupported(MemoryCapability::Recall))
    }

    /// Persists task state, returning true when the idempotency key was replayed.
    fn checkpoint_task(
        &self,
        _context: &BackendRequestContext,
        _idempotency_key: &str,
        _task: &TaskState,
        _expected_revision: Option<u64>,
        _evidence: &[EvidenceRef],
    ) -> ProtocolResult<bool> {
        Err(ProtocolError::unsupported(MemoryCapability::Checkpoint))
    }

    /// Explains selection and admission for an existing ContextView.
    fn explain_context(
        &self,
        _context: &BackendRequestContext,
        _context_view_id: &str,
    ) -> ProtocolResult<RecallTrace> {
        Err(ProtocolError::unsupported(MemoryCapability::Explain))
    }

    /// Records usefulness feedback, returning true for an equivalent replay.
    fn report_recall_outcome(
        &self,
        _context: &BackendRequestContext,
        _idempotency_key: &str,
        _context_view_id: &str,
        _admitted_item_ids: &[String],
        _dropped_item_ids: &[String],
        _outcome: FeedbackOutcome,
    ) -> ProtocolResult<bool> {
        Err(ProtocolError::unsupported(MemoryCapability::Outcome))
    }

    /// Removes a Memory-owned object in the caller scope.
    fn forget(
        &self,
        _context: &BackendRequestContext,
        _kind: MemoryObjectKind,
        _memory_id: &str,
    ) -> ProtocolResult<bool> {
        Err(ProtocolError::unsupported(MemoryCapability::Forget))
    }

    /// Closes a Runtime session without deleting durable task state.
    fn close_session(
        &self,
        _context: &BackendRequestContext,
        _idempotency_key: &str,
        _outcome: SessionOutcome,
    ) -> ProtocolResult<bool> {
        Err(ProtocolError::unsupported(MemoryCapability::Session))
    }
}

/// Validates and dispatches one request while preserving its request identity.
pub fn dispatch<B: MemoryBackend + ?Sized>(
    backend: &B,
    envelope: MemoryRequestEnvelope,
) -> MemoryWireResponse {
    let request_id = envelope.request_id.clone();
    if let Err(error) = envelope.validate() {
        return MemoryWireResponse::error(request_id, error);
    }
    if envelope
        .deadline_at_ms
        .is_some_and(|deadline| deadline <= now_ms())
    {
        return MemoryWireResponse::error(
            request_id,
            ProtocolError::new(
                ProtocolErrorCode::DeadlineExceeded,
                "memory request deadline has elapsed",
                false,
            ),
        );
    }

    let manifest = backend.manifest();
    if manifest.protocol_version != MEMORY_PROTOCOL_VERSION {
        return MemoryWireResponse::error(
            request_id,
            ProtocolError::new(
                ProtocolErrorCode::VersionUnsupported,
                "backend advertises an incompatible protocol version",
                false,
            ),
        );
    }
    if let Some(required) = envelope.request.required_capability() {
        if !manifest.capabilities.contains(&required) {
            return MemoryWireResponse::error(request_id, ProtocolError::unsupported(required));
        }
    }

    let context = envelope.backend_context();
    let durability = manifest.durability;
    let response = match &envelope.request {
        MemoryRequest::Negotiate { required } => {
            if let Some(missing) = required
                .iter()
                .find(|capability| !manifest.capabilities.contains(capability))
            {
                Err(ProtocolError::unsupported(missing.clone()))
            } else {
                Ok(MemoryResponse::Negotiated { manifest })
            }
        }
        MemoryRequest::OpenSession { runtime } => backend
            .open_session(&context, runtime)
            .map(|resumed| MemoryResponse::SessionOpened { resumed }),
        MemoryRequest::AppendEvent {
            idempotency_key,
            event,
        } => backend
            .append_event(&context, idempotency_key, event)
            .map(|replayed| MemoryResponse::EventAccepted {
                event_id: event.event_id.clone(),
                replayed,
                durability,
            }),
        MemoryRequest::MaterializeContext {
            purpose,
            binding,
            query,
            budget,
        } => backend
            .materialize_context(&context, *purpose, binding, query, *budget)
            .and_then(|view| {
                view.validate_backend_result(query, &context.trace_id, *budget)?;
                Ok(view)
            })
            .map(|view| MemoryResponse::ContextMaterialized { view }),
        MemoryRequest::CheckpointTask {
            idempotency_key,
            task,
            expected_revision,
            evidence,
        } => backend
            .checkpoint_task(
                &context,
                idempotency_key,
                task,
                *expected_revision,
                evidence,
            )
            .map(|replayed| MemoryResponse::TaskCheckpointed {
                task_id: task.task_id.clone(),
                revision: task.revision,
                replayed,
                durability,
            }),
        MemoryRequest::ExplainContext { context_view_id } => backend
            .explain_context(&context, context_view_id)
            .and_then(|trace| {
                trace.validate_backend_result(context_view_id, &context.trace_id)?;
                Ok(trace)
            })
            .map(|trace| MemoryResponse::ContextExplained { trace }),
        MemoryRequest::ReportRecallOutcome {
            idempotency_key,
            context_view_id,
            admitted_item_ids,
            dropped_item_ids,
            outcome,
        } => backend
            .report_recall_outcome(
                &context,
                idempotency_key,
                context_view_id,
                admitted_item_ids,
                dropped_item_ids,
                *outcome,
            )
            .map(|replayed| MemoryResponse::FeedbackRecorded {
                replayed,
                durability,
            }),
        MemoryRequest::Forget { kind, memory_id } => backend
            .forget(&context, *kind, memory_id)
            .map(|deleted| MemoryResponse::Forgotten {
                deleted,
                durability,
            }),
        MemoryRequest::CloseSession {
            idempotency_key,
            outcome,
        } => backend
            .close_session(&context, idempotency_key, *outcome)
            .map(|replayed| MemoryResponse::SessionClosed { replayed }),
    };

    if context
        .deadline_at_ms
        .is_some_and(|deadline| deadline <= now_ms())
    {
        return MemoryWireResponse::error(
            request_id,
            ProtocolError::new(
                ProtocolErrorCode::DeadlineExceeded,
                "memory request deadline elapsed during backend execution",
                false,
            ),
        );
    }

    match response {
        Ok(response) => MemoryWireResponse::success(request_id, response),
        Err(error) => MemoryWireResponse::error(request_id, error),
    }
}

/// Deterministic process-local backend used by conformance tests and adapter development.
#[derive(Debug, Clone, Default)]
pub struct EphemeralMemoryBackend {
    state: Arc<Mutex<EphemeralState>>,
}

#[derive(Debug, Default)]
struct EphemeralState {
    sessions: HashSet<String>,
    event_keys: HashMap<(String, String), String>,
    events: HashMap<(String, String), MemoryEvent>,
    tasks: HashMap<(String, String), StoredTask>,
    checkpoint_keys: HashMap<(String, String), StoredTask>,
    close_keys: HashMap<(String, String), SessionOutcome>,
    views: HashMap<(String, String), StoredView>,
    view_order: VecDeque<(String, String)>,
    next_view: u64,
    revision: u64,
}

#[derive(Debug, Clone)]
struct StoredTask {
    task: TaskState,
    evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone)]
struct StoredView {
    trace: RecallTrace,
    outcome_keys: HashMap<String, RecallOutcomeReport>,
}

const MAX_EPHEMERAL_SESSIONS: usize = 256;
const MAX_EPHEMERAL_EVENTS: usize = 4096;
const MAX_EPHEMERAL_TASKS: usize = 1024;
const MAX_EPHEMERAL_VIEWS: usize = 128;
const MAX_TRACE_DECISIONS: usize = 256;
const MAX_IDEMPOTENCY_ALIASES: usize = 8;
const MAX_CHECKPOINT_KEYS: usize = 4096;
const MAX_CLOSE_KEYS: usize = 4096;

impl EphemeralMemoryBackend {
    fn state(&self) -> ProtocolResult<MutexGuard<'_, EphemeralState>> {
        self.state.lock().map_err(|_| {
            ProtocolError::new(
                ProtocolErrorCode::Unavailable,
                "ephemeral backend state is unavailable",
                true,
            )
        })
    }

    fn require_session(
        state: &EphemeralState,
        context: &BackendRequestContext,
    ) -> ProtocolResult<()> {
        if state.sessions.contains(&context.identity.session_key()) {
            Ok(())
        } else {
            Err(ProtocolError::new(
                ProtocolErrorCode::SessionNotOpen,
                "memory session is not open",
                false,
            ))
        }
    }

    fn advance_revision(state: &mut EphemeralState) -> ProtocolResult<u64> {
        state.revision = state.revision.checked_add(1).ok_or_else(|| {
            ProtocolError::new(
                ProtocolErrorCode::ResourceExhausted,
                "ephemeral backend revision space is exhausted",
                false,
            )
        })?;
        Ok(state.revision)
    }
}

impl MemoryBackend for EphemeralMemoryBackend {
    fn manifest(&self) -> BackendManifest {
        BackendManifest {
            backend_id: "ephemeral".to_string(),
            display_name: "Ephemeral conformance backend".to_string(),
            protocol_version: MEMORY_PROTOCOL_VERSION,
            capabilities: vec![
                MemoryCapability::Session,
                MemoryCapability::Capture,
                MemoryCapability::Recall,
                MemoryCapability::Checkpoint,
                MemoryCapability::Explain,
                MemoryCapability::Outcome,
                MemoryCapability::Forget,
            ],
            durability: MemoryDurability::ProcessLocal,
        }
    }

    fn open_session(
        &self,
        context: &BackendRequestContext,
        _runtime: &RuntimeContext,
    ) -> ProtocolResult<bool> {
        let mut state = self.state()?;
        let session_key = context.identity.session_key();
        if state.sessions.contains(&session_key) {
            return Ok(true);
        }
        if state.sessions.len() >= MAX_EPHEMERAL_SESSIONS {
            return Err(resource_exhausted("session"));
        }
        state.sessions.insert(session_key);
        Ok(false)
    }

    fn append_event(
        &self,
        context: &BackendRequestContext,
        idempotency_key: &str,
        event: &MemoryEvent,
    ) -> ProtocolResult<bool> {
        let mut state = self.state()?;
        Self::require_session(&state, context)?;
        let session_key = context.identity.session_key();
        let key = (session_key.clone(), idempotency_key.to_string());
        if let Some(event_id) = state.event_keys.get(&key) {
            let existing = state
                .events
                .get(&(session_key.clone(), event_id.clone()))
                .ok_or_else(integrity_failed)?;
            if existing == event {
                return Ok(true);
            }
            return Err(ProtocolError::new(
                ProtocolErrorCode::Conflict,
                "idempotency key was already used for different event content",
                false,
            ));
        }
        let event_key = (session_key, event.event_id.clone());
        if let Some(existing) = state.events.get(&event_key) {
            if existing != event {
                return Err(ProtocolError::new(
                    ProtocolErrorCode::Conflict,
                    "event identity was already used for different content",
                    false,
                ));
            }
            let aliases = state
                .event_keys
                .iter()
                .filter(|((scope, _), event_id)| {
                    scope == &event_key.0 && *event_id == &event.event_id
                })
                .count();
            if aliases >= MAX_IDEMPOTENCY_ALIASES {
                return Err(resource_exhausted("event idempotency alias"));
            }
            state.event_keys.insert(key, event.event_id.clone());
            return Ok(true);
        }
        if state.events.len() >= MAX_EPHEMERAL_EVENTS {
            return Err(resource_exhausted("event"));
        }
        state.event_keys.insert(key, event.event_id.clone());
        state.events.insert(event_key, event.clone());
        Self::advance_revision(&mut state)?;
        Ok(false)
    }

    fn materialize_context(
        &self,
        context: &BackendRequestContext,
        _purpose: RecallPurpose,
        binding: &RecallBinding,
        query: &str,
        budget: ContextBudget,
    ) -> ProtocolResult<ContextView> {
        let mut state = self.state()?;
        Self::require_session(&state, context)?;
        let workspace_key = context.identity.workspace_key();
        let mut tasks: Vec<StoredTask> = state
            .tasks
            .iter()
            .filter(|((scope, task_id), _)| {
                scope == &workspace_key
                    && binding
                        .task_id
                        .as_ref()
                        .is_none_or(|requested| requested == task_id)
            })
            .map(|(_, task)| task.clone())
            .collect();
        tasks.sort_by(|left, right| left.task.task_id.cmp(&right.task.task_id));
        let trace_truncated = tasks.len() > MAX_TRACE_DECISIONS;

        let mut items = Vec::new();
        let mut decisions = Vec::new();
        let mut total_tokens = 0_u32;
        let mut total_bytes = 0_u32;
        let mut truncated = false;
        for (index, stored) in tasks.into_iter().take(MAX_TRACE_DECISIONS).enumerate() {
            let content = format_task(&stored.task, &stored.evidence);
            let token_estimate = estimate_tokens(&content);
            let byte_estimate = u32::try_from(content.len()).unwrap_or(u32::MAX);
            let within_items = items.len() < usize::from(budget.max_items);
            let within_tokens = total_tokens.saturating_add(token_estimate) <= budget.max_tokens;
            let within_bytes = total_bytes.saturating_add(byte_estimate) <= budget.max_bytes;
            let admitted = within_items && within_tokens && within_bytes;
            truncated |= !admitted;
            decisions.push(RecallDecision {
                item_id: stored.task.task_id.clone(),
                admitted,
                reason: if admitted {
                    "workspace task checkpoint".to_string()
                } else {
                    "context budget exhausted".to_string()
                },
                rank: u32::try_from(index + 1).unwrap_or(u32::MAX),
                score: 1.0,
            });
            if admitted {
                total_tokens = total_tokens.saturating_add(token_estimate);
                total_bytes = total_bytes.saturating_add(byte_estimate);
                items.push(ContextItem {
                    item_id: stored.task.task_id.clone(),
                    revision: Some(stored.task.revision),
                    kind: ContextItemKind::TaskState,
                    content,
                    source_ref: format!("task:{}", stored.task.task_id),
                    authority: MemoryAuthority::Verified,
                    token_estimate,
                    reason: "workspace task checkpoint".to_string(),
                    stale: false,
                    score: 1.0,
                });
            }
        }

        if trace_truncated {
            truncated = true;
        }
        state.next_view = state.next_view.checked_add(1).ok_or_else(|| {
            ProtocolError::new(
                ProtocolErrorCode::ResourceExhausted,
                "ephemeral context identity space is exhausted",
                false,
            )
        })?;
        let context_view_id = format!("ctx-{}", state.next_view);
        let view = ContextView {
            context_view_id: context_view_id.clone(),
            trace_id: context.trace_id.clone(),
            snapshot_revision: state.revision,
            query: query.to_string(),
            items,
            total_tokens,
            total_bytes,
            effective_strategy: "ephemeral_task_state".to_string(),
            degraded: false,
            truncated,
            created_at_ms: now_ms(),
        };
        let trace = RecallTrace {
            context_view_id: context_view_id.clone(),
            trace_id: context.trace_id.clone(),
            response_trace_id: context.trace_id.clone(),
            backend_id: "ephemeral".to_string(),
            decisions,
            degraded: false,
            degradation_reason: None,
            outcome_report: None,
        };
        while state.views.len() >= MAX_EPHEMERAL_VIEWS {
            if let Some(oldest) = state.view_order.pop_front() {
                state.views.remove(&oldest);
            }
        }
        let view_key = (context.identity.session_key(), context_view_id);
        state.view_order.push_back(view_key.clone());
        state.views.insert(
            view_key,
            StoredView {
                trace,
                outcome_keys: HashMap::new(),
            },
        );
        Ok(view)
    }

    fn checkpoint_task(
        &self,
        context: &BackendRequestContext,
        idempotency_key: &str,
        task: &TaskState,
        expected_revision: Option<u64>,
        evidence: &[EvidenceRef],
    ) -> ProtocolResult<bool> {
        let mut state = self.state()?;
        Self::require_session(&state, context)?;
        let workspace_key = context.identity.workspace_key();
        let checkpoint_key = (workspace_key.clone(), idempotency_key.to_string());
        let proposed = StoredTask {
            task: task.clone(),
            evidence: evidence.to_vec(),
        };
        if let Some(existing) = state.checkpoint_keys.get(&checkpoint_key) {
            if existing.task == proposed.task && existing.evidence == proposed.evidence {
                return Ok(true);
            }
            return Err(ProtocolError::new(
                ProtocolErrorCode::Conflict,
                "checkpoint idempotency key was already used for different content",
                false,
            ));
        }
        let key = (workspace_key, task.task_id.clone());
        let actual_revision = state.tasks.get(&key).map(|stored| stored.task.revision);
        if actual_revision != expected_revision {
            return Err(ProtocolError::new(
                ProtocolErrorCode::Conflict,
                "task checkpoint revision does not match the current projection",
                false,
            ));
        }
        if actual_revision.is_none() && state.tasks.len() >= MAX_EPHEMERAL_TASKS {
            return Err(resource_exhausted("task"));
        }
        if state.checkpoint_keys.len() >= MAX_CHECKPOINT_KEYS {
            return Err(resource_exhausted("checkpoint key"));
        }
        state.tasks.insert(key, proposed.clone());
        state.checkpoint_keys.insert(checkpoint_key, proposed);
        Self::advance_revision(&mut state)?;
        Ok(false)
    }

    fn explain_context(
        &self,
        context: &BackendRequestContext,
        context_view_id: &str,
    ) -> ProtocolResult<RecallTrace> {
        let state = self.state()?;
        Self::require_session(&state, context)?;
        state
            .views
            .get(&(context.identity.session_key(), context_view_id.to_string()))
            .map(|stored| {
                let mut trace = stored.trace.clone();
                trace.response_trace_id = context.trace_id.clone();
                trace
            })
            .ok_or_else(|| {
                ProtocolError::new(
                    ProtocolErrorCode::NotFound,
                    "context view was not found in the caller scope",
                    false,
                )
            })
    }

    fn report_recall_outcome(
        &self,
        context: &BackendRequestContext,
        idempotency_key: &str,
        context_view_id: &str,
        admitted_item_ids: &[String],
        dropped_item_ids: &[String],
        outcome: FeedbackOutcome,
    ) -> ProtocolResult<bool> {
        let mut state = self.state()?;
        Self::require_session(&state, context)?;
        let key = (context.identity.session_key(), context_view_id.to_string());
        let stored = state.views.get_mut(&key).ok_or_else(|| {
            ProtocolError::new(
                ProtocolErrorCode::NotFound,
                "context view was not found in the caller scope",
                false,
            )
        })?;
        let reported: Vec<&String> = admitted_item_ids.iter().chain(dropped_item_ids).collect();
        let unique: HashSet<&String> = reported.iter().copied().collect();
        let known: HashSet<&str> = stored
            .trace
            .decisions
            .iter()
            .filter(|decision| decision.admitted)
            .map(|decision| decision.item_id.as_str())
            .collect();
        let overlaps = admitted_item_ids
            .iter()
            .any(|item_id| dropped_item_ids.contains(item_id));
        let has_duplicates = unique.len() != reported.len();
        let has_unknown = reported
            .iter()
            .any(|item_id| !known.contains(item_id.as_str()));
        let incomplete = unique.len() != known.len();
        if overlaps || has_duplicates || has_unknown || incomplete {
            return Err(ProtocolError::new(
                ProtocolErrorCode::Conflict,
                "recall outcome contains overlapping, duplicate, or unknown item identities",
                false,
            ));
        }
        if known.is_empty() && matches!(outcome, FeedbackOutcome::Useful) {
            return Err(ProtocolError::new(
                ProtocolErrorCode::Conflict,
                "empty recall cannot be reported as useful",
                false,
            ));
        }
        let report = RecallOutcomeReport {
            admitted_item_ids: admitted_item_ids.to_vec(),
            dropped_item_ids: dropped_item_ids.to_vec(),
            outcome,
        };
        if let Some(existing) = stored.outcome_keys.get(idempotency_key) {
            if existing == &report {
                return Ok(true);
            }
            return Err(ProtocolError::new(
                ProtocolErrorCode::Conflict,
                "outcome idempotency key was already used for a different report",
                false,
            ));
        }
        if let Some(existing) = &stored.trace.outcome_report {
            if existing != &report {
                return Err(ProtocolError::new(
                    ProtocolErrorCode::Conflict,
                    "context view already has a different outcome report",
                    false,
                ));
            }
        }
        let replayed = stored.trace.outcome_report.is_some();
        if stored.outcome_keys.len() >= MAX_IDEMPOTENCY_ALIASES {
            return Err(resource_exhausted("outcome idempotency alias"));
        }
        stored
            .outcome_keys
            .insert(idempotency_key.to_string(), report.clone());
        stored.trace.outcome_report = Some(report);
        Ok(replayed)
    }

    fn forget(
        &self,
        context: &BackendRequestContext,
        kind: MemoryObjectKind,
        memory_id: &str,
    ) -> ProtocolResult<bool> {
        let mut state = self.state()?;
        Self::require_session(&state, context)?;
        let deleted = match kind {
            MemoryObjectKind::Task => {
                let workspace_key = context.identity.workspace_key();
                let deleted = state
                    .tasks
                    .remove(&(workspace_key.clone(), memory_id.to_string()))
                    .is_some();
                if deleted {
                    state.checkpoint_keys.retain(|(scope, _), stored| {
                        scope != &workspace_key || stored.task.task_id != memory_id
                    });
                }
                deleted
            }
            MemoryObjectKind::Event => {
                let session_key = context.identity.session_key();
                let deleted = state
                    .events
                    .remove(&(session_key.clone(), memory_id.to_string()))
                    .is_some();
                if deleted {
                    state.event_keys.retain(|(scope, _), event_id| {
                        scope != &session_key || event_id != memory_id
                    });
                }
                deleted
            }
            MemoryObjectKind::ContextView => {
                let key = (context.identity.session_key(), memory_id.to_string());
                state.view_order.retain(|view_key| view_key != &key);
                state.views.remove(&key).is_some()
            }
        };
        if deleted {
            Self::advance_revision(&mut state)?;
        }
        Ok(deleted)
    }

    fn close_session(
        &self,
        context: &BackendRequestContext,
        idempotency_key: &str,
        outcome: SessionOutcome,
    ) -> ProtocolResult<bool> {
        let mut state = self.state()?;
        let session_key = context.identity.session_key();
        let close_key = (session_key.clone(), idempotency_key.to_string());
        if let Some(existing) = state.close_keys.get(&close_key) {
            if existing == &outcome {
                return Ok(true);
            }
            return Err(ProtocolError::new(
                ProtocolErrorCode::Conflict,
                "close idempotency key was already used for a different outcome",
                false,
            ));
        }
        Self::require_session(&state, context)?;
        if state.close_keys.len() >= MAX_CLOSE_KEYS {
            return Err(resource_exhausted("session close key"));
        }
        state.close_keys.insert(close_key, outcome);
        state.sessions.remove(&session_key);
        Ok(false)
    }
}

fn estimate_tokens(content: &str) -> u32 {
    let bytes = u32::try_from(content.len()).unwrap_or(u32::MAX);
    bytes.saturating_add(3) / 4
}

fn resource_exhausted(resource: &str) -> ProtocolError {
    ProtocolError::new(
        ProtocolErrorCode::ResourceExhausted,
        format!("ephemeral backend {resource} capacity is exhausted"),
        false,
    )
}

fn integrity_failed() -> ProtocolError {
    ProtocolError::new(
        ProtocolErrorCode::IntegrityFailed,
        "ephemeral idempotency index is inconsistent",
        false,
    )
}

fn format_task(task: &TaskState, evidence: &[EvidenceRef]) -> String {
    let mut lines = vec![format!("Goal: {}", task.goal)];
    if let Some(next_action) = &task.next_action {
        lines.push(format!("Next action: {next_action}"));
    }
    for blocker in &task.blockers {
        lines.push(format!("Blocker: {blocker}"));
    }
    for item in evidence {
        lines.push(format!("Evidence [{}]: {}", item.provider, item.summary));
    }
    lines.join("\n")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
