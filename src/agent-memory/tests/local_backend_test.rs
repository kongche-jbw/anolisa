use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use agent_memory::knowledge::{
    KnowledgeCapability, KnowledgeError, KnowledgeErrorCode, KnowledgeItem, KnowledgeProvider,
    KnowledgeProviderDescriptor, KnowledgeQuery, KnowledgeResult,
};
use agent_memory::protocol::{
    BackendRequestContext, ContextBudget, EvidenceRef, FeedbackOutcome, IdentityContext,
    KnowledgeProviderBinding, KnowledgeRef, LocalManagementContext, LocalMemoryBackend,
    MemoryAuthority, MemoryBackend, MemoryCapability, MemoryDurability, MemoryEvent,
    MemoryEventKind, MemoryEventOutcome, MemoryObjectKind, ProtocolErrorCode, RecallBinding,
    RecallPurpose, RuntimeContext, SessionOutcome, TaskState,
};
use rusqlite::Connection;

fn database_path(root: &Path) -> PathBuf {
    root.join("private-memory").join("memory.sqlite3")
}

fn context(user: &str, workspace: &str, session: &str, trace: &str) -> BackendRequestContext {
    BackendRequestContext {
        request_id: format!("request-{trace}"),
        trace_id: trace.to_string(),
        run_id: Some("run-1".to_string()),
        task_id: None,
        turn_id: None,
        deadline_at_ms: None,
        identity: IdentityContext {
            tenant_id: None,
            team_id: None,
            user_id: user.to_string(),
            agent_id: "cosh".to_string(),
            session_id: session.to_string(),
            workspace_id: workspace.to_string(),
        },
    }
}

fn runtime() -> RuntimeContext {
    RuntimeContext {
        runtime: "cosh-ng".to_string(),
        runtime_version: Some("0.1.0".to_string()),
        model: None,
        platform: Some("linux".to_string()),
    }
}

fn task(revision: u64, goal: &str) -> TaskState {
    TaskState {
        task_id: "task-1".to_string(),
        revision,
        goal: goal.to_string(),
        next_action: Some("continue safely".to_string()),
        blockers: vec!["none".to_string()],
        updated_at_ms: revision,
    }
}

fn evidence() -> Vec<EvidenceRef> {
    vec![EvidenceRef {
        provider: "agentsight".to_string(),
        uri: "trace:1".to_string(),
        digest: Some("sha256:abc".to_string()),
        summary: "verified tool result".to_string(),
    }]
}

fn event(summary: &str) -> MemoryEvent {
    MemoryEvent {
        event_id: "event-1".to_string(),
        kind: MemoryEventKind::ToolCompleted,
        source: "cosh-ng-hook".to_string(),
        outcome: MemoryEventOutcome::Succeeded,
        observed_at_ms: 1,
        summary: summary.to_string(),
        evidence_ref: Some("cosh://tool/1".to_string()),
    }
}

fn budget() -> ContextBudget {
    ContextBudget {
        max_tokens: 4_096,
        max_bytes: 16 * 1024,
        max_items: 16,
    }
}

fn open(backend: &LocalMemoryBackend, context: &BackendRequestContext) -> bool {
    backend
        .open_session(context, &runtime())
        .expect("open local session")
}

struct FakeKnowledgeProvider {
    fails: bool,
}

impl KnowledgeProvider for FakeKnowledgeProvider {
    fn descriptor(&self) -> KnowledgeResult<KnowledgeProviderDescriptor> {
        Ok(KnowledgeProviderDescriptor {
            provider_id: "fake-docs".to_string(),
            display_name: "Fake docs".to_string(),
            version: Some("1".to_string()),
            protocol: Some("fake/v1".to_string()),
            capabilities: vec![KnowledgeCapability::Search],
        })
    }

