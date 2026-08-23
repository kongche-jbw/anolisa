use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use agent_memory::protocol::{
    BackendRequestContext, ContextBudget, EvidenceRef, FeedbackOutcome, IdentityContext,
    LocalMemoryBackend, MemoryBackend, MemoryCapability, MemoryDurability, MemoryEvent,
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
