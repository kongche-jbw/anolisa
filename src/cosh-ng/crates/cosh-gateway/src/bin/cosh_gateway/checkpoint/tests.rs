use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::thread;

use cosh_gateway::capability::{ExecutionTarget, ExecutionTargetOutcome};
use cosh_gateway::daemon::ScheduledRun;
use cosh_gateway::storage::{
    BrokerExecutionState, BrokeredRequestRecord, ExecutionRecord, ExecutionState,
    TypedExecutionResultState,
};
use cosh_gateway_contracts::capability::{
    BrokeredOperation, CapabilityRequest, CapabilityScope, OperationDescriptor,
    RuntimeExecutionFence, WorkspaceCheckpointCreateV1,
};
use cosh_gateway_contracts::common::{
    ActorKind, ActorRef, AuthAssurance, BoundedName, BoundedOpaque, BoundedText, Digest,
    RuntimeSelector, TargetRef, WorkspaceRef,
};
use cosh_gateway_contracts::ids::{
    ActorId, ExecutionId, RequestId, RunId, RuntimeBindingId, TaskId, ToolUseId, TurnId,
};
use cosh_gateway_contracts::runtime::{
    BrokeredExecutionRef, BrokeredOperationResult, ToolSummary, WorkspaceCheckpointCreateV1Outcome,
};
use cosh_types::checkpoint::{
    GuardedCheckpointEvidenceV2, GuardedCheckpointOutcomeV2, GuardedCheckpointRejectionCodeV2,
    WorkspaceGenerationTokenV2, WsCkptRequest, WsCkptResponse,
};

use super::*;

enum DaemonReply {
    Response(WsCkptResponse),
}

fn spawn_daemon(
    replies: Vec<DaemonReply>,
) -> (
    tempfile::TempDir,
    String,
    thread::JoinHandle<Vec<WsCkptRequest>>,
) {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let socket_path = directory.path().join("ws-ckpt.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let mut requests = Vec::with_capacity(replies.len());
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            let mut length = [0_u8; 4];
            stream.read_exact(&mut length).unwrap();
            let mut payload = vec![0_u8; u32::from_le_bytes(length) as usize];
            stream.read_exact(&mut payload).unwrap();
            requests.push(bincode::deserialize(&payload).unwrap());

            let DaemonReply::Response(response) = reply;
            let payload = bincode::serialize(&response).unwrap();
            stream
                .write_all(&(payload.len() as u32).to_le_bytes())
                .unwrap();
            stream.write_all(&payload).unwrap();
        }
        requests
    });
    (
        directory,
        socket_path.to_string_lossy().into_owned(),
        handle,
    )
}

fn digest(byte: u8) -> Digest {
    Digest::parse(format!("{byte:02x}").repeat(32)).unwrap()
}

fn operation() -> CheckpointOperation {
    CheckpointOperation {
        target: TargetRef {
            kind: BoundedName::new("local").unwrap(),
            authority: BoundedName::new("cosh").unwrap(),
            identifier: BoundedOpaque::new("primary").unwrap(),
        },
        target_identity_digest: digest(8),
        runtime_fence: RuntimeExecutionFence {
            binding_id: RuntimeBindingId::new(),
            runtime_generation: 11,
            lease_generation: 12,
            lease_revision: 13,
        },
        operation_digest: digest(9),
        input_digest: digest(10),
        checkpoint_id: CheckpointId::new(),
        binding: CheckpointBinding {
            version: BINDING_VERSION,
            protocol_version: GUARDED_CHECKPOINT_PROTOCOL_VERSION_V2,
            socket_device: 21,
            socket_inode: 34,
            daemon_uid: nix::unistd::Uid::effective().as_raw(),
            runtime_workspace_device: 55,
            runtime_workspace_inode: 89,
            ws_id: "ws-abc123".to_owned(),
            registered_path: "/workspace".to_owned(),
            generation: [7; 32],
            owner_uid: nix::unistd::Uid::effective().as_raw(),
            permit_id: PermitId::new(),
            execution_id: ExecutionId::new(),
        },
    }
}