    fn query(&self, query: &KnowledgeQuery) -> KnowledgeResult<Vec<KnowledgeItem>> {
        if self.fails {
            return Err(KnowledgeError::new(
                KnowledgeErrorCode::Unavailable,
                "fake provider unavailable",
                true,
            ));
        }
        query.validate()?;
        Ok(vec![KnowledgeItem {
            reference: KnowledgeRef {
                provider: "fake-docs".to_string(),
                document_id: query.document_id.clone(),
                selector: Some(query.reference_selector()),
                content_digest: Some("fixture:pipe-status".to_string()),
                retrieved_at_ms: 1,
            },
            title: Some("Bash pipeline status".to_string()),
            excerpt: "PIPESTATUS records every command status in a pipeline.".to_string(),
            fingerprint: "fixture:pipe-status".to_string(),
            score: Some(0.9),
        }])
    }
}

struct CountingKnowledgeProvider {
    query_calls: Arc<AtomicUsize>,
}

impl KnowledgeProvider for CountingKnowledgeProvider {
    fn descriptor(&self) -> KnowledgeResult<KnowledgeProviderDescriptor> {
        Ok(KnowledgeProviderDescriptor {
            provider_id: "counting-docs".to_string(),
            display_name: "Counting docs".to_string(),
            version: Some("1".to_string()),
            protocol: Some("counting/v1".to_string()),
            capabilities: vec![KnowledgeCapability::Search],
        })
    }

    fn query(&self, _query: &KnowledgeQuery) -> KnowledgeResult<Vec<KnowledgeItem>> {
        self.query_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }
}

#[test]
fn configures_private_wal_database_and_reports_stats() {
    let root = tempfile::tempdir().expect("temp root");
    let path = database_path(root.path());
    let backend = LocalMemoryBackend::open(&path).expect("open backend");
    let manifest = backend.manifest();

    assert_eq!(manifest.durability, MemoryDurability::Durable);
    for capability in [
        MemoryCapability::Session,
        MemoryCapability::Capture,
        MemoryCapability::Recall,
        MemoryCapability::Checkpoint,
        MemoryCapability::Explain,
        MemoryCapability::Outcome,
        MemoryCapability::Forget,
    ] {
        assert!(manifest.capabilities.contains(&capability));
    }
    assert_eq!(
        fs::metadata(path.parent().expect("parent"))
            .expect("parent metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&path)
            .expect("database metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let observer = Connection::open(&path).expect("observer connection");
    let journal: String = observer
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("journal mode");
    let version: i64 = observer
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("schema version");
    assert_eq!(journal.to_ascii_lowercase(), "wal");
    assert_eq!(version, 1);

    let stats = backend.stats().expect("stats");
    assert!(stats.logical_bytes > 0);
    assert!(stats.physical_bytes > 0);
    assert_eq!(stats.session_count, 0);
    assert_eq!(stats.event_count, 0);
    assert_eq!(stats.task_count, 0);
    assert_eq!(stats.view_count, 0);
}

#[test]
fn concurrent_first_open_serializes_schema_creation() {
    let root = tempfile::tempdir().expect("temp root");
    let path = database_path(root.path());
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            LocalMemoryBackend::open(path)
        }));
    }
    barrier.wait();
    for worker in workers {
        worker
            .join()
            .expect("schema worker")
            .expect("concurrent first open");
    }

    let observer = Connection::open(path).expect("observer connection");
    let version: i64 = observer
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("schema version");
    assert_eq!(version, 1);
}

