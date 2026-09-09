use aw_adapters::{profiles, Host};
use aw_contracts::Registry;
use serde_json::json;

#[test]
fn six_hosts_map_exact_native_events_to_existing_aw_boundaries() {
    let registry = Registry::new().unwrap();
    for (host, events) in [
        (Host::Qoder, ["PreToolUse", "PostToolUse"]),
        (Host::Codex, ["PreToolUse", "PostToolUse"]),
        (Host::QwenCode, ["PreToolUse", "PostToolUse"]),
        (Host::Hermes, ["pre_tool_call", "post_tool_call"]),
        (Host::OpenClaw, ["before_tool_call", "after_tool_call"]),
        (Host::Cosh, ["PreToolUse", "PostToolUse"]),
    ] {
        let profile = profiles::profile(host).unwrap();
        assert_eq!(profile["host"], host.as_str());
        assert_eq!(profile["boundaries"].as_array().unwrap().len(), 2);
        for (event, phase) in events.into_iter().zip(["pre_tool", "post_tool"]) {
            let boundary = profiles::boundary(host, event).unwrap();
            registry.validate_boundary(&boundary).unwrap();
            assert_eq!(boundary["boundary"], phase);
            assert_eq!(boundary["can_deny_dispatch"], false);
            assert_eq!(boundary["has_final_input_guard"], false);
            assert_eq!(
                boundary["proof_boundaries"],
                if host == Host::Qoder && phase == "post_tool" {
                    json!(["local_history"])
                } else {
                    json!([])
                }
            );
            assert_eq!(boundary["ledger_policy"], "best_effort");
            assert_eq!(boundary["composition"]["gate"], "none");
            assert_eq!(
                boundary["composition"]["result_finality"],
                "subject_to_later_change"
            );
        }
        for event in ["", "pre_tool", "PRETOOLUSE", "Stop", "PostToolUseFailure"] {
            assert!(profiles::boundary(host, event).is_err());
        }
    }
}

#[test]
fn native_result_replacement_does_not_claim_adoption() {
    let qoder = profiles::boundary(Host::Qoder, "PostToolUse").unwrap();
    assert_eq!(qoder["can_replace_text"], true);
    assert_eq!(qoder["proof_boundaries"], json!(["local_history"]));
    assert_eq!(
        qoder["composition"]["result_finality"],
        "subject_to_later_change"
    );
    for (host, event) in [
        (Host::Codex, "PostToolUse"),
        (Host::QwenCode, "PostToolUse"),
        (Host::Hermes, "post_tool_call"),
        (Host::OpenClaw, "after_tool_call"),
        (Host::Cosh, "PostToolUse"),
    ] {
        let descriptor = profiles::boundary(host, event).unwrap();
        assert_eq!(descriptor["can_replace_text"], false);
        assert_eq!(descriptor["invocation_mode"], "observe_only");
    }
}