fn evidence(operation: &CheckpointOperation) -> GuardedCheckpointEvidenceV2 {
    GuardedCheckpointEvidenceV2 {
        ws_id: operation.binding.ws_id.clone(),
        registered_path: operation.binding.registered_path.clone(),
        generation: WorkspaceGenerationTokenV2::from_bytes(operation.binding.generation),
        checkpoint_id: operation.checkpoint_id.as_str().to_owned(),
        operation_digest: digest_bytes(&operation.operation_digest).unwrap(),
        caller_uid: operation.binding.owner_uid,
        outcome: GuardedCheckpointOutcomeV2::Created {
            snapshot_id: operation.checkpoint_id.as_str().to_owned(),
        },
    }
}

fn execute(
    operation: &CheckpointOperation,
    replies: Vec<DaemonReply>,
) -> (ExecutionTargetOutcome, Vec<WsCkptRequest>) {
    let (_directory, socket_path, daemon) = spawn_daemon(replies);
    let client = CkptClient::with_timeout(&socket_path, 1_000)
        .require_trusted_peer(operation.binding.owner_uid);
    let mut target = CheckpointTarget { client: &client };
    let outcome = target.execute(operation);
    (outcome, daemon.join().unwrap())
}

fn assert_guarded_request(request: &WsCkptRequest, operation: &CheckpointOperation) {
    let WsCkptRequest::GuardedCheckpointV2 {
        ws_id,
        expected_generation,
        checkpoint_id,
        operation_digest,
        message,
        metadata,
        pin,
    } = request
    else {
        panic!("expected one guarded checkpoint request")
    };
    assert_eq!(ws_id, &operation.binding.ws_id);
    assert_eq!(
        expected_generation,
        &WorkspaceGenerationTokenV2::from_bytes(operation.binding.generation)
    );
    assert_eq!(checkpoint_id, operation.checkpoint_id.as_str());
    assert_eq!(
        operation_digest,
        &digest_bytes(&operation.operation_digest).unwrap()
    );
    assert_eq!(message.as_deref(), Some("COSH governed Task checkpoint"));
    assert_eq!(metadata, &None);
    assert!(!pin);
}

fn assert_evidence_request(request: &WsCkptRequest, operation: &CheckpointOperation) {
    let WsCkptRequest::CheckpointEvidenceV2 {
        ws_id,
        expected_generation,
        checkpoint_id,
        operation_digest,
    } = request
    else {
        panic!("expected one read-only evidence request")
    };
    assert_eq!(ws_id, &operation.binding.ws_id);
    assert_eq!(
        expected_generation,
        &WorkspaceGenerationTokenV2::from_bytes(operation.binding.generation)
    );
    assert_eq!(checkpoint_id, operation.checkpoint_id.as_str());
    assert_eq!(
        operation_digest,
        &digest_bytes(&operation.operation_digest).unwrap()
    );
}