#[test]
fn reopens_cold_state_and_replays_lost_ack_mutations() {
    let root = tempfile::tempdir().expect("temp root");
    let path = database_path(root.path());
    let first = LocalMemoryBackend::open(&path).expect("first backend");
    let context = context("user-a", "workspace-a", "session-a", "trace-1");
    assert!(!open(&first, &context));
    assert!(
        !first
            .append_event(&context, "event-key", &event("compiled"))
            .expect("append event")
    );
    assert!(
        !first
            .checkpoint_task(
                &context,
                "checkpoint-key",
                &task(1, "ship"),
                None,
                &evidence()
            )
            .expect("checkpoint")
    );
    let original_view = first
        .materialize_context(
            &context,
            RecallPurpose::SessionResume,
            &RecallBinding::default(),
            "resume",
            budget(),
        )
        .expect("materialize");
    drop(first);

    let reopened = LocalMemoryBackend::open(&path).expect("reopen backend");
    assert!(open(&reopened, &context));
    assert!(
        reopened
            .append_event(&context, "event-key", &event("compiled"))
            .expect("event replay")
    );
    assert!(
        reopened
            .checkpoint_task(
                &context,
                "checkpoint-key",
                &task(1, "ship"),
                None,
                &evidence()
            )
            .expect("checkpoint replay")
    );
    let trace = reopened
        .explain_context(&context, &original_view.context_view_id)
        .expect("persisted trace");
    assert_eq!(trace.context_view_id, original_view.context_view_id);
    let recovered = reopened
        .materialize_context(
            &context,
            RecallPurpose::SessionResume,
            &RecallBinding::default(),
            "cold recovery",
            budget(),
        )
        .expect("cold recall");
    assert!(
        recovered
            .items
            .iter()
            .any(|item| item.content.contains("Goal: ship"))
    );

    assert!(
        !reopened
            .close_session(&context, "close-key", SessionOutcome::Interrupted)
            .expect("close")
    );
    drop(reopened);
    let after_lost_close_ack = LocalMemoryBackend::open(&path).expect("reopen after close");
    assert!(open(&after_lost_close_ack, &context));
    assert!(
        after_lost_close_ack
            .close_session(&context, "close-key", SessionOutcome::Interrupted)
            .expect("close replay")
    );
    assert_eq!(
        after_lost_close_ack
            .append_event(&context, "after-close", &event("must reject"))
            .expect_err("replayed close must close reopened session")
            .code,
        ProtocolErrorCode::SessionNotOpen
    );
}

#[test]
fn two_writers_enforce_optimistic_task_revision() {
    let root = tempfile::tempdir().expect("temp root");
    let path = database_path(root.path());
    let first = LocalMemoryBackend::open(&path).expect("first writer");
    let second = LocalMemoryBackend::open(&path).expect("second writer");
    let context = context("user-a", "workspace-a", "session-a", "trace-1");
    open(&first, &context);
    first
        .checkpoint_task(&context, "create", &task(1, "v1"), None, &[])
        .expect("create task");
    first
        .checkpoint_task(&context, "writer-one", &task(2, "writer one"), Some(1), &[])
        .expect("first update");
    let error = second
        .checkpoint_task(&context, "writer-two", &task(2, "writer two"), Some(1), &[])
        .expect_err("stale revision must conflict");
    assert_eq!(error.code, ProtocolErrorCode::Conflict);
}

#[test]
fn isolates_session_and_workspace_scopes() {
    let root = tempfile::tempdir().expect("temp root");
    let backend = LocalMemoryBackend::open(database_path(root.path())).expect("backend");
    let owner = context("user-a", "workspace-a", "session-a", "trace-owner");
    let stranger = context("user-b", "workspace-a", "session-b", "trace-stranger");
    open(&backend, &owner);
    open(&backend, &stranger);
    backend
        .checkpoint_task(&owner, "checkpoint", &task(1, "private goal"), None, &[])
        .expect("checkpoint");
    backend
        .append_event(&owner, "event", &event("private event"))
        .expect("event");
    let owner_view = backend
        .materialize_context(
            &owner,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "owner query",
            budget(),
        )
        .expect("owner recall");
    let stranger_view = backend
        .materialize_context(
            &stranger,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "stranger query",
            budget(),
        )
        .expect("stranger recall");

    assert_eq!(owner_view.items.len(), 1);
    assert!(stranger_view.items.is_empty());
    assert_eq!(
        backend
            .explain_context(&stranger, &owner_view.context_view_id)
            .expect_err("cross-scope view")
            .code,
        ProtocolErrorCode::NotFound
    );
    assert!(
        !backend
            .forget(&stranger, MemoryObjectKind::Event, "event-1")
            .expect("scoped forget")
    );
}

