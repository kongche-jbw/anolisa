use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_memory::protocol::{
    BackendManifest, BackendRequestContext, ContextBudget, ContextItem, ContextItemKind,
    ContextView, EphemeralMemoryBackend, EvidenceRef, FeedbackOutcome, IdentityContext,
    MEMORY_PROTOCOL_VERSION, MemoryAuthority, MemoryBackend, MemoryCapability, MemoryDurability,
    MemoryEvent, MemoryEventKind, MemoryEventOutcome, MemoryObjectKind, MemoryRequest,
    MemoryRequestEnvelope, MemoryResponse, MemoryWireResponse, ProtocolErrorCode, ProtocolResult,
    RecallBinding, RecallDecision, RecallPurpose, RecallTrace, RuntimeContext, SessionOutcome,
    TaskState, dispatch, schema_bundle,
};

fn identity(workspace: &str) -> IdentityContext {
    IdentityContext {
        tenant_id: None,
        team_id: None,
        user_id: "unix:1000".to_string(),
        agent_id: "cosh-ng".to_string(),
        session_id: "session-1".to_string(),
        workspace_id: workspace.to_string(),
    }
}

fn envelope(
    identity: IdentityContext,
    request_id: &str,
    request: MemoryRequest,
) -> MemoryRequestEnvelope {
    MemoryRequestEnvelope {
        protocol_version: MEMORY_PROTOCOL_VERSION,
        request_id: request_id.to_string(),
        trace_id: format!("trace-{request_id}"),
        run_id: Some("run-1".to_string()),
        task_id: None,
        turn_id: None,
        deadline_at_ms: None,
        identity,
        request,
    }
}

fn open(backend: &EphemeralMemoryBackend, identity: &IdentityContext) -> MemoryWireResponse {
    dispatch(
        backend,
        envelope(
            identity.clone(),
            "open-1",
            MemoryRequest::OpenSession {
                runtime: RuntimeContext {
                    runtime: "cosh-ng".to_string(),
                    runtime_version: Some("0.19.0".to_string()),
                    model: None,
                    platform: Some("linux".to_string()),
                },
            },
        ),
    )
}

#[test]
fn schema_bundle_contains_both_wire_envelopes() {
    let schema = schema_bundle();
    assert_eq!(schema["protocol"], "anolisa.agent-memory");
    assert_eq!(schema["version"], MEMORY_PROTOCOL_VERSION);
    assert!(schema["request"].is_object());
    assert!(schema["response"].is_object());
}

#[test]
fn request_validation_rejects_version_identity_and_unknown_fields() {
    let backend = EphemeralMemoryBackend::default();
    let mut request = envelope(
        identity("workspace-1"),
        "request-1",
        MemoryRequest::Negotiate { required: vec![] },
    );
    request.protocol_version = MEMORY_PROTOCOL_VERSION + 1;
    assert!(matches!(
        dispatch(&backend, request),
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::VersionUnsupported,
                ..
            },
            ..
        }
    ));

    let mut missing_identity = envelope(
        identity("workspace-1"),
        "request-2",
        MemoryRequest::Negotiate { required: vec![] },
    );
    missing_identity.identity.agent_id.clear();
    assert!(matches!(
        dispatch(&backend, missing_identity),
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::InvalidRequest,
                ..
            },
            ..
        }
    ));

    let value = serde_json::json!({
        "protocol_version": 1,
        "request_id": "request-3",
        "trace_id": "trace-request-3",
        "run_id": null,
        "task_id": null,
        "turn_id": null,
        "deadline_at_ms": null,
        "unexpected": true,
        "identity": {
            "tenant_id": null,
            "team_id": null,
            "user_id": "unix:1000",
            "agent_id": "cosh-ng",
            "session_id": "session-1",
            "workspace_id": "workspace-1"
        },
        "request": {"operation": "negotiate", "input": {"required": []}}
    });
    assert!(serde_json::from_value::<MemoryRequestEnvelope>(value).is_err());
}

