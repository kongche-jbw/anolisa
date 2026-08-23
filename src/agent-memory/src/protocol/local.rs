//! Durable SQLite implementation of the typed Memory backend contract.

use std::collections::HashSet;
use std::env;
use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::knowledge::{
    KnowledgeError, KnowledgeItem, KnowledgeProvider, KnowledgeQuery, KnowledgeSelector,
};
use crate::safety::{contains_secrets, looks_like_prompt_injection, redact_secrets};

use super::{
    BackendManifest, BackendRequestContext, ContextBudget, ContextItem, ContextItemKind,
    ContextView, EvidenceRef, FeedbackOutcome, MEMORY_PROTOCOL_VERSION, MemoryAuthority,
    MemoryBackend, MemoryCapability, MemoryDurability, MemoryEvent, MemoryObjectKind,
    ProtocolError, ProtocolErrorCode, ProtocolResult, RecallBinding, RecallDecision,
    RecallOutcomeReport, RecallPurpose, RecallTrace, RuntimeContext, SessionOutcome, TaskState,
};

const SCHEMA_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SESSIONS: u64 = 16_384;
const MAX_EVENTS: u64 = 1_000_000;
const MAX_TASKS: u64 = 100_000;
const MAX_VIEWS: u64 = 65_536;
const MAX_IDEMPOTENCY_ROWS: u64 = 2_000_000;
const MAX_EVENT_ALIASES: u64 = 8;
const MAX_OUTCOME_ALIASES: u64 = 8;
const MAX_TRACE_DECISIONS: usize = 256;
const MAX_CONTEXT_BYTES: u32 = 512 * 1024;
const MAX_CONTEXT_TOKENS: u32 = 128 * 1024;
const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_EVENT_SCAN: usize = 1024;
const VIEW_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const SESSION_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
const CAPACITY_PRUNE_BATCH: u64 = 1_024;
const MAX_KNOWLEDGE_DOCUMENT_BYTES: usize = 1_024;
const DEFAULT_KNOWLEDGE_EXCERPT_BYTES: usize = 4 * 1_024;
const DEFAULT_KNOWLEDGE_ITEMS: u16 = 4;

const SCHEMA_V1: &str = r#"
CREATE TABLE backend_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL CHECK (revision >= 0),
    next_view INTEGER NOT NULL CHECK (next_view >= 0)
);
INSERT INTO backend_state(singleton, revision, next_view) VALUES (1, 0, 0);

CREATE TABLE sessions (
    session_scope TEXT PRIMARY KEY,
    workspace_scope TEXT NOT NULL,
    runtime_json BLOB NOT NULL,
    opened_at_ms INTEGER NOT NULL,
    closed_outcome_json BLOB,
    closed_at_ms INTEGER
);
CREATE INDEX sessions_workspace_idx ON sessions(workspace_scope, session_scope);

CREATE TABLE events (
    session_scope TEXT NOT NULL,
    event_id TEXT NOT NULL,
    observed_at_ms INTEGER NOT NULL,
    event_json BLOB NOT NULL,
    PRIMARY KEY(session_scope, event_id),
    FOREIGN KEY(session_scope) REFERENCES sessions(session_scope) ON DELETE CASCADE
);
CREATE INDEX events_scope_time_idx ON events(session_scope, observed_at_ms, event_id);
CREATE TABLE event_idempotency (
    session_scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    event_id TEXT NOT NULL,
    event_json BLOB NOT NULL,
    PRIMARY KEY(session_scope, idempotency_key),
    FOREIGN KEY(session_scope, event_id)
        REFERENCES events(session_scope, event_id) ON DELETE CASCADE
);

CREATE TABLE tasks (
    workspace_scope TEXT NOT NULL,
    task_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    task_json BLOB NOT NULL,
    evidence_json BLOB NOT NULL,
    PRIMARY KEY(workspace_scope, task_id)
);
CREATE INDEX tasks_scope_revision_idx ON tasks(workspace_scope, revision, task_id);
CREATE TABLE checkpoint_idempotency (
    workspace_scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    task_id TEXT NOT NULL,
    task_json BLOB NOT NULL,
    evidence_json BLOB NOT NULL,
    PRIMARY KEY(workspace_scope, idempotency_key),
    FOREIGN KEY(workspace_scope, task_id)
        REFERENCES tasks(workspace_scope, task_id) ON DELETE CASCADE
);

CREATE TABLE views (
    session_scope TEXT NOT NULL,
    view_id TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    view_json BLOB NOT NULL,
    trace_json BLOB NOT NULL,
    PRIMARY KEY(session_scope, view_id),
    FOREIGN KEY(session_scope) REFERENCES sessions(session_scope) ON DELETE CASCADE
);
CREATE INDEX views_scope_time_idx ON views(session_scope, created_at_ms, view_id);
CREATE TABLE outcome_idempotency (
    session_scope TEXT NOT NULL,
    view_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    report_json BLOB NOT NULL,
    PRIMARY KEY(session_scope, view_id, idempotency_key),
    FOREIGN KEY(session_scope, view_id)
        REFERENCES views(session_scope, view_id) ON DELETE CASCADE
);

CREATE TABLE close_idempotency (
    session_scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    outcome_json BLOB NOT NULL,
    PRIMARY KEY(session_scope, idempotency_key),
    FOREIGN KEY(session_scope) REFERENCES sessions(session_scope) ON DELETE CASCADE
);
"#;

/// Capacity and storage counters exposed without revealing the database path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalMemoryStats {
    /// Allocated SQLite bytes excluding pages currently on the freelist.
    pub logical_bytes: u64,
    /// On-disk bytes across the database, WAL, and shared-memory sidecar.
    pub physical_bytes: u64,
    /// Stored session rows, including closed sessions retained for replay.
    pub session_count: u64,
    /// Stored immutable event rows.
    pub event_count: u64,
    /// Stored current task projections.
    pub task_count: u64,
    /// Stored ContextView and RecallTrace pairs.
    pub view_count: u64,
}

/// SQLite-backed Memory authority with transactionally durable acknowledgements.
pub struct LocalMemoryBackend {
    connection: Mutex<Connection>,
    path: PathBuf,
    knowledge: Option<KnowledgeProviderBinding>,
}

/// Task-policy binding between the local broker and one replaceable provider.
#[derive(Clone)]
pub struct KnowledgeProviderBinding {
    provider: Arc<dyn KnowledgeProvider>,
    document_id: String,
    max_excerpt_bytes: usize,
    max_items: u16,
}