#[test]
fn recalls_relevant_tool_evidence_across_cold_session_boundary() {
    let root = tempfile::tempdir().expect("temp root");
    let path = database_path(root.path());
    let first = LocalMemoryBackend::open(&path).expect("first process");
    let session_a = context("user-a", "workspace-a", "session-a", "trace-a");
    open(&first, &session_a);
    first
        .append_event(
            &session_a,
            "compile-event",
            &event("cargo compile succeeded for the memory backend"),
        )
        .expect("append tool evidence");
    drop(first);

    let recovered = LocalMemoryBackend::open(&path).expect("cold reopen");
    let session_b = context("user-a", "workspace-a", "session-b", "trace-b");
    open(&recovered, &session_b);
    let relevant = recovered
        .materialize_context(
            &session_b,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "compile memory",
            budget(),
        )
        .expect("relevant recall");

    assert_eq!(relevant.items.len(), 1);
    let evidence = &relevant.items[0];
    assert_eq!(
        evidence.kind,
        agent_memory::protocol::ContextItemKind::Evidence
    );
    assert_eq!(
        evidence.authority,
        agent_memory::protocol::MemoryAuthority::Candidate
    );
    assert!(evidence.item_id.starts_with("local-event-"));
    assert!(evidence.source_ref.contains("event-1"));
    assert!(evidence.content.contains("cargo compile succeeded"));

    let unrelated = recovered
        .materialize_context(
            &session_b,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "banana orchard",
            budget(),
        )
        .expect("unrelated recall");
    assert!(unrelated.items.is_empty());
}

#[test]
fn merges_replaceable_knowledge_before_candidate_evidence() {
    let root = tempfile::tempdir().expect("temp root");
    let binding = KnowledgeProviderBinding::new(
        Arc::new(FakeKnowledgeProvider { fails: false }),
        "manual/1/bash",
    )
    .expect("knowledge binding");
    let backend = LocalMemoryBackend::open_with_knowledge(database_path(root.path()), binding)
        .expect("backend");
    let context = context("user-a", "workspace-a", "session-a", "trace-knowledge");
    open(&backend, &context);
    backend
        .append_event(
            &context,
            "event-key",
            &event("PIPESTATUS preserved every pipeline status"),
        )
        .expect("event");

    assert!(
        backend
            .manifest()
            .capabilities
            .contains(&MemoryCapability::Knowledge)
    );
    let view = backend
        .materialize_context(
            &context,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "How does PIPESTATUS work?",
            budget(),
        )
        .expect("knowledge recall");
    assert!(!view.degraded);
    assert_eq!(view.effective_strategy, "local_with_knowledge");
    let knowledge_index = view
        .items
        .iter()
        .position(|item| item.kind == agent_memory::protocol::ContextItemKind::Knowledge)
        .expect("knowledge item");
    let evidence_index = view
        .items
        .iter()
        .position(|item| item.kind == agent_memory::protocol::ContextItemKind::Evidence)
        .expect("evidence item");
    assert!(knowledge_index < evidence_index);
    let knowledge = &view.items[knowledge_index];
    assert_eq!(knowledge.authority, MemoryAuthority::Candidate);
    assert!(knowledge.source_ref.starts_with("knowledge://fake-docs/"));
    assert!(
        knowledge
            .content
            .contains("PIPESTATUS records every command")
    );
    let trace = backend
        .explain_context(&context, &view.context_view_id)
        .expect("knowledge trace");
    assert!(
        trace
            .decisions
            .iter()
            .any(|decision| decision.item_id == knowledge.item_id)
    );
}