#[test]
fn negotiation_fails_when_required_capability_is_missing() {
    let backend = EphemeralMemoryBackend::default();
    let response = dispatch(
        &backend,
        envelope(
            identity("workspace-1"),
            "negotiate-1",
            MemoryRequest::Negotiate {
                required: vec![MemoryCapability::Knowledge],
            },
        ),
    );
    assert!(matches!(
        response,
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::UnsupportedCapability,
                ..
            },
            ..
        }
    ));
}

#[test]
fn response_types_tolerate_additive_fields_and_unknown_values() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/protocol/v1/negotiate-response.json"))
            .expect("fixture is JSON");
    let manifest = &mut value["response"]["output"]["manifest"];
    manifest["future_status"] = serde_json::json!({"ready": true});
    manifest["capabilities"]
        .as_array_mut()
        .expect("capabilities is an array")
        .push(serde_json::json!("future_capability"));
    let response: MemoryWireResponse =
        serde_json::from_value(value).expect("additive response remains compatible");
    assert!(matches!(
        response,
        MemoryWireResponse::Ok {
            response: MemoryResponse::Negotiated { manifest },
            ..
        } if manifest.capabilities.contains(&MemoryCapability::Other(
            "future_capability".to_string()
        ))
    ));
}

#[derive(Clone)]
struct SlowBackend {
    entered: Arc<AtomicBool>,
}

impl MemoryBackend for SlowBackend {
    fn manifest(&self) -> BackendManifest {
        test_manifest(vec![MemoryCapability::Session])
    }

    fn open_session(
        &self,
        _context: &BackendRequestContext,
        _runtime: &RuntimeContext,
    ) -> ProtocolResult<bool> {
        self.entered.store(true, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(80));
        Ok(false)
    }
}

#[test]
fn deadline_elapsed_inside_backend_discards_success() {
    let entered = Arc::new(AtomicBool::new(false));
    let backend = SlowBackend {
        entered: Arc::clone(&entered),
    };
    let mut request = envelope(
        identity("workspace-1"),
        "slow-open",
        MemoryRequest::OpenSession {
            runtime: RuntimeContext {
                runtime: "test".to_string(),
                runtime_version: None,
                model: None,
                platform: None,
            },
        },
    );
    request.deadline_at_ms = Some(now_ms().saturating_add(50));
    assert!(matches!(
        dispatch(&backend, request),
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::DeadlineExceeded,
                ..
            },
            ..
        }
    ));
    assert!(entered.load(Ordering::SeqCst));
}

struct InvalidContextBackend;

impl MemoryBackend for InvalidContextBackend {
    fn manifest(&self) -> BackendManifest {
        test_manifest(vec![MemoryCapability::Recall, MemoryCapability::Explain])
    }

    fn materialize_context(
        &self,
        context: &BackendRequestContext,
        _purpose: RecallPurpose,
        _binding: &RecallBinding,
        query: &str,
        _budget: ContextBudget,
    ) -> ProtocolResult<ContextView> {
        Ok(ContextView {
            context_view_id: "invalid-view".to_string(),
            trace_id: context.trace_id.clone(),
            snapshot_revision: 1,
            query: query.to_string(),
            items: vec![ContextItem {
                item_id: "item-1".to_string(),
                revision: None,
                kind: ContextItemKind::Evidence,
                content: "four".to_string(),
                source_ref: "test://item-1".to_string(),
                authority: MemoryAuthority::Candidate,
                token_estimate: 1,
                reason: "test".to_string(),
                stale: false,
                score: 1.0,
            }],
            total_tokens: 1,
            total_bytes: 1,
            effective_strategy: "invalid".to_string(),
            degraded: false,
            truncated: false,
            created_at_ms: 1,
        })
    }

    fn explain_context(
        &self,
        context: &BackendRequestContext,
        _context_view_id: &str,
    ) -> ProtocolResult<RecallTrace> {
        Ok(RecallTrace {
            context_view_id: "wrong-view".to_string(),
            trace_id: context.trace_id.clone(),
            response_trace_id: context.trace_id.clone(),
            backend_id: "invalid".to_string(),
            decisions: vec![RecallDecision {
                item_id: "item-1".to_string(),
                admitted: true,
                reason: "invalid score".to_string(),
                rank: 1,
                score: f32::NAN,
            }],
            degraded: false,
            degradation_reason: None,
            outcome_report: None,
        })
    }
}