impl fmt::Debug for KnowledgeProviderBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KnowledgeProviderBinding")
            .field("provider", &"<replaceable>")
            .field("document_id", &self.document_id)
            .field("max_excerpt_bytes", &self.max_excerpt_bytes)
            .field("max_items", &self.max_items)
            .finish()
    }
}

impl KnowledgeProviderBinding {
    /// Creates a focused document binding with conservative context limits.
    ///
    /// # Errors
    ///
    /// Returns an invalid-request error when the logical document identity is
    /// empty or exceeds the provider-neutral boundary.
    pub fn new(
        provider: Arc<dyn KnowledgeProvider>,
        document_id: impl Into<String>,
    ) -> ProtocolResult<Self> {
        let document_id = document_id.into();
        if document_id.trim().is_empty() || document_id.len() > MAX_KNOWLEDGE_DOCUMENT_BYTES {
            return Err(invalid("knowledge document identity is invalid"));
        }
        Ok(Self {
            provider,
            document_id,
            max_excerpt_bytes: DEFAULT_KNOWLEDGE_EXCERPT_BYTES,
            max_items: DEFAULT_KNOWLEDGE_ITEMS,
        })
    }
}

impl fmt::Debug for LocalMemoryBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalMemoryBackend")
            .field("path", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Resolves the default local database without deriving identity from environment data.
pub fn default_local_memory_path() -> ProtocolResult<PathBuf> {
    if let Some(path) = env::var_os("ANOLISA_MEMORY_DB") {
        if path.is_empty() {
            return Err(invalid("local memory database override must not be empty"));
        }
        return Ok(PathBuf::from(path));
    }
    let state = dirs::state_dir().ok_or_else(|| {
        ProtocolError::new(
            ProtocolErrorCode::Unavailable,
            "local state directory is unavailable",
            false,
        )
    })?;
    Ok(state
        .join("anolisa")
        .join("agent-memory")
        .join("memory-v1.sqlite3"))
}

impl LocalMemoryBackend {
    /// Opens or creates a private schema-v1 database.
    ///
    /// The parent directory must be private to its owner. New parents are
    /// created as `0700`; the database is always restricted to `0600`.
    pub fn open(path: impl AsRef<Path>) -> ProtocolResult<Self> {
        Self::open_inner(path.as_ref(), None)
    }

    /// Opens a local backend with one task-policy-selected knowledge provider.
    ///
    /// The provider remains optional and replaceable. Provider failures mark
    /// the resulting ContextView degraded while local memory remains usable.
    pub fn open_with_knowledge(
        path: impl AsRef<Path>,
        binding: KnowledgeProviderBinding,
    ) -> ProtocolResult<Self> {
        Self::open_inner(path.as_ref(), Some(binding))
    }

    fn open_inner(
        path: &Path,
        knowledge: Option<KnowledgeProviderBinding>,
    ) -> ProtocolResult<Self> {
        let path = path.to_path_buf();
        prepare_private_path(&path)?;
        let mut connection = Connection::open(&path).map_err(|error| sql_error(&error))?;
        fs::set_permissions(&path, Permissions::from_mode(0o600)).map_err(|_| io_error())?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| sql_error(&error))?;
        // Establish the schema under an IMMEDIATE transaction before asking
        // concurrent first-open connections to switch the journal mode.
        // SQLite does not consistently honor the busy handler while changing
        // journal_mode on a brand-new database.
        initialize_schema(&mut connection)?;
        configure(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            path,
            knowledge,
        })
    }

    /// Returns bounded capacity counters and SQLite logical/physical size.
    pub fn stats(&self) -> ProtocolResult<LocalMemoryStats> {
        let connection = self.connection()?;
        let page_size = pragma_u64(&connection, "page_size")?;
        let page_count = pragma_u64(&connection, "page_count")?;
        let freelist_count = pragma_u64(&connection, "freelist_count")?;
        let logical_bytes = page_count
            .saturating_sub(freelist_count)
            .saturating_mul(page_size);
        let physical_bytes = physical_size(&self.path)?;
        Ok(LocalMemoryStats {
            logical_bytes,
            physical_bytes,
            session_count: table_count(&connection, "SELECT COUNT(*) FROM sessions")?,
            event_count: table_count(&connection, "SELECT COUNT(*) FROM events")?,
            task_count: table_count(&connection, "SELECT COUNT(*) FROM tasks")?,
            view_count: table_count(&connection, "SELECT COUNT(*) FROM views")?,
        })
    }

    fn connection(&self) -> ProtocolResult<MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| {
            ProtocolError::new(
                ProtocolErrorCode::Unavailable,
                "local memory database is unavailable",
                true,
            )
        })
    }
}

impl MemoryBackend for LocalMemoryBackend {
    fn manifest(&self) -> BackendManifest {
        let mut capabilities = vec![
            MemoryCapability::Session,
            MemoryCapability::Capture,
            MemoryCapability::Recall,
            MemoryCapability::Checkpoint,
            MemoryCapability::Explain,
            MemoryCapability::Outcome,
            MemoryCapability::Forget,
        ];
        if self.knowledge.is_some() {
            capabilities.push(MemoryCapability::Knowledge);
        }
        BackendManifest {
            backend_id: "local-sqlite-v1".to_string(),
            display_name: "Local SQLite memory".to_string(),
            protocol_version: MEMORY_PROTOCOL_VERSION,
            capabilities,
            durability: MemoryDurability::Durable,
        }
    }

    fn open_session(
        &self,
        context: &BackendRequestContext,
        runtime: &RuntimeContext,
    ) -> ProtocolResult<bool> {
        let runtime_json = encode(runtime)?;
        ensure_record_bytes(runtime_json.len())?;
        let opened_at_ms = as_sql_u64(now_ms())?;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        prune_expired(&transaction, now_ms())?;
        let session_scope = context.identity.session_key();
        let existed = transaction
            .query_row(
                "SELECT 1 FROM sessions WHERE session_scope = ?1",
                params![session_scope],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| sql_error(&error))?
            .is_some();
        if !existed {
            ensure_session_capacity(&transaction)?;
            transaction
                .execute(
                    "INSERT INTO sessions(session_scope, workspace_scope, runtime_json, opened_at_ms, closed_outcome_json, closed_at_ms) VALUES (?1, ?2, ?3, ?4, NULL, NULL)",
                    params![
                        context.identity.session_key(),
                        context.identity.workspace_key(),
                        runtime_json,
                        opened_at_ms
                    ],
                )
                .map_err(|error| sql_error(&error))?;
        } else {
            transaction
                .execute(
                    "UPDATE sessions SET workspace_scope = ?2, runtime_json = ?3, opened_at_ms = ?4, closed_outcome_json = NULL, closed_at_ms = NULL WHERE session_scope = ?1",
                    params![
                        context.identity.session_key(),
                        context.identity.workspace_key(),
                        runtime_json,
                        opened_at_ms
                    ],
                )
                .map_err(|error| sql_error(&error))?;
        }
        commit(transaction)?;
        Ok(existed)
    }