#[test]
fn authorizes_session_scope_before_querying_knowledge_provider() {
    let root = tempfile::tempdir().expect("temp root");
    let query_calls = Arc::new(AtomicUsize::new(0));
    let binding = KnowledgeProviderBinding::new(
        Arc::new(CountingKnowledgeProvider {
            query_calls: Arc::clone(&query_calls),
        }),
        "manual/1/bash",
    )
    .expect("knowledge binding");
    let backend = LocalMemoryBackend::open_with_knowledge(database_path(root.path()), binding)
        .expect("backend");
    let owner = context("user-a", "workspace-a", "session-a", "trace-owner");

    let unopened_error = backend
        .materialize_context(
            &owner,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "PIPESTATUS",
            budget(),
        )
        .expect_err("unopened session must fail before provider query");
    assert_eq!(unopened_error.code, ProtocolErrorCode::SessionNotOpen);
    assert_eq!(query_calls.load(Ordering::SeqCst), 0);

    open(&backend, &owner);
    let foreign_scope = context("user-a", "workspace-b", "session-a", "trace-foreign");
    let scope_error = backend
        .materialize_context(
            &foreign_scope,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "PIPESTATUS",
            budget(),
        )
        .expect_err("foreign scope must fail before provider query");
    assert_eq!(scope_error.code, ProtocolErrorCode::SessionNotOpen);
    assert_eq!(query_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn provider_failure_degrades_without_hiding_local_memory() {
    let root = tempfile::tempdir().expect("temp root");
    let binding = KnowledgeProviderBinding::new(
        Arc::new(FakeKnowledgeProvider { fails: true }),
        "manual/1/bash",
    )
    .expect("knowledge binding");
    let backend = LocalMemoryBackend::open_with_knowledge(database_path(root.path()), binding)
        .expect("backend");
    let context = context("user-a", "workspace-a", "session-a", "trace-degraded");
    open(&backend, &context);
    backend
        .append_event(
            &context,
            "event-key",
            &event("PIPESTATUS local observation"),
        )
        .expect("event");

    let view = backend
        .materialize_context(
            &context,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "PIPESTATUS",
            budget(),
        )
        .expect("local fallback");
    assert!(view.degraded);
    assert_eq!(view.effective_strategy, "local_only_knowledge_degraded");
    assert!(
        view.items
            .iter()
            .any(|item| item.kind == agent_memory::protocol::ContextItemKind::Evidence)
    );
    let trace = backend
        .explain_context(&context, &view.context_view_id)
        .expect("degraded trace");
    assert!(trace.degraded);
    assert_eq!(
        trace.degradation_reason.as_deref(),
        Some("knowledge provider unavailable")
    );
}

#[test]
fn persists_trace_and_outcome_with_idempotent_aliases() {
    let root = tempfile::tempdir().expect("temp root");
    let path = database_path(root.path());
    let backend = LocalMemoryBackend::open(&path).expect("backend");
    let recall_context = context("user-a", "workspace-a", "session-a", "trace-recall");
    open(&backend, &recall_context);
    backend
        .checkpoint_task(&recall_context, "checkpoint", &task(1, "goal"), None, &[])
        .expect("checkpoint");
    let view = backend
        .materialize_context(
            &recall_context,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "query",
            budget(),
        )
        .expect("view");
    let admitted = vec![view.items[0].item_id.clone()];
    assert!(
        !backend
            .report_recall_outcome(
                &recall_context,
                "outcome-key",
                &view.context_view_id,
                &admitted,
                &[],
                FeedbackOutcome::Useful,
            )
            .expect("outcome")
    );
    drop(backend);

    let reopened = LocalMemoryBackend::open(&path).expect("reopen");
    open(&reopened, &recall_context);
    assert!(
        reopened
            .report_recall_outcome(
                &recall_context,
                "outcome-key",
                &view.context_view_id,
                &admitted,
                &[],
                FeedbackOutcome::Useful,
            )
            .expect("lost ack replay")
    );
    assert!(
        reopened
            .report_recall_outcome(
                &recall_context,
                "outcome-alias",
                &view.context_view_id,
                &admitted,
                &[],
                FeedbackOutcome::Useful,
            )
            .expect("equivalent alias")
    );
    let explain_context = context("user-a", "workspace-a", "session-a", "trace-explain");
    let trace = reopened
        .explain_context(&explain_context, &view.context_view_id)
        .expect("explain");
    assert_eq!(trace.trace_id, "trace-recall");
    assert_eq!(trace.response_trace_id, "trace-explain");
    assert_eq!(
        trace.outcome_report.expect("persisted outcome").outcome,
        FeedbackOutcome::Useful
    );
}

#[test]
fn rejects_idempotency_key_reuse_with_different_payloads() {
    let root = tempfile::tempdir().expect("temp root");
    let backend = LocalMemoryBackend::open(database_path(root.path())).expect("backend");
    let context = context("user-a", "workspace-a", "session-a", "trace-1");
    open(&backend, &context);

    backend
        .append_event(&context, "event-key", &event("first"))
        .expect("first event");
    assert_eq!(
        backend
            .append_event(&context, "event-key", &event("different"))
            .expect_err("event key conflict")
            .code,
        ProtocolErrorCode::Conflict
    );

    backend
        .checkpoint_task(&context, "task-key", &task(1, "first"), None, &[])
        .expect("first checkpoint");
    assert_eq!(
        backend
            .checkpoint_task(&context, "task-key", &task(1, "different"), None, &[])
            .expect_err("checkpoint key conflict")
            .code,
        ProtocolErrorCode::Conflict
    );

    let view = backend
        .materialize_context(
            &context,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "query",
            budget(),
        )
        .expect("view");
    let admitted = vec![view.items[0].item_id.clone()];
    backend
        .report_recall_outcome(
            &context,
            "outcome-key",
            &view.context_view_id,
            &admitted,
            &[],
            FeedbackOutcome::Useful,
        )
        .expect("first outcome");
    assert_eq!(
        backend
            .report_recall_outcome(
                &context,
                "outcome-key",
                &view.context_view_id,
                &admitted,
                &[],
                FeedbackOutcome::Harmful,
            )
            .expect_err("outcome key conflict")
            .code,
        ProtocolErrorCode::Conflict
    );

    backend
        .close_session(&context, "close-key", SessionOutcome::Completed)
        .expect("first close");
    assert_eq!(
        backend
            .close_session(&context, "close-key", SessionOutcome::Failed)
            .expect_err("close key conflict")
            .code,
        ProtocolErrorCode::Conflict
    );
}

#[test]
fn bounds_event_idempotency_aliases() {
    let root = tempfile::tempdir().expect("temp root");
    let backend = LocalMemoryBackend::open(database_path(root.path())).expect("backend");
    let context = context("user-a", "workspace-a", "session-a", "trace-1");
    open(&backend, &context);

    backend
        .append_event(&context, "event-key", &event("first"))
        .expect("first event");
    for alias in 1..8 {
        assert!(
            backend
                .append_event(&context, &format!("event-alias-{alias}"), &event("first"))
                .expect("bounded alias")
        );
    }
    assert_eq!(
        backend
            .append_event(&context, "event-alias-overflow", &event("first"))
            .expect_err("alias cap")
            .code,
        ProtocolErrorCode::ResourceExhausted
    );
}

#[test]
fn prunes_expired_views_and_closed_sessions() {
    let root = tempfile::tempdir().expect("temp root");
    let path = database_path(root.path());
    let backend = LocalMemoryBackend::open(&path).expect("backend");
    let first = context("user-a", "workspace-a", "session-a", "trace-1");
    open(&backend, &first);
    backend
        .append_event(&first, "event-key", &event("durable evidence"))
        .expect("event");
    backend
        .materialize_context(
            &first,
            RecallPurpose::SessionResume,
            &RecallBinding::default(),
            "resume",
            budget(),
        )
        .expect("old view");
    drop(backend);

    let connection = Connection::open(&path).expect("maintenance connection");
    connection
        .execute("UPDATE views SET created_at_ms = 0", [])
        .expect("age view");
    drop(connection);
    let reopened = LocalMemoryBackend::open(&path).expect("reopen backend");
    reopened
        .materialize_context(
            &first,
            RecallPurpose::SessionResume,
            &RecallBinding::default(),
            "resume",
            budget(),
        )
        .expect("prune and replace view");
    assert_eq!(reopened.stats().expect("view stats").view_count, 1);
    reopened
        .close_session(&first, "close-key", SessionOutcome::Completed)
        .expect("close session");
    drop(reopened);

    let connection = Connection::open(&path).expect("maintenance connection");
    connection
        .execute("UPDATE sessions SET closed_at_ms = 0", [])
        .expect("age session");
    drop(connection);
    let final_backend = LocalMemoryBackend::open(&path).expect("final backend");
    let second = context("user-a", "workspace-a", "session-b", "trace-2");
    open(&final_backend, &second);
    let stats = final_backend.stats().expect("pruned stats");
    assert_eq!(stats.session_count, 1);
    assert_eq!(stats.event_count, 0);
    assert_eq!(stats.view_count, 0);
}

#[test]
fn forget_cascades_indexes_without_cross_kind_leaks() {
    let root = tempfile::tempdir().expect("temp root");
    let backend = LocalMemoryBackend::open(database_path(root.path())).expect("backend");
    let context = context("user-a", "workspace-a", "session-a", "trace-1");
    open(&backend, &context);
    backend
        .append_event(&context, "event-key", &event("event"))
        .expect("event");
    backend
        .checkpoint_task(&context, "task-key", &task(1, "task"), None, &[])
        .expect("task");
    let view = backend
        .materialize_context(
            &context,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "query",
            budget(),
        )
        .expect("view");

    assert!(
        backend
            .forget(&context, MemoryObjectKind::Event, "event-1")
            .expect("forget event")
    );
    assert!(
        backend
            .forget(&context, MemoryObjectKind::Task, "task-1")
            .expect("forget task")
    );
    assert!(
        backend
            .forget(
                &context,
                MemoryObjectKind::ContextView,
                &view.context_view_id,
            )
            .expect("forget view")
    );
    assert!(
        !backend
            .forget(&context, MemoryObjectKind::Event, "event-1")
            .expect("repeat forget")
    );
    assert_eq!(
        backend
            .explain_context(&context, &view.context_view_id)
            .expect_err("forgotten view")
            .code,
        ProtocolErrorCode::NotFound
    );
    let stats = backend.stats().expect("stats");
    assert_eq!(stats.event_count, 0);
    assert_eq!(stats.task_count, 0);
    assert_eq!(stats.view_count, 0);
}

#[test]
fn management_lists_explains_and_cascades_owned_view() {
    let root = tempfile::tempdir().expect("temp root");
    let path = database_path(root.path());
    let backend = LocalMemoryBackend::open(&path).expect("backend");
    let owner = context("user-a", "workspace-a", "session-a", "trace-management");
    open(&backend, &owner);
    backend
        .checkpoint_task(&owner, "checkpoint", &task(1, "managed task"), None, &[])
        .expect("checkpoint");
    let view = backend
        .materialize_context(
            &owner,
            RecallPurpose::Turn,
            &RecallBinding::default(),
            "managed task",
            budget(),
        )
        .expect("materialize owned view");
    let admitted = view
        .items
        .iter()
        .map(|item| item.item_id.clone())
        .collect::<Vec<_>>();
    backend
        .report_recall_outcome(
            &owner,
            "management-outcome",
            &view.context_view_id,
            &admitted,
            &[],
            FeedbackOutcome::Useful,
        )
        .expect("report outcome");

    let management =
        LocalManagementContext::from_identity(&owner.identity).expect("management context");
    let summaries = backend
        .list_owned_views(&management, 10)
        .expect("list owned views");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].context_view_id, view.context_view_id);
    assert_eq!(summaries[0].outcome, Some(FeedbackOutcome::Useful));
    assert!(summaries[0].candidate_count >= summaries[0].admitted_count);

    let trace = backend
        .explain_owned_view(&management, &view.context_view_id)
        .expect("explain owned view");
    assert_eq!(trace.context_view_id, view.context_view_id);
    assert_eq!(
        trace.outcome_report.map(|report| report.outcome),
        Some(FeedbackOutcome::Useful)
    );
    assert!(
        backend
            .forget_owned(
                &management,
                MemoryObjectKind::ContextView,
                &view.context_view_id,
            )
            .expect("forget owned view")
    );
    assert!(
        backend
            .list_owned_views(&management, 10)
            .expect("list after forget")
            .is_empty()
    );
    assert_eq!(
        backend
            .explain_owned_view(&management, &view.context_view_id)
            .expect_err("forgotten trace must be absent")
            .code,
        ProtocolErrorCode::NotFound
    );

    let observer = Connection::open(&path).expect("observer connection");
    let view_rows: u64 = observer
        .query_row("SELECT COUNT(*) FROM views", [], |row| row.get(0))
        .expect("view rows");
    let outcome_rows: u64 = observer
        .query_row("SELECT COUNT(*) FROM outcome_idempotency", [], |row| {
            row.get(0)
        })
        .expect("outcome rows");
    assert_eq!(view_rows, 0);
    assert_eq!(outcome_rows, 0);
}