#[test]
fn admission_binds_the_guarded_workspace_and_target_identity() {
    let workspace = tempfile::tempdir().unwrap();
    fs::set_permissions(workspace.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let live_path = workspace.path().join("live");
    fs::create_dir(&live_path).unwrap();
    let registration_path = workspace.path().join("workspace");
    symlink(&live_path, &registration_path).unwrap();
    let registered_path = registration_path.to_str().unwrap();
    let runtime_workspace = PinnedDirectory::pin(&registration_path).unwrap();
    let runtime_workspace_identity = runtime_workspace.identity();
    let generation = WorkspaceGenerationTokenV2::from_bytes([7; 32]);
    let (directory, socket_path, daemon) = spawn_daemon(vec![DaemonReply::Response(
        WsCkptResponse::WorkspaceIdentityV2Ok {
            protocol_version: GUARDED_CHECKPOINT_PROTOCOL_VERSION_V2,
            ws_id: "ws-abc123".to_owned(),
            registered_path: registered_path.to_owned(),
            generation,
        },
    )]);
    let audit_path = directory.path().join("security-audit.jsonl");
    let profile = GatewayCapabilityProfile::workspace_checkpoint_v1();
    let driver = CheckpointDriver::admit_with_generation(
        profile,
        PathBuf::from(&socket_path),
        Path::new(registered_path),
        runtime_workspace,
        generation.into_bytes(),
        audit_path.clone(),
        nix::unistd::Uid::effective().as_raw(),
    )
    .unwrap();
    let requests = daemon.join().unwrap();

    assert!(matches!(
        requests.as_slice(),
        [WsCkptRequest::WorkspaceIdentityV2 { registration_path }]
            if registration_path == registered_path
    ));
    let binding = CheckpointBinding {
        version: BINDING_VERSION,
        protocol_version: GUARDED_CHECKPOINT_PROTOCOL_VERSION_V2,
        socket_device: driver.socket_identity.device,
        socket_inode: driver.socket_identity.inode,
        daemon_uid: driver.socket_identity.daemon_uid,
        runtime_workspace_device: runtime_workspace_identity.device(),
        runtime_workspace_inode: runtime_workspace_identity.inode(),
        ws_id: "ws-abc123".to_owned(),
        registered_path: registered_path.to_owned(),
        generation: generation.into_bytes(),
        owner_uid: nix::unistd::Uid::effective().as_raw(),
        permit_id: PermitId::new(),
        execution_id: ExecutionId::new(),
    };
    let target_digest = driver.target_digest(&binding).unwrap();
    let expected = digest_parts(&[
        TARGET_DOMAIN,
        profile.id().as_str().as_bytes(),
        profile.manifest_digest().as_str().as_bytes(),
        CapabilityProviderId::WsCkpt.as_str().as_bytes(),
        driver.target.kind.as_str().as_bytes(),
        driver.target.authority.as_str().as_bytes(),
        driver.target.identifier.as_str().as_bytes(),
        driver.socket_path.as_os_str().as_encoded_bytes(),
        &driver.socket_identity.device.to_le_bytes(),
        &driver.socket_identity.inode.to_le_bytes(),
        &driver.socket_identity.daemon_uid.to_le_bytes(),
        &binding.runtime_workspace_device.to_le_bytes(),
        &binding.runtime_workspace_inode.to_le_bytes(),
        &binding.version.to_le_bytes(),
        &binding.protocol_version.to_le_bytes(),
        binding.ws_id.as_bytes(),
        binding.registered_path.as_bytes(),
        &binding.generation,
        &binding.owner_uid.to_le_bytes(),
        binding.permit_id.as_str().as_bytes(),
        binding.execution_id.as_str().as_bytes(),
    ])
    .unwrap();
    assert_eq!(target_digest, expected);
    let mut changed_generation = binding;
    changed_generation.generation[0] ^= 1;
    assert_ne!(
        driver.target_digest(&changed_generation).unwrap(),
        target_digest
    );
    let mut changed_runtime_workspace = changed_generation.clone();
    changed_runtime_workspace.runtime_workspace_inode ^= 1;
    assert_ne!(
        driver.target_digest(&changed_runtime_workspace).unwrap(),
        driver.target_digest(&changed_generation).unwrap()
    );
    let mut changed_authority = changed_runtime_workspace.clone();
    changed_authority.execution_id = ExecutionId::new();
    assert_ne!(
        driver.target_digest(&changed_authority).unwrap(),
        driver.target_digest(&changed_runtime_workspace).unwrap()
    );
    assert_eq!(
        fs::read_to_string(audit_path).unwrap(),
        "{\"schema\":\"cosh.gateway.security-audit-log.v1\"}\n"
    );

    fs::remove_file(&registration_path).unwrap();
    let replacement = workspace.path().join("replacement");
    fs::create_dir(&replacement).unwrap();
    symlink(replacement, &registration_path).unwrap();
    let error = driver.verify_runtime_workspace_unchanged().unwrap_err();
    assert_eq!(error.code.as_str(), "checkpoint_workspace_changed");
}

#[test]
fn admission_rejects_daemon_identity_for_another_symlink_target() {
    let workspace = tempfile::tempdir().unwrap();
    fs::set_permissions(workspace.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let first = workspace.path().join("first");
    let second = workspace.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let registration_path = workspace.path().join("workspace");
    symlink(&second, &registration_path).unwrap();
    let pinned_second = PinnedDirectory::pin(&registration_path).unwrap();
    fs::remove_file(&registration_path).unwrap();
    symlink(&first, &registration_path).unwrap();
    let registered_path = registration_path.to_str().unwrap();
    let first_generation = WorkspaceGenerationTokenV2::from_bytes([1; 32]);
    let second_generation = [2; 32];
    let (directory, socket_path, daemon) = spawn_daemon(vec![DaemonReply::Response(
        WsCkptResponse::WorkspaceIdentityV2Ok {
            protocol_version: GUARDED_CHECKPOINT_PROTOCOL_VERSION_V2,
            ws_id: "ws-first".to_owned(),
            registered_path: registered_path.to_owned(),
            generation: first_generation,
        },
    )]);

    let result = CheckpointDriver::admit_with_generation(
        GatewayCapabilityProfile::workspace_checkpoint_v1(),
        PathBuf::from(socket_path),
        &registration_path,
        pinned_second,
        second_generation,
        directory.path().join("security-audit.jsonl"),
        nix::unistd::Uid::effective().as_raw(),
    );
    let requests = daemon.join().unwrap();

    assert!(matches!(result, Err(CheckpointAdmissionError::Identity)));
    assert!(matches!(
        requests.as_slice(),
        [WsCkptRequest::WorkspaceIdentityV2 { registration_path }]
            if registration_path == registered_path
    ));
}

#[test]
fn approval_plan_persists_preallocated_permit_and_execution_ids() {
    let workspace = tempfile::tempdir().unwrap();
    fs::set_permissions(workspace.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let runtime_workspace = PinnedDirectory::pin(workspace.path()).unwrap();
    let generation = WorkspaceGenerationTokenV2::from_bytes([7; 32]);
    let identity_reply = || {
        DaemonReply::Response(WsCkptResponse::WorkspaceIdentityV2Ok {
            protocol_version: GUARDED_CHECKPOINT_PROTOCOL_VERSION_V2,
            ws_id: "ws-abc123".to_owned(),
            registered_path: workspace.path().to_string_lossy().into_owned(),
            generation,
        })
    };
    let (directory, socket_path, daemon) = spawn_daemon(vec![identity_reply(), identity_reply()]);
    let profile = GatewayCapabilityProfile::workspace_checkpoint_v1();
    let mut driver = CheckpointDriver::admit_with_generation(
        profile,
        PathBuf::from(socket_path),
        workspace.path(),
        runtime_workspace,
        generation.into_bytes(),
        directory.path().join("security-audit.jsonl"),
        nix::unistd::Uid::effective().as_raw(),
    )
    .unwrap();
    let actor = ActorRef {
        actor_id: ActorId::new(),
        actor_kind: ActorKind::Human,
        issuer: BoundedName::new("local-os").unwrap(),
        assurance: AuthAssurance::LocalOs,
    };
    let task_id = TaskId::new();
    let run_id = RunId::new();
    let request_id = RequestId::new();
    let checkpoint_id = CheckpointId::new();
    let operation = BrokeredOperation::WorkspaceCheckpointCreateV1(WorkspaceCheckpointCreateV1 {
        checkpoint_id,
    });
    let request = CapabilityRequest {
        request_id: request_id.clone(),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        actor: actor.clone(),
        target: profile.governed_target(),
        operation: OperationDescriptor {
            namespace: BoundedName::new("workspace").unwrap(),
            name: BoundedName::new("checkpoint_create").unwrap(),
            arguments_digest: digest(1),
        },
        operation_digest: digest(2),
        requested_scope: CapabilityScope {
            resource: BoundedName::new("workspace").unwrap(),
            access: BoundedName::new("checkpoint_create").unwrap(),
        },
        input_digest: digest(3),
        expires_at_ms: 10_000,
    };
    let runtime_fence = RuntimeExecutionFence {
        binding_id: RuntimeBindingId::new(),
        runtime_generation: 5,
        lease_generation: 6,
        lease_revision: 7,
    };
    let scheduled = ScheduledRun {
        actor,
        task_id,
        run_id: run_id.clone(),
        runtime: RuntimeSelector {
            runtime: BoundedName::new("core").unwrap(),
            profile: Some(BoundedName::new("gateway-brokered-v1").unwrap()),
        },
        intent: BoundedText::new("create a checkpoint").unwrap(),
        target: profile.governed_target(),
        workspace: WorkspaceRef {
            scope_digest: digest(4),
            display_name: None,
        },
        capability_profile: profile.identity(),
        lease_generation: 6,
    };
    let brokered = BrokeredExecutionRef {
        binding_id: runtime_fence.binding_id.clone(),
        runtime_generation: runtime_fence.runtime_generation,
        event_sequence: 8,
        run_id,
        turn_id: TurnId::new(),
        tool_use_id: Some(ToolUseId::new()),
        request_id,
        operation: operation.clone(),
    };
    let summary = ToolSummary {
        name: BoundedName::new("workspace_checkpoint_create").unwrap(),
        summary: BoundedText::new("Create one workspace checkpoint").unwrap(),
    };

    let plan = driver
        .plan_approval(BrokeredApprovalContext {
            scheduled: &scheduled,
            brokered: &brokered,
            request: &request,
            operation: &operation,
            summary: &summary,
            runtime_fence: &runtime_fence,
            now_ms: 1,
        })
        .unwrap();
    let binding: CheckpointBinding = serde_json::from_str(
        plan.provider_binding
            .as_ref()
            .expect("checkpoint plan must persist its authority IDs")
            .as_str(),
    )
    .unwrap();
    let requests = daemon.join().unwrap();

    assert_eq!(binding.version, BINDING_VERSION);
    assert_eq!(
        plan.target_identity_digest,
        driver.target_digest(&binding).unwrap()
    );
    assert!(!binding.permit_id.as_str().is_empty());
    assert!(!binding.execution_id.as_str().is_empty());
    assert_eq!(requests.len(), 2);
}

#[test]
fn task_only_profile_never_reaches_checkpoint_socket_admission() {
    let workspace = tempfile::tempdir().unwrap();
    let runtime_workspace = PinnedDirectory::pin(workspace.path()).unwrap();
    let result = CheckpointDriver::admit(
        GatewayCapabilityProfile::task_only_v1(),
        PathBuf::from("/does/not/exist.sock"),
        Path::new("/workspace"),
        runtime_workspace,
        PathBuf::from("/does/not/exist-audit.jsonl"),
        nix::unistd::Uid::effective().as_raw(),
    );

    assert!(matches!(result, Err(CheckpointAdmissionError::Profile)));
}

#[test]
fn created_checkpoint_commits_the_exact_typed_receipt() {
    let operation = operation();
    let evidence = evidence(&operation);
    let expected_receipt = digest_parts(&[
        RECEIPT_DOMAIN,
        operation.target_identity_digest.as_str().as_bytes(),
        operation.operation_digest.as_str().as_bytes(),
        operation.checkpoint_id.as_str().as_bytes(),
        &evidence.generation.into_bytes(),
        &serde_json::to_vec(&evidence).unwrap(),
    ])
    .unwrap();
    let (outcome, requests) = execute(
        &operation,
        vec![DaemonReply::Response(
            WsCkptResponse::GuardedCheckpointV2Ok {
                evidence: evidence.clone(),
            },
        )],
    );

    let ExecutionTargetOutcome::Conclusive {
        succeeded,
        receipt_digest,
        typed_result,
        ..
    } = outcome
    else {
        panic!("created checkpoint must be conclusive")
    };
    assert!(succeeded);
    assert_eq!(receipt_digest, expected_receipt);
    assert!(matches!(
        typed_result,
        Some(BrokeredOperationResult::WorkspaceCheckpointCreateV1(result))
            if result.checkpoint_id == operation.checkpoint_id
                && matches!(
                    result.outcome,
                    WorkspaceCheckpointCreateV1Outcome::Created { ref snapshot_id }
                        if snapshot_id.as_str() == operation.checkpoint_id.as_str()
                )
    ));
    assert_eq!(requests.len(), 1);
    assert_guarded_request(&requests[0], &operation);
}

#[test]
fn possibly_applied_reconciles_with_evidence_without_replay() {
    let operation = operation();
    let evidence = evidence(&operation);
    let mut mismatched_evidence = evidence.clone();
    mismatched_evidence.operation_digest = [6; 32];
    let (outcome, requests) = execute(
        &operation,
        vec![
            DaemonReply::Response(WsCkptResponse::GuardedCheckpointV2Ok {
                evidence: mismatched_evidence,
            }),
            DaemonReply::Response(WsCkptResponse::CheckpointEvidenceV2Ok {
                evidence: Some(evidence),
            }),
        ],
    );

    assert!(matches!(
        outcome,
        ExecutionTargetOutcome::Conclusive {
            succeeded: true,
            typed_result: Some(BrokeredOperationResult::WorkspaceCheckpointCreateV1(_)),
            ..
        }
    ));
    assert_eq!(requests.len(), 2);
    assert_guarded_request(&requests[0], &operation);
    assert_evidence_request(&requests[1], &operation);
}

#[test]
fn missing_reconcile_evidence_is_unknown_and_never_replayed() {
    let operation = operation();
    let mut mismatched_evidence = evidence(&operation);
    mismatched_evidence.operation_digest = [6; 32];
    let (outcome, requests) = execute(
        &operation,
        vec![
            DaemonReply::Response(WsCkptResponse::GuardedCheckpointV2Ok {
                evidence: mismatched_evidence,
            }),
            DaemonReply::Response(WsCkptResponse::CheckpointEvidenceV2Ok { evidence: None }),
        ],
    );

    assert!(matches!(outcome, ExecutionTargetOutcome::Unknown { .. }));
    assert_eq!(requests.len(), 2);
    assert_guarded_request(&requests[0], &operation);
    assert_evidence_request(&requests[1], &operation);
}

#[test]
fn mismatched_reconcile_evidence_is_unknown() {
    let operation = operation();
    let mut first = evidence(&operation);
    first.operation_digest = [6; 32];
    let mut second = evidence(&operation);
    second.checkpoint_id = CheckpointId::new().to_string();
    let (outcome, requests) = execute(
        &operation,
        vec![
            DaemonReply::Response(WsCkptResponse::GuardedCheckpointV2Ok { evidence: first }),
            DaemonReply::Response(WsCkptResponse::CheckpointEvidenceV2Ok {
                evidence: Some(second),
            }),
        ],
    );

    assert!(matches!(outcome, ExecutionTargetOutcome::Unknown { .. }));
    assert_eq!(requests.len(), 2);
    assert_guarded_request(&requests[0], &operation);
    assert_evidence_request(&requests[1], &operation);
}

#[test]
fn restart_reconciliation_issues_only_the_evidence_query() {
    let operation = operation();
    let evidence = evidence(&operation);
    let (_directory, socket_path, daemon) = spawn_daemon(vec![DaemonReply::Response(
        WsCkptResponse::CheckpointEvidenceV2Ok {
            evidence: Some(evidence),
        },
    )]);
    let client = CkptClient::with_timeout(&socket_path, 1_000)
        .require_trusted_peer(operation.binding.owner_uid);

    let outcome = reconcile_evidence(&client, &operation);
    let requests = daemon.join().unwrap();

    assert!(matches!(
        outcome,
        ExecutionTargetOutcome::Conclusive {
            succeeded: true,
            ..
        }
    ));
    assert_eq!(requests.len(), 1);
    assert_evidence_request(&requests[0], &operation);
}

#[test]
fn recovery_accepts_historical_binding_after_listener_and_workspace_replacement() {
    let mut operation = operation();
    let runtime_workspace = tempfile::tempdir().unwrap();
    fs::set_permissions(runtime_workspace.path(), fs::Permissions::from_mode(0o700)).unwrap();
    operation.binding.registered_path = "/historical/workspace".to_owned();
    let evidence = evidence(&operation);
    let (directory, socket_path, daemon) = spawn_daemon(vec![DaemonReply::Response(
        WsCkptResponse::CheckpointEvidenceV2Ok {
            evidence: Some(evidence),
        },
    )]);
    let socket_path = PathBuf::from(socket_path);
    let gateway_uid = nix::unistd::Uid::effective().as_raw();
    let profile = GatewayCapabilityProfile::workspace_checkpoint_v1();
    let target = profile.governed_target();
    let current_runtime_workspace = PinnedDirectory::pin(runtime_workspace.path()).unwrap();
    let mut driver = CheckpointDriver {
        client: CkptClient::with_timeout(socket_path.to_str().unwrap(), 1_000)
            .require_trusted_peer(gateway_uid),
        socket_identity: verify_socket_trust(&socket_path, gateway_uid).unwrap(),
        socket_path,
        registration_path: runtime_workspace.path().to_string_lossy().into_owned(),
        runtime_workspace: current_runtime_workspace,
        runtime_generation: [9; 32],
        gateway_uid,
        profile,
        target: target.clone(),
        audit_file: open_audit_file(&directory.path().join("security-audit.jsonl"), gateway_uid)
            .unwrap(),
    };
    operation.binding.socket_device = driver.socket_identity.device ^ 1;
    operation.binding.socket_inode = driver.socket_identity.inode ^ 1;
    operation.binding.daemon_uid = driver.socket_identity.daemon_uid;
    operation.binding.runtime_workspace_device ^= 1;
    operation.binding.runtime_workspace_inode ^= 1;
    operation.target = target;
    operation.target_identity_digest = driver.target_digest(&operation.binding).unwrap();
    let actor_id = ActorId::new();
    let task_id = TaskId::new();
    let run_id = RunId::new();
    let request = BrokeredRequestRecord {
        request: CapabilityRequest {
            request_id: RequestId::new(),
            task_id: task_id.clone(),
            run_id: run_id.clone(),
            actor: ActorRef {
                actor_id: actor_id.clone(),
                actor_kind: ActorKind::Human,
                issuer: BoundedName::new("local-os").unwrap(),
                assurance: AuthAssurance::LocalOs,
            },
            target: operation.target.clone(),
            operation: OperationDescriptor {
                namespace: BoundedName::new("workspace").unwrap(),
                name: BoundedName::new("checkpoint_create").unwrap(),
                arguments_digest: digest(11),
            },
            operation_digest: operation.operation_digest.clone(),
            requested_scope: CapabilityScope {
                resource: BoundedName::new("workspace").unwrap(),
                access: BoundedName::new("checkpoint_create").unwrap(),
            },
            input_digest: operation.input_digest.clone(),
            expires_at_ms: 10_000,
        },
        operation: BrokeredOperation::WorkspaceCheckpointCreateV1(WorkspaceCheckpointCreateV1 {
            checkpoint_id: operation.checkpoint_id.clone(),
        }),
        typed_operation_digest: digest(12),
        target_identity_digest: operation.target_identity_digest.clone(),
        runtime_fence: operation.runtime_fence.clone(),
        provider_binding: Some(
            BoundedOpaque::new(serde_json::to_string(&operation.binding).unwrap()).unwrap(),
        ),
        approval_id: None,
        created_at_ms: 1,
    };
    let execution = ExecutionRecord {
        execution_id: operation.binding.execution_id.clone(),
        actor_id,
        task_id,
        run_id,
        target: operation.target.clone(),
        target_identity_digest: Some(operation.target_identity_digest.clone()),
        runtime_fence: Some(operation.runtime_fence.clone()),
        broker_state: Some(BrokerExecutionState::Started),
        claimed_at_ms: Some(2),
        start_audit_proof_digest: Some(digest(13)),
        typed_result_state: TypedExecutionResultState::NotApplicable,
        operation_digest: operation.operation_digest.clone(),
        input_digest: operation.input_digest.clone(),
        state: ExecutionState::Started,
        revision: 3,
        started_at_ms: Some(3),
        completed_at_ms: None,
        created_at_ms: 1,
        updated_at_ms: 3,
    };

    let outcome = driver.reconcile_started(BrokeredRecoveryContext {
        execution: &execution,
        request: &request,
    });
    let requests = daemon.join().unwrap();

    assert!(matches!(
        outcome,
        ExecutionTargetOutcome::Conclusive {
            succeeded: true,
            ..
        }
    ));
    assert_eq!(requests.len(), 1);
    assert_evidence_request(&requests[0], &operation);
}

#[test]
fn explicit_v2_rejection_is_a_conclusive_failure() {
    let operation = operation();
    let (outcome, requests) = execute(
        &operation,
        vec![DaemonReply::Response(
            WsCkptResponse::GuardedCheckpointV2Rejected {
                code: GuardedCheckpointRejectionCodeV2::GenerationMismatch,
                message: "daemon-private generation detail".to_owned(),
            },
        )],
    );

    let ExecutionTargetOutcome::Conclusive {
        succeeded,
        safe_detail,
        typed_result,
        ..
    } = outcome
    else {
        panic!("an explicit V2 pre-effect rejection must be conclusive")
    };
    assert!(!succeeded);
    assert!(typed_result.is_none());
    let safe_detail = safe_detail.unwrap();
    assert!(!safe_detail.as_str().contains("daemon-private"));
    assert_eq!(requests.len(), 1);
    assert_guarded_request(&requests[0], &operation);
}