    fn append_event(
        &self,
        context: &BackendRequestContext,
        idempotency_key: &str,
        event: &MemoryEvent,
    ) -> ProtocolResult<bool> {
        let event_json = encode(event)?;
        ensure_record_bytes(event_json.len())?;
        let observed_at_ms = as_sql_u64(event.observed_at_ms)?;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        require_session(&transaction, context)?;
        let scope = context.identity.session_key();
        if let Some(existing) = query_string(
            &transaction,
            "SELECT event_json FROM event_idempotency WHERE session_scope = ?1 AND idempotency_key = ?2",
            params![scope, idempotency_key],
        )? {
            return same_or_conflict(existing == event_json, "event idempotency conflict");
        }
        if let Some(existing) = query_string(
            &transaction,
            "SELECT event_json FROM events WHERE session_scope = ?1 AND event_id = ?2",
            params![context.identity.session_key(), event.event_id],
        )? {
            if existing != event_json {
                return Err(conflict("event identity conflict"));
            }
            let aliases = transaction
                .query_row(
                    "SELECT COUNT(*) FROM event_idempotency WHERE session_scope = ?1 AND event_id = ?2",
                    params![context.identity.session_key(), event.event_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| sql_error(&error))?;
            if to_u64(aliases)? >= MAX_EVENT_ALIASES {
                return Err(resource_exhausted());
            }
            ensure_idempotency_capacity(&transaction)?;
            transaction
                .execute(
                    "INSERT INTO event_idempotency(session_scope, idempotency_key, event_id, event_json) VALUES (?1, ?2, ?3, ?4)",
                    params![context.identity.session_key(), idempotency_key, event.event_id, event_json],
                )
                .map_err(|error| sql_error(&error))?;
            commit(transaction)?;
            return Ok(true);
        }
        ensure_capacity(&transaction, "SELECT COUNT(*) FROM events", MAX_EVENTS)?;
        ensure_idempotency_capacity(&transaction)?;
        transaction
            .execute(
                "INSERT INTO events(session_scope, event_id, observed_at_ms, event_json) VALUES (?1, ?2, ?3, ?4)",
                params![context.identity.session_key(), event.event_id, observed_at_ms, event_json],
            )
            .map_err(|error| sql_error(&error))?;
        transaction
            .execute(
                "INSERT INTO event_idempotency(session_scope, idempotency_key, event_id, event_json) VALUES (?1, ?2, ?3, ?4)",
                params![context.identity.session_key(), idempotency_key, event.event_id, encode(event)?],
            )
            .map_err(|error| sql_error(&error))?;
        advance_revision(&transaction)?;
        commit(transaction)?;
        Ok(false)
    }

    fn materialize_context(
        &self,
        context: &BackendRequestContext,
        purpose: RecallPurpose,
        binding: &RecallBinding,
        query: &str,
        budget: ContextBudget,
    ) -> ProtocolResult<ContextView> {
        validate_budget(budget)?;
        {
            let connection = self.connection()?;
            require_session(&connection, context)?;
        }
        let knowledge = if matches!(purpose, RecallPurpose::Turn) {
            recall_knowledge(self.knowledge.as_ref(), query, budget)
        } else {
            KnowledgeRecall::disabled()
        };
        let KnowledgeRecall {
            candidates: knowledge_candidates,
            enabled: knowledge_enabled,
            degraded: knowledge_degraded,
            degradation_reason: knowledge_degradation_reason,
            filtered: knowledge_filtered,
        } = knowledge;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        require_session(&transaction, context)?;
        prune_expired(&transaction, now_ms())?;
        ensure_view_capacity(&transaction)?;
        let tasks = load_tasks(&transaction, &context.identity.workspace_key(), binding)?;
        let mut candidates = Vec::new();
        let mut candidate_ids = HashSet::new();
        let mut truncated = tasks.len() > MAX_TRACE_DECISIONS || knowledge_filtered;
        for stored in tasks.into_iter().take(MAX_TRACE_DECISIONS) {
            let content = format_task(&stored.task, &stored.evidence);
            let token_estimate = estimate_tokens(&content);
            candidate_ids.insert(stored.task.task_id.clone());
            candidates.push(RecallCandidate {
                item: ContextItem {
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
                },
                reason: "workspace task checkpoint".to_string(),
                score: 1.0,
            });
        }
        for mut candidate in knowledge_candidates
            .into_iter()
            .take(MAX_TRACE_DECISIONS.saturating_sub(candidates.len()))
        {
            while !candidate_ids.insert(candidate.item.item_id.clone()) {
                candidate.item.item_id.push('k');
            }
            candidates.push(candidate);
        }
        let event_slots = MAX_TRACE_DECISIONS.saturating_sub(candidates.len());
        if event_slots > 0 {
            let (events, scan_truncated) = load_event_candidates(
                &transaction,
                &context.identity.workspace_key(),
                purpose,
                query,
            )?;
            truncated |= scan_truncated || events.len() > event_slots;
            for event in events.into_iter().take(event_slots) {
                let mut item_id = format!("local-event-{}", event.row_id);
                while !candidate_ids.insert(item_id.clone()) {
                    item_id.push('e');
                }
                let content = format_event(&event.event);
                let reason = match purpose {
                    RecallPurpose::Turn => "workspace tool event overlaps query",
                    RecallPurpose::SessionResume | RecallPurpose::Handoff => {
                        "recent workspace tool event"
                    }
                };
                candidates.push(RecallCandidate {
                    item: ContextItem {
                        item_id,
                        revision: None,
                        kind: ContextItemKind::Evidence,
                        content: content.clone(),
                        source_ref: format!(
                            "local-event:{}:{}",
                            event.row_id, event.event.event_id
                        ),
                        authority: MemoryAuthority::Candidate,
                        token_estimate: estimate_tokens(&content),
                        reason: reason.to_string(),
                        stale: false,
                        score: event.score,
                    },
                    reason: reason.to_string(),
                    score: event.score,
                });
            }
        }
        let mut items = Vec::new();
        let mut decisions = Vec::new();
        let mut total_tokens = 0_u32;
        let mut total_bytes = 0_u32;
        for (index, candidate) in candidates.into_iter().enumerate() {
            let byte_estimate = u32::try_from(candidate.item.content.len()).unwrap_or(u32::MAX);
            let admitted = items.len() < usize::from(budget.max_items)
                && total_tokens.saturating_add(candidate.item.token_estimate) <= budget.max_tokens
                && total_bytes.saturating_add(byte_estimate) <= budget.max_bytes;
            truncated |= !admitted;
            decisions.push(RecallDecision {
                item_id: candidate.item.item_id.clone(),
                admitted,
                reason: if admitted {
                    candidate.reason
                } else {
                    "context budget exhausted".to_string()
                },
                rank: u32::try_from(index + 1).unwrap_or(u32::MAX),
                score: candidate.score,
            });
            if admitted {
                total_tokens = total_tokens.saturating_add(candidate.item.token_estimate);
                total_bytes = total_bytes.saturating_add(byte_estimate);
                items.push(candidate.item);
            }
        }
        let (snapshot_revision, next_view) = allocate_view(&transaction)?;
        let context_view_id = format!("local-ctx-{next_view}");
        let created_at_ms = now_ms();
        let view = ContextView {
            context_view_id: context_view_id.clone(),
            trace_id: context.trace_id.clone(),
            snapshot_revision,
            query: query.to_string(),
            items,
            total_tokens,
            total_bytes,
            effective_strategy: if knowledge_enabled {
                if knowledge_degraded {
                    "local_only_knowledge_degraded"
                } else {
                    "local_with_knowledge"
                }
            } else {
                "local_task_and_event"
            }
            .to_string(),
            degraded: knowledge_degraded,
            truncated,
            created_at_ms,
        };
        let trace = RecallTrace {
            context_view_id: context_view_id.clone(),
            trace_id: context.trace_id.clone(),
            response_trace_id: context.trace_id.clone(),
            backend_id: "local-sqlite-v1".to_string(),
            decisions,
            degraded: knowledge_degraded,
            degradation_reason: knowledge_degradation_reason,
            outcome_report: None,
        };
        transaction
            .execute(
                "INSERT INTO views(session_scope, view_id, created_at_ms, view_json, trace_json) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    context.identity.session_key(),
                    context_view_id,
                    as_sql_u64(created_at_ms)?,
                    encode(&view)?,
                    encode(&trace)?
                ],
            )
            .map_err(|error| sql_error(&error))?;
        commit(transaction)?;
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
        validate_task_revision(task.revision, expected_revision)?;
        let revision = as_sql_u64(task.revision)?;
        let task_json = encode(task)?;
        let evidence_json = encode(evidence)?;
        ensure_record_bytes(task_json.len().saturating_add(evidence_json.len()))?;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        require_session(&transaction, context)?;
        let workspace = context.identity.workspace_key();
        let existing_key = transaction
            .query_row(
                "SELECT task_json, evidence_json FROM checkpoint_idempotency WHERE workspace_scope = ?1 AND idempotency_key = ?2",
                params![workspace, idempotency_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|error| sql_error(&error))?;
        if let Some((existing_task, existing_evidence)) = existing_key {
            return same_or_conflict(
                existing_task == task_json && existing_evidence == evidence_json,
                "checkpoint idempotency conflict",
            );
        }
        let actual_revision = transaction
            .query_row(
                "SELECT revision FROM tasks WHERE workspace_scope = ?1 AND task_id = ?2",
                params![context.identity.workspace_key(), task.task_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| sql_error(&error))?;
        let expected_sql = expected_revision.map(as_sql_u64).transpose()?;
        if actual_revision != expected_sql {
            return Err(conflict("task checkpoint revision conflict"));
        }
        if actual_revision.is_none() {
            ensure_capacity(&transaction, "SELECT COUNT(*) FROM tasks", MAX_TASKS)?;
        }
        ensure_idempotency_capacity(&transaction)?;
        transaction
            .execute(
                "INSERT INTO tasks(workspace_scope, task_id, revision, task_json, evidence_json) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(workspace_scope, task_id) DO UPDATE SET revision = excluded.revision, task_json = excluded.task_json, evidence_json = excluded.evidence_json",
                params![context.identity.workspace_key(), task.task_id, revision, task_json, evidence_json],
            )
            .map_err(|error| sql_error(&error))?;
        transaction
            .execute(
                "INSERT INTO checkpoint_idempotency(workspace_scope, idempotency_key, task_id, task_json, evidence_json) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![context.identity.workspace_key(), idempotency_key, task.task_id, encode(task)?, encode(evidence)?],
            )
            .map_err(|error| sql_error(&error))?;
        advance_revision(&transaction)?;
        commit(transaction)?;
        Ok(false)
    }

    fn explain_context(
        &self,
        context: &BackendRequestContext,
        context_view_id: &str,
    ) -> ProtocolResult<RecallTrace> {
        let connection = self.connection()?;
        require_session(&connection, context)?;
        let encoded = query_string(
            &connection,
            "SELECT trace_json FROM views WHERE session_scope = ?1 AND view_id = ?2",
            params![context.identity.session_key(), context_view_id],
        )?
        .ok_or_else(not_found)?;
        let mut trace: RecallTrace = decode(&encoded)?;
        trace.response_trace_id = context.trace_id.clone();
        Ok(trace)
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
        let report = RecallOutcomeReport {
            admitted_item_ids: admitted_item_ids.to_vec(),
            dropped_item_ids: dropped_item_ids.to_vec(),
            outcome,
        };
        let report_json = encode(&report)?;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        require_session(&transaction, context)?;
        if let Some(existing) = query_string(
            &transaction,
            "SELECT report_json FROM outcome_idempotency WHERE session_scope = ?1 AND view_id = ?2 AND idempotency_key = ?3",
            params![
                context.identity.session_key(),
                context_view_id,
                idempotency_key
            ],
        )? {
            return same_or_conflict(existing == report_json, "outcome idempotency conflict");
        }
        let encoded_trace = query_string(
            &transaction,
            "SELECT trace_json FROM views WHERE session_scope = ?1 AND view_id = ?2",
            params![context.identity.session_key(), context_view_id],
        )?
        .ok_or_else(not_found)?;
        let mut trace: RecallTrace = decode(&encoded_trace)?;
        validate_outcome(&trace, &report)?;
        let replayed = if let Some(existing) = &trace.outcome_report {
            if existing != &report {
                return Err(conflict("context view already has a different outcome"));
            }
            true
        } else {
            trace.outcome_report = Some(report.clone());
            false
        };
        let aliases = transaction
            .query_row(
                "SELECT COUNT(*) FROM outcome_idempotency WHERE session_scope = ?1 AND view_id = ?2",
                params![context.identity.session_key(), context_view_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| sql_error(&error))?;
        if to_u64(aliases)? >= MAX_OUTCOME_ALIASES {
            return Err(resource_exhausted());
        }
        ensure_idempotency_capacity(&transaction)?;
        transaction
            .execute(
                "INSERT INTO outcome_idempotency(session_scope, view_id, idempotency_key, report_json) VALUES (?1, ?2, ?3, ?4)",
                params![context.identity.session_key(), context_view_id, idempotency_key, report_json],
            )
            .map_err(|error| sql_error(&error))?;
        transaction
            .execute(
                "UPDATE views SET trace_json = ?3 WHERE session_scope = ?1 AND view_id = ?2",
                params![
                    context.identity.session_key(),
                    context_view_id,
                    encode(&trace)?
                ],
            )
            .map_err(|error| sql_error(&error))?;
        commit(transaction)?;
        Ok(replayed)
    }

    fn forget(
        &self,
        context: &BackendRequestContext,
        kind: MemoryObjectKind,
        memory_id: &str,
    ) -> ProtocolResult<bool> {
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        require_session(&transaction, context)?;
        let deleted = match kind {
            MemoryObjectKind::Task => transaction.execute(
                "DELETE FROM tasks WHERE workspace_scope = ?1 AND task_id = ?2",
                params![context.identity.workspace_key(), memory_id],
            ),
            MemoryObjectKind::Event => transaction.execute(
                "DELETE FROM events WHERE session_scope = ?1 AND event_id = ?2",
                params![context.identity.session_key(), memory_id],
            ),
            MemoryObjectKind::ContextView => transaction.execute(
                "DELETE FROM views WHERE session_scope = ?1 AND view_id = ?2",
                params![context.identity.session_key(), memory_id],
            ),
        }
        .map_err(|error| sql_error(&error))?
            != 0;
        if deleted {
            advance_revision(&transaction)?;
        }
        commit(transaction)?;
        Ok(deleted)
    }

    fn close_session(
        &self,
        context: &BackendRequestContext,
        idempotency_key: &str,
        outcome: SessionOutcome,
    ) -> ProtocolResult<bool> {
        let outcome_json = encode(&outcome)?;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        if let Some(existing) = query_string(
            &transaction,
            "SELECT outcome_json FROM close_idempotency WHERE session_scope = ?1 AND idempotency_key = ?2",
            params![context.identity.session_key(), idempotency_key],
        )? {
            if existing != outcome_json {
                return Err(conflict("close idempotency conflict"));
            }
            transaction
                .execute(
                    "UPDATE sessions SET closed_outcome_json = ?2, closed_at_ms = ?3 WHERE session_scope = ?1",
                    params![
                        context.identity.session_key(),
                        outcome_json,
                        as_sql_u64(now_ms())?
                    ],
                )
                .map_err(|error| sql_error(&error))?;
            commit(transaction)?;
            return Ok(true);
        }
        require_session(&transaction, context)?;
        ensure_idempotency_capacity(&transaction)?;
        transaction
            .execute(
                "INSERT INTO close_idempotency(session_scope, idempotency_key, outcome_json) VALUES (?1, ?2, ?3)",
                params![context.identity.session_key(), idempotency_key, outcome_json],
            )
            .map_err(|error| sql_error(&error))?;
        transaction
            .execute(
                "UPDATE sessions SET closed_outcome_json = ?2, closed_at_ms = ?3 WHERE session_scope = ?1",
                params![
                    context.identity.session_key(),
                    encode(&outcome)?,
                    as_sql_u64(now_ms())?
                ],
            )
            .map_err(|error| sql_error(&error))?;
        commit(transaction)?;
        Ok(false)
    }
}

#[derive(Debug)]
struct StoredTask {
    task: TaskState,
    evidence: Vec<EvidenceRef>,
}

#[derive(Debug)]
struct RecallCandidate {
    item: ContextItem,
    reason: String,
    score: f32,
}

#[derive(Debug)]
struct RankedEvent {
    row_id: u64,
    observed_at_ms: u64,
    event: MemoryEvent,
    overlap: usize,
    score: f32,
}

struct KnowledgeRecall {
    candidates: Vec<RecallCandidate>,
    enabled: bool,
    degraded: bool,
    degradation_reason: Option<String>,
    filtered: bool,
}

impl KnowledgeRecall {
    fn disabled() -> Self {
        Self {
            candidates: Vec::new(),
            enabled: false,
            degraded: false,
            degradation_reason: None,
            filtered: false,
        }
    }

    fn degraded(error: KnowledgeError) -> Self {
        Self {
            candidates: Vec::new(),
            enabled: true,
            degraded: true,
            degradation_reason: Some(format!("knowledge provider {}", error.code)),
            filtered: false,
        }
    }
}

fn recall_knowledge(
    binding: Option<&KnowledgeProviderBinding>,
    query: &str,
    budget: ContextBudget,
) -> KnowledgeRecall {
    let Some(binding) = binding else {
        return KnowledgeRecall::disabled();
    };
    let request = KnowledgeQuery {
        document_id: binding.document_id.clone(),
        selector: KnowledgeSelector::Search {
            pattern: focused_knowledge_pattern(query),
            context_lines: 1,
        },
        max_excerpt_bytes: binding
            .max_excerpt_bytes
            .min(usize::try_from(budget.max_bytes).unwrap_or(usize::MAX)),
        max_items: binding.max_items.min(budget.max_items),
    };
    let items = match binding.provider.query(&request) {
        Ok(items) => items,
        Err(error) => return KnowledgeRecall::degraded(error),
    };
    let mut candidates = Vec::new();
    let mut filtered = items.len() > usize::from(request.max_items);
    for item in items.into_iter().take(usize::from(request.max_items)) {
        match knowledge_candidate(item, request.max_excerpt_bytes) {
            Some(candidate) => candidates.push(candidate),
            None => filtered = true,
        }
    }
    KnowledgeRecall {
        candidates,
        enabled: true,
        degraded: false,
        degradation_reason: None,
        filtered,
    }
}

fn knowledge_candidate(item: KnowledgeItem, maximum: usize) -> Option<RecallCandidate> {
    if item.excerpt.trim().is_empty() || item.excerpt.len() > maximum {
        return None;
    }
    let excerpt = redact_secrets(&item.excerpt);
    if looks_like_prompt_injection(&excerpt) {
        return None;
    }
    let title = item
        .title
        .as_deref()
        .map(redact_secrets)
        .filter(|value| !looks_like_prompt_injection(value));
    let content = match title {
        Some(title) => format!("{title}\n{excerpt}"),
        None => excerpt,
    };
    let selector = item.reference.selector.as_deref().unwrap_or("focused");
    let source_ref = redact_secrets(&format!(
        "knowledge://{}/{}/{}?fingerprint={}",
        item.reference.provider, item.reference.document_id, selector, item.fingerprint
    ));
    let identity = format!("{source_ref}\u{1f}{}", item.fingerprint);
    let item_id = format!("knowledge-{:016x}", fnv1a64(identity.as_bytes()));
    let score = item
        .score
        .filter(|score| score.is_finite())
        .map(|score| score.clamp(0.0, 1.0))
        .unwrap_or(0.5);
    Some(RecallCandidate {
        item: ContextItem {
            item_id,
            revision: None,
            kind: ContextItemKind::Knowledge,
            content: content.clone(),
            source_ref,
            authority: MemoryAuthority::Candidate,
            token_estimate: estimate_tokens(&content),
            reason: "focused provider-owned reference".to_string(),
            stale: false,
            score,
        },
        reason: "focused provider-owned reference".to_string(),
        score,
    })
}

fn focused_knowledge_pattern(query: &str) -> String {
    let mut candidates = query
        .split(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    ',' | ';' | ':' | '?' | '!' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\''
                )
        })
        .filter(|candidate| !candidate.trim_matches('.').is_empty())
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| {
        let uppercase = candidate.chars().any(char::is_alphabetic)
            && candidate
                .chars()
                .filter(|character| character.is_alphabetic())
                .all(|character| character.is_uppercase());
        (uppercase, candidate.starts_with('-'), candidate.len())
    });
    let selected = candidates
        .last()
        .copied()
        .unwrap_or(query)
        .trim_matches('.');
    let mut pattern = selected.to_string();
    truncate_utf8(&mut pattern, MAX_KNOWLEDGE_DOCUMENT_BYTES);
    if pattern.trim().is_empty() {
        "shell".to_string()
    } else {
        pattern
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn truncate_utf8(value: &mut String, maximum: usize) {
    if value.len() <= maximum {
        return;
    }
    let mut boundary = maximum;
    while !value.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    value.truncate(boundary);
}

fn prepare_private_path(path: &Path) -> ProtocolResult<()> {
    let parent = path
        .parent()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .ok_or_else(|| invalid("local memory database requires an explicit parent directory"))?;
    let parent_existed = parent.exists();
    let mut builder = DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(parent).map_err(|_| io_error())?;
    if !parent_existed {
        fs::set_permissions(parent, Permissions::from_mode(0o700)).map_err(|_| io_error())?;
    }
    let mode = fs::metadata(parent)
        .map_err(|_| io_error())?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(ProtocolError::new(
            ProtocolErrorCode::Unauthorized,
            "local memory parent directory must be owner-only",
            false,
        ));
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(invalid("local memory database must be a regular file"));
        }
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|_| io_error())?;
    fs::set_permissions(path, Permissions::from_mode(0o600)).map_err(|_| io_error())
}