#[test]
fn management_keeps_cross_workspace_views_invisible() {
    let root = tempfile::tempdir().expect("temp root");
    let backend = LocalMemoryBackend::open(database_path(root.path())).expect("backend");
    let owner = context("user-a", "workspace-a", "session-a", "trace-owner-view");
    open(&backend, &owner);
    let view = backend
        .materialize_context(
            &owner,
            RecallPurpose::SessionResume,
            &RecallBinding::default(),
            "resume",
            budget(),
        )
        .expect("materialize owner view");
    let owner_management =
        LocalManagementContext::from_identity(&owner.identity).expect("owner management");
    let foreign = context("user-a", "workspace-b", "session-a", "trace-foreign-view");
    let foreign_management =
        LocalManagementContext::from_identity(&foreign.identity).expect("foreign management");

    assert!(
        backend
            .list_owned_views(&foreign_management, 10)
            .expect("foreign list")
            .is_empty()
    );
    assert_eq!(
        backend
            .explain_owned_view(&foreign_management, &view.context_view_id)
            .expect_err("foreign why must not reveal the view")
            .code,
        ProtocolErrorCode::NotFound
    );
    assert!(
        !backend
            .forget_owned(
                &foreign_management,
                MemoryObjectKind::ContextView,
                &view.context_view_id,
            )
            .expect("foreign forget must be indistinguishable from absent")
    );
    assert_eq!(
        backend
            .list_owned_views(&owner_management, 10)
            .expect("owner list")
            .len(),
        1
    );
    assert_eq!(
        backend
            .explain_owned_view(&owner_management, &view.context_view_id)
            .expect("owner why")
            .context_view_id,
        view.context_view_id
    );
}

