use super::*;
use cosh_gateway::runtime::PinnedDirectory;
use cosh_gateway_contracts::common::{Digest, WorkspaceRef};

use super::acp_command::prompt_exit_code;

#[test]
fn prompt_stop_reasons_map_to_stable_exit_codes() {
    assert_eq!(prompt_exit_code(AcpV1StopReason::EndTurn), 0);
    assert_eq!(prompt_exit_code(AcpV1StopReason::Cancelled), EXIT_CANCELLED);
    for reason in [
        AcpV1StopReason::MaxTokens,
        AcpV1StopReason::MaxTurnRequests,
        AcpV1StopReason::Refusal,
        AcpV1StopReason::Unsupported,
    ] {
        assert_eq!(prompt_exit_code(reason), EXIT_AGENT);
    }
}

#[cfg(unix)]
#[test]
fn prompt_file_rejects_a_symlink() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("prompt.txt");
    let link = directory.path().join("prompt-link.txt");
    std::fs::write(&target, "inspect safely\n").unwrap();
    symlink(&target, &link).unwrap();

    let error = read_prompt(Some(&link)).unwrap_err();
    let CliError::Input(error) = error else {
        panic!("expected prompt input error")
    };
    assert_eq!(error.raw_os_error(), Some(nix::libc::ELOOP));
}

#[test]
fn terminal_text_escapes_control_sequences() {
    assert_eq!(terminal_safe("ok\u{1b}[2J\rnext"), "ok\\u{1b}[2J\\rnext");
}

#[test]
fn task_only_workspace_preserves_kernel_path_resolution() {
    let task_only = GatewayCapabilityProfile::task_only_v1();
    assert_eq!(
        super::serve::configured_workspace(
            task_only,
            Some(&PathBuf::from("/work/link/../project")),
        )
        .unwrap(),
        Path::new("/work/link/../project")
    );
}

#[cfg(unix)]
#[test]
fn task_only_link_parent_uses_kernel_resolution() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let actual = root.path().join("actual");
    let nested = actual.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let link = root.path().join("link");
    symlink(&nested, &link).unwrap();
    let configured = link.join("..");
    let preserved = super::serve::configured_workspace(
        GatewayCapabilityProfile::task_only_v1(),
        Some(&configured),
    )
    .unwrap();
    let target = GatewayCapabilityProfile::task_only_v1().governed_target();
    let resolved = TrustedWorkspaceResolver::new(target.clone(), preserved)
        .unwrap()
        .resolve(&target)
        .unwrap();

    assert_eq!(
        resolved.identity(),
        PinnedDirectory::pin(actual).unwrap().identity()
    );
    assert_ne!(
        resolved.identity(),
        PinnedDirectory::pin(root.path()).unwrap().identity()
    );
}

#[test]
fn checkpoint_workspace_rejects_dot_components() {
    let checkpoint = GatewayCapabilityProfile::workspace_checkpoint_v1();
    for path in ["/work/./project", "/work/link/../project"] {
        assert!(
            super::serve::configured_workspace(checkpoint, Some(&PathBuf::from(path))).is_err()
        );
    }
    assert_eq!(
        super::serve::configured_workspace(checkpoint, Some(&PathBuf::from("/work/project")))
            .unwrap(),
        Path::new("/work/project")
    );
}

#[test]
fn json_observation_fields_include_driver_sequence() {
    assert_eq!(
        with_observation_sequence(7, json!({"text": "chunk"})),
        json!({"sequence": 7, "text": "chunk"})
    );
}

#[test]
fn cli_does_not_accept_prompt_as_an_argument() {
    assert!(Cli::try_parse_from(["cosh-gateway", "run", "secret prompt"]).is_err());
}

#[test]
fn task_submit_does_not_accept_intent_as_an_argument() {
    assert!(Cli::try_parse_from(["cosh-gateway", "task", "submit", "private intent"]).is_err());
}

#[test]
fn task_admission_is_an_explicit_read_only_command() {
    let cli = Cli::try_parse_from([
        "cosh-gateway",
        "task",
        "--socket",
        "/run/user/1000/cosh/poc.sock",
        "--output",
        "jsonl",
        "admission",
    ])
    .unwrap();

    assert!(matches!(
        cli.command,
        Command::Task(TaskArgs {
            command: TaskCommand::Admission,
            ..
        })
    ));
}

#[test]
fn task_event_page_is_bounded_by_clap() {
    assert!(Cli::try_parse_from([
        "cosh-gateway",
        "task",
        "events",
        "tsk_00000000-0000-0000-0000-000000000000",
        "--limit",
        "65",
    ])
    .is_err());
}