fn configure(connection: &Connection) -> ProtocolResult<()> {
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|error| sql_error(&error))?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|error| sql_error(&error))?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(|error| sql_error(&error))
}

fn initialize_schema(connection: &mut Connection) -> ProtocolResult<()> {
    let version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|error| sql_error(&error))?;
    if version > SCHEMA_VERSION {
        return Err(ProtocolError::new(
            ProtocolErrorCode::VersionUnsupported,
            "local memory database schema is newer than this binary",
            false,
        ));
    }
    if version != 0 {
        return Ok(());
    }

    // Serialize the first open across hook processes, then re-check the
    // version after acquiring the write lock. Otherwise two first-use hooks
    // could both observe version zero and race CREATE TABLE.
    let transaction = immediate(connection)?;
    let locked_version = transaction
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|error| sql_error(&error))?;
    if locked_version > SCHEMA_VERSION {
        return Err(ProtocolError::new(
            ProtocolErrorCode::VersionUnsupported,
            "local memory database schema is newer than this binary",
            false,
        ));
    }
    if locked_version == 0 {
        transaction
            .execute_batch(SCHEMA_V1)
            .map_err(|error| sql_error(&error))?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|error| sql_error(&error))?;
    }
    commit(transaction)
}

fn immediate(connection: &mut Connection) -> ProtocolResult<Transaction<'_>> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error(&error))
}