#[test]
fn dispatch_rejects_backend_context_that_violates_budget_postconditions() {
    let response = dispatch(
        &InvalidContextBackend,
        envelope(
            identity("workspace-1"),
            "invalid-context",
            MemoryRequest::MaterializeContext {
                purpose: RecallPurpose::Turn,
                binding: RecallBinding::default(),
                query: "query".to_string(),
                budget: ContextBudget {
                    max_tokens: 16,
                    max_bytes: 64,
                    max_items: 2,
                },
            },
        ),
    );
    assert!(matches!(
        response,
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::IntegrityFailed,
                ..
            },
            ..
        }
    ));
}

#[test]
fn dispatch_rejects_backend_trace_with_invalid_correlation_or_scores() {
    let response = dispatch(
        &InvalidContextBackend,
        envelope(
            identity("workspace-1"),
            "invalid-trace",
            MemoryRequest::ExplainContext {
                context_view_id: "expected-view".to_string(),
            },
        ),
    );
    assert!(matches!(
        response,
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::IntegrityFailed,
                ..
            },
            ..
        }
    ));
}

fn test_manifest(capabilities: Vec<MemoryCapability>) -> BackendManifest {
    BackendManifest {
        backend_id: "test".to_string(),
        display_name: "Test backend".to_string(),
        protocol_version: MEMORY_PROTOCOL_VERSION,
        capabilities,
        durability: MemoryDurability::ProcessLocal,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[test]
fn elapsed_deadline_is_rejected_before_backend_dispatch() {
    let backend = EphemeralMemoryBackend::default();
    let mut request = envelope(
        identity("workspace-1"),
        "expired-1",
        MemoryRequest::Negotiate { required: vec![] },
    );
    request.deadline_at_ms = Some(1);
    assert!(matches!(
        dispatch(&backend, request),
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::DeadlineExceeded,
                ..
            },
            ..
        }
    ));
}

#[test]
fn handoff_requires_matching_task_and_target_bindings() {
    let backend = EphemeralMemoryBackend::default();
    let mut request = envelope(
        identity("workspace-1"),
        "handoff-1",
        MemoryRequest::MaterializeContext {
            purpose: RecallPurpose::Handoff,
            binding: RecallBinding {
                task_id: Some("task-a".to_string()),
                target_agent_id: Some("agent-b".to_string()),
            },
            query: "continue the task".to_string(),
            budget: ContextBudget {
                max_tokens: 64,
                max_bytes: 256,
                max_items: 2,
            },
        },
    );
    request.task_id = Some("task-b".to_string());
    assert!(matches!(
        dispatch(&backend, request),
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::InvalidRequest,
                ..
            },
            ..
        }
    ));
}

#[test]
fn close_session_is_idempotent_after_the_session_is_removed() {
    let backend = EphemeralMemoryBackend::default();
    let scope = identity("workspace-1");
    assert!(matches!(
        open(&backend, &scope),
        MemoryWireResponse::Ok { .. }
    ));
    let close = |request_id: &str| {
        dispatch(
            &backend,
            envelope(
                scope.clone(),
                request_id,
                MemoryRequest::CloseSession {
                    idempotency_key: "close-session-1".to_string(),
                    outcome: SessionOutcome::Completed,
                },
            ),
        )
    };
    assert!(matches!(
        close("close-1"),
        MemoryWireResponse::Ok {
            response: MemoryResponse::SessionClosed { replayed: false },
            ..
        }
    ));
    assert!(matches!(
        close("close-1-retry"),
        MemoryWireResponse::Ok {
            response: MemoryResponse::SessionClosed { replayed: true },
            ..
        }
    ));
}