#[test]
fn task_submit_uses_the_exact_daemon_admission_for_each_profile() {
    let defaults = Cli::try_parse_from([
        "cosh-gateway",
        "task",
        "submit",
        "--idempotency-key",
        "stable-submit-key",
    ])
    .unwrap();
    let Command::Task(TaskArgs {
        command: TaskCommand::Submit(defaults),
        ..
    }) = defaults.command
    else {
        panic!("expected task submit command");
    };
    assert_eq!(defaults.runtime, "core");
    assert_eq!(
        defaults.runtime_profile,
        GATEWAY_BROKERED_CORE_RUNTIME_PROFILE
    );
    let admitted = |profile: GatewayCapabilityProfile| cosh_gateway::daemon::GatewayAdmission {
        installation_id: InstallationId::new(),
        capability_profile: profile.identity(),
        target: profile.governed_target(),
        workspace: WorkspaceRef {
            scope_digest: Digest::parse("9".repeat(64)).unwrap(),
            display_name: None,
        },
        runtime: RuntimeSelector {
            runtime: BoundedName::new("core").unwrap(),
            profile: Some(BoundedName::new(GATEWAY_BROKERED_CORE_RUNTIME_PROFILE).unwrap()),
        },
    };
    for profile in [
        GatewayCapabilityProfile::task_only_v1(),
        GatewayCapabilityProfile::workspace_checkpoint_v1(),
    ] {
        let admission = admitted(profile);
        let (target, runtime) = control::admitted_submission_scope(&defaults, &admission).unwrap();
        assert_eq!(target, profile.governed_target());
        assert_eq!(runtime, admission.runtime);
    }
    let mut drifted = admitted(GatewayCapabilityProfile::task_only_v1());
    drifted.capability_profile.manifest_digest = Digest::parse("8".repeat(64)).unwrap();
    assert!(matches!(
        control::admitted_submission_scope(&defaults, &drifted),
        Err(CliError::Profile(_))
    ));

    let explicit = Cli::try_parse_from([
        "cosh-gateway",
        "task",
        "submit",
        "--idempotency-key",
        "explicit-acp-key",
        "--runtime",
        "acp",
        "--runtime-profile",
        "codex",
    ])
    .unwrap();
    let Command::Task(TaskArgs {
        command: TaskCommand::Submit(explicit),
        ..
    }) = explicit.command
    else {
        panic!("expected explicit task submit command");
    };
    assert_eq!(explicit.runtime, "acp");
    assert_eq!(explicit.runtime_profile, "codex");
}

#[test]
fn task_submit_rejects_hand_assembled_target_flags() {
    for removed in [
        vec!["--target-kind", "workspace"],
        vec!["--target-authority", "ws-ckpt"],
        vec!["--target", "checkpoint-create-v1"],
    ] {
        let parsed = Cli::try_parse_from(
            [
                "cosh-gateway",
                "task",
                "submit",
                "--idempotency-key",
                "fixed-target-key",
            ]
            .into_iter()
            .chain(removed.iter().copied()),
        );
        assert!(
            parsed.is_err(),
            "removed target flags must not parse: {removed:?}"
        );
    }
}

#[test]
fn task_approval_decision_needs_no_internal_ledger_revision() {
    let cli = Cli::try_parse_from([
        "cosh-gateway",
        "task",
        "resolve-approval",
        "apr_00000000-0000-0000-0000-000000000000",
        "--decision",
        "approve",
        "--idempotency-key",
        "stable-approval-key",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Command::Task(TaskArgs {
            command: TaskCommand::ResolveApproval(TaskResolveApprovalArgs {
                decision: ApprovalChoice::Approve,
                ..
            }),
            ..
        })
    ));
    assert!(Cli::try_parse_from([
        "cosh-gateway",
        "task",
        "resolve-approval",
        "apr_00000000-0000-0000-0000-000000000000",
        "--decision",
        "approve",
        "--idempotency-key",
        "stable-approval-key",
        "--expected-revision",
        "1",
    ])
    .is_err());
}

#[test]
fn task_append_parses_exact_input_identity_and_bounded_options() {
    let cli = Cli::try_parse_from([
        "cosh-gateway",
        "task",
        "append",
        "tsk_00000000-0000-0000-0000-000000000000",
        "--input-request-id",
        "inp_00000000-0000-0000-0000-000000000001",
        "--select",
        "0",
        "--select",
        "2",
        "--idempotency-key",
        "stable-input-key",
        "--expected-revision",
        "5",
    ])
    .unwrap();
    let Command::Task(TaskArgs {
        command: TaskCommand::Append(append),
        ..
    }) = cli.command
    else {
        panic!("expected task append command");
    };
    assert_eq!(
        append.input_request_id,
        "inp_00000000-0000-0000-0000-000000000001"
    );
    assert_eq!(append.selections, vec![0, 2]);
    assert_eq!(append.expected_revision, Some(5));

    assert!(Cli::try_parse_from([
        "cosh-gateway",
        "task",
        "append",
        "tsk_00000000-0000-0000-0000-000000000000",
        "--input-request-id",
        "inp_00000000-0000-0000-0000-000000000001",
        "--input-file",
        "/tmp/input",
        "--select",
        "0",
        "--idempotency-key",
        "conflicting-input-source",
    ])
    .is_err());
}

#[test]
fn task_retry_requires_exact_previous_run_and_stable_key() {
    let cli = Cli::try_parse_from([
        "cosh-gateway",
        "task",
        "retry",
        "tsk_00000000-0000-0000-0000-000000000000",
        "--previous-run-id",
        "run_00000000-0000-0000-0000-000000000001",
        "--idempotency-key",
        "stable-retry-key",
        "--expected-revision",
        "4",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Command::Task(TaskArgs {
            command: TaskCommand::Retry(TaskRetryArgs {
                expected_revision: Some(4),
                ..
            }),
            ..
        })
    ));
    assert!(Cli::try_parse_from([
        "cosh-gateway",
        "task",
        "retry",
        "tsk_00000000-0000-0000-0000-000000000000",
        "--idempotency-key",
        "stable-retry-key",
    ])
    .is_err());
}