fn commit(transaction: Transaction<'_>) -> ProtocolResult<()> {
    transaction.commit().map_err(|error| sql_error(&error))
}

fn require_session(connection: &Connection, context: &BackendRequestContext) -> ProtocolResult<()> {
    let active = connection
        .query_row(
            "SELECT closed_outcome_json IS NULL FROM sessions WHERE session_scope = ?1",
            params![context.identity.session_key()],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(|error| sql_error(&error))?
        .unwrap_or(false);
    if active {
        Ok(())
    } else {
        Err(ProtocolError::new(
            ProtocolErrorCode::SessionNotOpen,
            "memory session is not open",
            false,
        ))
    }
}

fn load_tasks(
    connection: &Connection,
    workspace_scope: &str,
    binding: &RecallBinding,
) -> ProtocolResult<Vec<StoredTask>> {
    let mut statement = connection
        .prepare(
            "SELECT task_json, evidence_json FROM tasks WHERE workspace_scope = ?1 AND (?2 IS NULL OR task_id = ?2) ORDER BY task_id LIMIT 257",
        )
        .map_err(|error| sql_error(&error))?;
    let rows = statement
        .query_map(
            params![workspace_scope, binding.task_id.as_deref()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|error| sql_error(&error))?;
    let mut tasks = Vec::new();
    for row in rows {
        let (task, evidence) = row.map_err(|error| sql_error(&error))?;
        tasks.push(StoredTask {
            task: decode(&task)?,
            evidence: decode(&evidence)?,
        });
    }
    Ok(tasks)
}

fn load_event_candidates(
    connection: &Connection,
    workspace_scope: &str,
    purpose: RecallPurpose,
    query: &str,
) -> ProtocolResult<(Vec<RankedEvent>, bool)> {
    let mut statement = connection
        .prepare(
            "SELECT e.rowid, e.observed_at_ms, e.event_json FROM events e INNER JOIN sessions s ON s.session_scope = e.session_scope WHERE s.workspace_scope = ?1 ORDER BY e.observed_at_ms DESC, e.rowid DESC LIMIT 1025",
        )
        .map_err(|error| sql_error(&error))?;
    let rows = statement
        .query_map(params![workspace_scope], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| sql_error(&error))?;
    let query_tokens = normalized_tokens(query);
    let requires_overlap = !matches!(purpose, RecallPurpose::SessionResume);
    let mut events = Vec::new();
    let mut scanned = 0_usize;
    for row in rows {
        let (row_id, observed_at_ms, encoded) = row.map_err(|error| sql_error(&error))?;
        scanned = scanned.saturating_add(1);
        if scanned > MAX_EVENT_SCAN {
            break;
        }
        let event: MemoryEvent = decode(&encoded)?;
        if !matches!(
            event.kind,
            super::MemoryEventKind::ToolCompleted | super::MemoryEventKind::ToolFailed
        ) || !event_is_safe(&event)
        {
            continue;
        }
        let overlap = token_overlap(&query_tokens, &event.summary);
        if requires_overlap && overlap == 0 {
            continue;
        }
        let score = if requires_overlap {
            overlap as f32 / query_tokens.len().max(1) as f32
        } else {
            1.0
        };
        events.push(RankedEvent {
            row_id: to_u64(row_id)?,
            observed_at_ms: to_u64(observed_at_ms)?,
            event,
            overlap,
            score,
        });
    }
    if requires_overlap {
        events.sort_by(|left, right| {
            right
                .overlap
                .cmp(&left.overlap)
                .then_with(|| right.observed_at_ms.cmp(&left.observed_at_ms))
                .then_with(|| right.row_id.cmp(&left.row_id))
        });
    }
    Ok((events, scanned > MAX_EVENT_SCAN))
}

fn normalized_tokens(text: &str) -> HashSet<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn token_overlap(query_tokens: &HashSet<String>, text: &str) -> usize {
    let event_tokens = normalized_tokens(text);
    query_tokens.intersection(&event_tokens).count()
}

fn event_is_safe(event: &MemoryEvent) -> bool {
    !looks_like_prompt_injection(&event.summary)
        && !contains_secrets(&event.summary)
        && event.evidence_ref.as_deref().is_none_or(|reference| {
            !looks_like_prompt_injection(reference) && !contains_secrets(reference)
        })
}

fn format_event(event: &MemoryEvent) -> String {
    let kind = match event.kind {
        super::MemoryEventKind::ToolCompleted => "tool_completed",
        super::MemoryEventKind::ToolFailed => "tool_failed",
        _ => "runtime_event",
    };
    let outcome = match event.outcome {
        super::MemoryEventOutcome::Succeeded => "succeeded",
        super::MemoryEventOutcome::Failed => "failed",
        super::MemoryEventOutcome::Unknown => "unknown",
    };
    let mut content = format!(
        "Event: {kind}\nOutcome: {outcome}\nSource: {}\nSummary: {}",
        event.source, event.summary
    );
    if let Some(reference) = &event.evidence_ref {
        content.push_str("\nEvidence reference: ");
        content.push_str(reference);
    }
    content
}

fn validate_budget(budget: ContextBudget) -> ProtocolResult<()> {
    if budget.max_items == 0 || budget.max_tokens == 0 || budget.max_bytes == 0 {
        return Err(invalid("context budget values must be non-zero"));
    }
    if usize::from(budget.max_items) > MAX_TRACE_DECISIONS
        || budget.max_tokens > MAX_CONTEXT_TOKENS
        || budget.max_bytes > MAX_CONTEXT_BYTES
    {
        return Err(resource_exhausted());
    }
    Ok(())
}

fn validate_task_revision(revision: u64, expected: Option<u64>) -> ProtocolResult<()> {
    let required = expected
        .map(|value| value.checked_add(1))
        .unwrap_or(Some(1))
        .ok_or_else(resource_exhausted)?;
    if revision == required {
        Ok(())
    } else {
        Err(invalid("task revision does not follow expected revision"))
    }
}

fn validate_outcome(trace: &RecallTrace, report: &RecallOutcomeReport) -> ProtocolResult<()> {
    let returned: HashSet<&str> = trace
        .decisions
        .iter()
        .filter(|decision| decision.admitted)
        .map(|decision| decision.item_id.as_str())
        .collect();
    let reported: Vec<&str> = report
        .admitted_item_ids
        .iter()
        .chain(&report.dropped_item_ids)
        .map(String::as_str)
        .collect();
    let unique: HashSet<&str> = reported.iter().copied().collect();
    let overlaps = report
        .admitted_item_ids
        .iter()
        .any(|item| report.dropped_item_ids.contains(item));
    if overlaps
        || unique.len() != reported.len()
        || unique != returned
        || (returned.is_empty() && matches!(report.outcome, FeedbackOutcome::Useful))
    {
        return Err(conflict(
            "recall outcome is not a complete unique partition",
        ));
    }
    Ok(())
}

fn allocate_view(transaction: &Transaction<'_>) -> ProtocolResult<(u64, u64)> {
    let (revision, next_view) = transaction
        .query_row(
            "SELECT revision, next_view FROM backend_state WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|error| sql_error(&error))?;
    let allocated = next_view.checked_add(1).ok_or_else(resource_exhausted)?;
    transaction
        .execute(
            "UPDATE backend_state SET next_view = ?1 WHERE singleton = 1",
            params![allocated],
        )
        .map_err(|error| sql_error(&error))?;
    Ok((to_u64(revision)?, to_u64(allocated)?))
}

fn advance_revision(transaction: &Transaction<'_>) -> ProtocolResult<()> {
    let changed = transaction
        .execute(
            "UPDATE backend_state SET revision = revision + 1 WHERE singleton = 1 AND revision < 9223372036854775807",
            [],
        )
        .map_err(|error| sql_error(&error))?;
    if changed == 1 {
        Ok(())
    } else {
        Err(resource_exhausted())
    }
}

fn prune_expired(transaction: &Transaction<'_>, now: u64) -> ProtocolResult<()> {
    let view_cutoff = as_sql_u64(now.saturating_sub(VIEW_RETENTION_MS))?;
    transaction
        .execute(
            "DELETE FROM views WHERE created_at_ms < ?1",
            params![view_cutoff],
        )
        .map_err(|error| sql_error(&error))?;
    let session_cutoff = as_sql_u64(now.saturating_sub(SESSION_RETENTION_MS))?;
    transaction
        .execute(
            "DELETE FROM sessions WHERE closed_at_ms IS NOT NULL AND closed_at_ms < ?1",
            params![session_cutoff],
        )
        .map_err(|error| sql_error(&error))?;
    Ok(())
}

fn ensure_session_capacity(transaction: &Transaction<'_>) -> ProtocolResult<()> {
    if table_count(transaction, "SELECT COUNT(*) FROM sessions")? >= MAX_SESSIONS {
        transaction
            .execute(
                "DELETE FROM sessions WHERE session_scope IN (SELECT session_scope FROM sessions WHERE closed_at_ms IS NOT NULL ORDER BY closed_at_ms, session_scope LIMIT ?1)",
                params![as_sql_u64(CAPACITY_PRUNE_BATCH)?],
            )
            .map_err(|error| sql_error(&error))?;
    }
    ensure_capacity(transaction, "SELECT COUNT(*) FROM sessions", MAX_SESSIONS)
}

fn ensure_view_capacity(transaction: &Transaction<'_>) -> ProtocolResult<()> {
    if table_count(transaction, "SELECT COUNT(*) FROM views")? >= MAX_VIEWS {
        transaction
            .execute(
                "DELETE FROM views WHERE rowid IN (SELECT rowid FROM views ORDER BY created_at_ms, rowid LIMIT ?1)",
                params![as_sql_u64(CAPACITY_PRUNE_BATCH)?],
            )
            .map_err(|error| sql_error(&error))?;
    }
    ensure_capacity(transaction, "SELECT COUNT(*) FROM views", MAX_VIEWS)
}

fn ensure_capacity(transaction: &Transaction<'_>, query: &str, limit: u64) -> ProtocolResult<()> {
    if table_count(transaction, query)? >= limit {
        Err(resource_exhausted())
    } else {
        Ok(())
    }
}

fn ensure_idempotency_capacity(transaction: &Transaction<'_>) -> ProtocolResult<()> {
    let total = table_count(
        transaction,
        "SELECT (SELECT COUNT(*) FROM event_idempotency) + (SELECT COUNT(*) FROM checkpoint_idempotency) + (SELECT COUNT(*) FROM outcome_idempotency) + (SELECT COUNT(*) FROM close_idempotency)",
    )?;
    if total >= MAX_IDEMPOTENCY_ROWS {
        Err(resource_exhausted())
    } else {
        Ok(())
    }
}

fn ensure_record_bytes(bytes: usize) -> ProtocolResult<()> {
    if bytes > MAX_RECORD_BYTES {
        Err(resource_exhausted())
    } else {
        Ok(())
    }
}

fn query_string<P: rusqlite::Params>(
    connection: &Connection,
    query: &str,
    params: P,
) -> ProtocolResult<Option<String>> {
    connection
        .query_row(query, params, |row| row.get(0))
        .optional()
        .map_err(|error| sql_error(&error))
}

fn table_count(connection: &Connection, query: &str) -> ProtocolResult<u64> {
    let count = connection
        .query_row(query, [], |row| row.get::<_, i64>(0))
        .map_err(|error| sql_error(&error))?;
    to_u64(count)
}

fn pragma_u64(connection: &Connection, pragma: &str) -> ProtocolResult<u64> {
    let value = connection
        .pragma_query_value(None, pragma, |row| row.get::<_, i64>(0))
        .map_err(|error| sql_error(&error))?;
    to_u64(value)
}

fn physical_size(path: &Path) -> ProtocolResult<u64> {
    let mut total = metadata_len(path)?;
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        total = total.saturating_add(metadata_len(Path::new(&sidecar))?);
    }
    Ok(total)
}

fn metadata_len(path: &Path) -> ProtocolResult<u64> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(_) => Err(io_error()),
    }
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

fn estimate_tokens(content: &str) -> u32 {
    u32::try_from(content.len())
        .unwrap_or(u32::MAX)
        .saturating_add(3)
        / 4
}

fn encode<T: Serialize + ?Sized>(value: &T) -> ProtocolResult<String> {
    serde_json::to_string(value).map_err(|_| {
        ProtocolError::new(
            ProtocolErrorCode::Internal,
            "local memory serialization failed",
            false,
        )
    })
}

fn decode<T: DeserializeOwned>(value: &str) -> ProtocolResult<T> {
    serde_json::from_str(value).map_err(|_| {
        ProtocolError::new(
            ProtocolErrorCode::IntegrityFailed,
            "local memory record failed integrity validation",
            false,
        )
    })
}

fn same_or_conflict(same: bool, message: &'static str) -> ProtocolResult<bool> {
    if same {
        Ok(true)
    } else {
        Err(conflict(message))
    }
}

fn conflict(message: &'static str) -> ProtocolError {
    ProtocolError::new(ProtocolErrorCode::Conflict, message, false)
}

fn not_found() -> ProtocolError {
    ProtocolError::new(
        ProtocolErrorCode::NotFound,
        "memory object was not found in the caller scope",
        false,
    )
}

fn invalid(message: &'static str) -> ProtocolError {
    ProtocolError::new(ProtocolErrorCode::InvalidRequest, message, false)
}

fn resource_exhausted() -> ProtocolError {
    ProtocolError::new(
        ProtocolErrorCode::ResourceExhausted,
        "local memory capacity is exhausted",
        false,
    )
}

fn io_error() -> ProtocolError {
    ProtocolError::new(
        ProtocolErrorCode::Unavailable,
        "local memory storage is unavailable",
        true,
    )
}

fn sql_error(error: &rusqlite::Error) -> ProtocolError {
    let retryable = matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    );
    ProtocolError::new(
        if retryable {
            ProtocolErrorCode::Unavailable
        } else {
            ProtocolErrorCode::Internal
        },
        "local memory database operation failed",
        retryable,
    )
}

fn as_sql_u64(value: u64) -> ProtocolResult<i64> {
    i64::try_from(value).map_err(|_| resource_exhausted())
}

fn to_u64(value: i64) -> ProtocolResult<u64> {
    u64::try_from(value).map_err(|_| {
        ProtocolError::new(
            ProtocolErrorCode::IntegrityFailed,
            "local memory counter failed integrity validation",
            false,
        )
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