#[test]
fn ephemeral_backend_checkpoints_materializes_and_explains() {
    let backend = EphemeralMemoryBackend::default();
    let scope = identity("workspace-1");
    assert!(matches!(
        open(&backend, &scope),
        MemoryWireResponse::Ok {
            response: MemoryResponse::SessionOpened { resumed: false },
            ..
        }
    ));

    let task = TaskState {
        task_id: "task-1".to_string(),
        revision: 1,
        goal: "Make Bash and Zsh behavior consistent".to_string(),
        next_action: Some("Run the POSIX fixture".to_string()),
        blockers: vec!["Confirm shell version".to_string()],
        updated_at_ms: 1,
    };
    let evidence = vec![EvidenceRef {
        provider: "test".to_string(),
        uri: "fixture://posix".to_string(),
        digest: Some("sha256:abc".to_string()),
        summary: "Bash fixture passed".to_string(),
    }];
    let checkpoint = dispatch(
        &backend,
        envelope(
            scope.clone(),
            "checkpoint-1",
            MemoryRequest::CheckpointTask {
                idempotency_key: "checkpoint-task-1".to_string(),
                task: task.clone(),
                expected_revision: None,
                evidence: evidence.clone(),
            },
        ),
    );
    assert!(matches!(
        checkpoint,
        MemoryWireResponse::Ok {
            response: MemoryResponse::TaskCheckpointed { revision: 1, .. },
            ..
        }
    ));

    let replayed_checkpoint = dispatch(
        &backend,
        envelope(
            scope.clone(),
            "checkpoint-1-retry",
            MemoryRequest::CheckpointTask {
                idempotency_key: "checkpoint-task-1".to_string(),
                task,
                expected_revision: None,
                evidence,
            },
        ),
    );
    assert!(matches!(
        replayed_checkpoint,
        MemoryWireResponse::Ok {
            response: MemoryResponse::TaskCheckpointed {
                revision: 1,
                replayed: true,
                ..
            },
            ..
        }
    ));

    let stale_checkpoint = dispatch(
        &backend,
        envelope(
            scope.clone(),
            "checkpoint-stale",
            MemoryRequest::CheckpointTask {
                idempotency_key: "checkpoint-stale-task-1".to_string(),
                task: TaskState {
                    task_id: "task-1".to_string(),
                    revision: 1,
                    goal: "Overwrite a concurrent update".to_string(),
                    next_action: None,
                    blockers: vec![],
                    updated_at_ms: 2,
                },
                expected_revision: None,
                evidence: vec![],
            },
        ),
    );
    assert!(matches!(
        stale_checkpoint,
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::Conflict,
                ..
            },
            ..
        }
    ));

    let revision_two = dispatch(
        &backend,
        envelope(
            scope.clone(),
            "checkpoint-2",
            MemoryRequest::CheckpointTask {
                idempotency_key: "checkpoint-task-2".to_string(),
                task: TaskState {
                    task_id: "task-1".to_string(),
                    revision: 2,
                    goal: "Make Bash and Zsh behavior consistent".to_string(),
                    next_action: Some("Run the Zsh fixture".to_string()),
                    blockers: vec![],
                    updated_at_ms: 3,
                },
                expected_revision: Some(1),
                evidence: vec![],
            },
        ),
    );
    assert!(matches!(
        revision_two,
        MemoryWireResponse::Ok {
            response: MemoryResponse::TaskCheckpointed {
                revision: 2,
                replayed: false,
                ..
            },
            ..
        }
    ));

    let materialized = dispatch(
        &backend,
        envelope(
            scope.clone(),
            "recall-1",
            MemoryRequest::MaterializeContext {
                purpose: RecallPurpose::Turn,
                binding: RecallBinding::default(),
                query: "continue compatibility work".to_string(),
                budget: ContextBudget {
                    max_tokens: 512,
                    max_bytes: 2048,
                    max_items: 4,
                },
            },
        ),
    );
    let view = match materialized {
        MemoryWireResponse::Ok {
            response: MemoryResponse::ContextMaterialized { view },
            ..
        } => view,
        other => panic!("expected materialized context, got {other:?}"),
    };
    assert_eq!(view.items.len(), 1);
    assert_eq!(view.items[0].revision, Some(2));
    assert!(view.total_tokens <= 512);
    assert!(view.items[0].content.contains("Run the Zsh fixture"));

    let incomplete_feedback = dispatch(
        &backend,
        envelope(
            scope.clone(),
            "feedback-incomplete",
            MemoryRequest::ReportRecallOutcome {
                idempotency_key: "feedback-incomplete".to_string(),
                context_view_id: view.context_view_id.clone(),
                admitted_item_ids: vec![],
                dropped_item_ids: vec![],
                outcome: FeedbackOutcome::Useful,
            },
        ),
    );
    assert!(matches!(
        incomplete_feedback,
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::Conflict,
                ..
            },
            ..
        }
    ));

    let feedback = dispatch(
        &backend,
        envelope(
            scope.clone(),
            "feedback-1",
            MemoryRequest::ReportRecallOutcome {
                idempotency_key: "feedback-task-1".to_string(),
                context_view_id: view.context_view_id.clone(),
                admitted_item_ids: vec![view.items[0].item_id.clone()],
                dropped_item_ids: vec![],
                outcome: FeedbackOutcome::Useful,
            },
        ),
    );
    assert!(matches!(
        feedback,
        MemoryWireResponse::Ok {
            response: MemoryResponse::FeedbackRecorded {
                replayed: false,
                ..
            },
            ..
        }
    ));

    let feedback_replay = dispatch(
        &backend,
        envelope(
            scope.clone(),
            "feedback-1-retry",
            MemoryRequest::ReportRecallOutcome {
                idempotency_key: "feedback-task-1".to_string(),
                context_view_id: view.context_view_id.clone(),
                admitted_item_ids: vec![view.items[0].item_id.clone()],
                dropped_item_ids: vec![],
                outcome: FeedbackOutcome::Useful,
            },
        ),
    );
    assert!(matches!(
        feedback_replay,
        MemoryWireResponse::Ok {
            response: MemoryResponse::FeedbackRecorded { replayed: true, .. },
            ..
        }
    ));

    let mut other_session = scope.clone();
    other_session.session_id = "session-2".to_string();
    assert!(matches!(
        open(&backend, &other_session),
        MemoryWireResponse::Ok { .. }
    ));
    let cross_session_explain = dispatch(
        &backend,
        envelope(
            other_session,
            "cross-session-explain",
            MemoryRequest::ExplainContext {
                context_view_id: view.context_view_id.clone(),
            },
        ),
    );
    assert!(matches!(
        cross_session_explain,
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::NotFound,
                ..
            },
            ..
        }
    ));

    let explained = dispatch(
        &backend,
        envelope(
            scope.clone(),
            "explain-1",
            MemoryRequest::ExplainContext {
                context_view_id: view.context_view_id.clone(),
            },
        ),
    );
    let trace = match explained {
        MemoryWireResponse::Ok {
            response: MemoryResponse::ContextExplained { trace },
            ..
        } => trace,
        other => panic!("expected recall trace, got {other:?}"),
    };
    assert_eq!(
        trace
            .outcome_report
            .expect("feedback is reflected in explain")
            .outcome,
        FeedbackOutcome::Useful
    );

    for (kind, memory_id) in [
        (MemoryObjectKind::ContextView, view.context_view_id),
        (MemoryObjectKind::Task, "task-1".to_string()),
    ] {
        assert!(matches!(
            dispatch(
                &backend,
                envelope(
                    scope.clone(),
                    "forget-task-artifact",
                    MemoryRequest::Forget { kind, memory_id },
                ),
            ),
            MemoryWireResponse::Ok {
                response: MemoryResponse::Forgotten { deleted: true, .. },
                ..
            }
        ));
    }
}