#[test]
fn management_rejects_ambiguous_event_id_without_deleting() {
    let root = tempfile::tempdir().expect("temp root");
    let backend = LocalMemoryBackend::open(database_path(root.path())).expect("backend");
    let first = context("user-a", "workspace-a", "session-a", "trace-first-event");
    let second = context("user-a", "workspace-a", "session-b", "trace-second-event");
    open(&backend, &first);
    open(&backend, &second);
    backend
        .append_event(&first, "first-event", &event("first session event"))
        .expect("first event");
    backend
        .append_event(&second, "second-event", &event("second session event"))
        .expect("second event");
    let management =
        LocalManagementContext::from_identity(&first.identity).expect("management context");

    let error = backend
        .forget_owned(&management, MemoryObjectKind::Event, "event-1")
        .expect_err("ambiguous event identity must not be guessed");
    assert_eq!(error.code, ProtocolErrorCode::Conflict);
    assert_eq!(backend.stats().expect("stats").event_count, 2);
}

#[test]
fn rejects_newer_schema_without_disclosing_path() {
    let root = tempfile::tempdir().expect("temp root");
    let parent = root.path().join("private-memory");
    fs::create_dir(&parent).expect("private parent");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).expect("parent mode");
    let path = parent.join("newer.sqlite3");
    let connection = Connection::open(&path).expect("seed database");
    connection
        .pragma_update(None, "user_version", 2)
        .expect("newer version");
    drop(connection);

    let error = LocalMemoryBackend::open(&path).expect_err("newer schema must fail");
    assert_eq!(error.code, ProtocolErrorCode::VersionUnsupported);
    assert!(!error.safe_message.contains(path.to_string_lossy().as_ref()));
}