#[test]
fn ephemeral_backend_isolates_tenants_with_identical_local_ids() {
    let backend = EphemeralMemoryBackend::default();
    let mut tenant_a = identity("workspace-1");
    tenant_a.tenant_id = Some("tenant-a".to_string());
    let mut tenant_b = tenant_a.clone();
    tenant_b.tenant_id = Some("tenant-b".to_string());
    assert!(matches!(
        open(&backend, &tenant_a),
        MemoryWireResponse::Ok { .. }
    ));
    assert!(matches!(
        open(&backend, &tenant_b),
        MemoryWireResponse::Ok { .. }
    ));

    let checkpoint = dispatch(
        &backend,
        envelope(
            tenant_a,
            "tenant-a-checkpoint",
            MemoryRequest::CheckpointTask {
                idempotency_key: "tenant-a-task".to_string(),
                task: TaskState {
                    task_id: "shared-task-id".to_string(),
                    revision: 1,
                    goal: "Tenant A private goal".to_string(),
                    next_action: None,
                    blockers: vec![],
                    updated_at_ms: 1,
                },
                expected_revision: None,
                evidence: vec![],
            },
        ),
    );
    assert!(matches!(checkpoint, MemoryWireResponse::Ok { .. }));

    let recalled = dispatch(
        &backend,
        envelope(
            tenant_b,
            "tenant-b-recall",
            MemoryRequest::MaterializeContext {
                purpose: RecallPurpose::Turn,
                binding: RecallBinding::default(),
                query: "private goal".to_string(),
                budget: ContextBudget {
                    max_tokens: 128,
                    max_bytes: 512,
                    max_items: 4,
                },
            },
        ),
    );
    assert!(matches!(
        recalled,
        MemoryWireResponse::Ok {
            response: MemoryResponse::ContextMaterialized {
                view: agent_memory::protocol::ContextView { items, .. }
            },
            ..
        } if items.is_empty()
    ));
}

#[test]
fn event_idempotency_replays_equal_content_and_rejects_conflict() {
    let backend = EphemeralMemoryBackend::default();
    let scope = identity("workspace-1");
    open(&backend, &scope);
    let event = MemoryEvent {
        event_id: "event-1".to_string(),
        kind: MemoryEventKind::ToolCompleted,
        source: "cosh-ng".to_string(),
        outcome: MemoryEventOutcome::Succeeded,
        observed_at_ms: 1,
        summary: "compatibility fixture passed".to_string(),
        evidence_ref: Some("fixture://result/1".to_string()),
    };
    let append = |request_id: &str, idempotency_key: &str, event: MemoryEvent| {
        dispatch(
            &backend,
            envelope(
                scope.clone(),
                request_id,
                MemoryRequest::AppendEvent {
                    idempotency_key: idempotency_key.to_string(),
                    event,
                },
            ),
        )
    };
    assert!(matches!(
        append("append-1", "tool-call-1", event.clone()),
        MemoryWireResponse::Ok {
            response: MemoryResponse::EventAccepted {
                replayed: false,
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        append("append-2", "tool-call-1", event.clone()),
        MemoryWireResponse::Ok {
            response: MemoryResponse::EventAccepted { replayed: true, .. },
            ..
        }
    ));
    assert!(matches!(
        append("append-2-alias", "tool-call-alias", event.clone()),
        MemoryWireResponse::Ok {
            response: MemoryResponse::EventAccepted { replayed: true, .. },
            ..
        }
    ));
    let mut conflicting = event;
    conflicting.summary = "different result".to_string();
    assert!(matches!(
        append("append-3", "tool-call-1", conflicting),
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: ProtocolErrorCode::Conflict,
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        dispatch(
            &backend,
            envelope(
                scope,
                "forget-event",
                MemoryRequest::Forget {
                    kind: MemoryObjectKind::Event,
                    memory_id: "event-1".to_string(),
                },
            ),
        ),
        MemoryWireResponse::Ok {
            response: MemoryResponse::Forgotten { deleted: true, .. },
            ..
        }
    ));
}

#[test]
fn golden_wire_fixtures_remain_compatible() {
    let request: MemoryRequestEnvelope =
        serde_json::from_str(include_str!("fixtures/protocol/v1/negotiate-request.json"))
            .expect("golden request follows protocol v1");
    assert_eq!(request.protocol_version, MEMORY_PROTOCOL_VERSION);
    assert_eq!(request.request_id, "golden-negotiate-1");

    let response: MemoryWireResponse =
        serde_json::from_str(include_str!("fixtures/protocol/v1/negotiate-response.json"))
            .expect("golden response follows protocol v1");
    assert!(matches!(
        response,
        MemoryWireResponse::Ok { request_id, .. } if request_id == "golden-negotiate-1"
    ));
}
