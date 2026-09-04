//! Real Tokenless coverage for the COSH-to-AW adapter boundary.

use std::fs;
use std::path::{Path, PathBuf};

use aw_core::CapabilityPreferences;
use aw_cosh_hook::{local_host_target, run_cosh_post_tool_use, CoshHookConfig};
use aw_provider_host::{ProviderAdmissionOptions, ProviderManifestSource};
use serde_json::{json, Value};

#[test]
#[ignore = "requires a built tokenless binary for the current platform"]
fn cosh_keeps_source_when_tokenless_reports_information_loss() {
    let root = repository_root();
    let fixture: Value = serde_json::from_slice(
        &fs::read(root.join("providers/tokenless/fixtures/context-projection-prepare.json"))
            .expect("context fixture is readable"),
    )
    .expect("context fixture is valid JSON");
    let content = fixture
        .pointer("/artifact/content")
        .and_then(Value::as_str)
        .expect("fixture has model-visible content");
    let input = json!({
        "session_id": "11111111-1111-4111-8111-111111111111",
        "hook_event_name": "PostToolUse",
        "tool_use_id": "provider-call-demo",
        "tool_name": "list_recent_builds",
        "tool_response": {
            "llmContent": content,
            "returnDisplay": content,
        },
        "execution_scope": {
            "environment_id": "env_33333333-3333-4333-8333-333333333333",
            "execution_context_id": "ctx_44444444-4444-4444-8444-444444444444",
            "actor_id": "act_55555555-5555-4555-8555-555555555555",
            "agent_session_id": "ags_11111111-1111-4111-8111-111111111111",
            "turn_id": "trn_22222222-2222-4222-8222-222222222222",
            "tool_use_id": "tol_66666666-6666-4666-8666-666666666666"
        }
    });
    let mut output = Vec::new();
    let run = run_cosh_post_tool_use(
        serde_json::to_vec(&input)
            .expect("hook input serializes")
            .as_slice(),
        &mut output,
        &CoshHookConfig {
            provider_source: ProviderManifestSource::File(
                root.join("providers/tokenless/provider.toml"),
            ),
            provider_admission: ProviderAdmissionOptions {
                executable_roots: vec![root.join("src/tokenless/target/debug")],
            },
            target: local_host_target("test-host").expect("target is valid"),
            preferences: CapabilityPreferences::default(),
            provider_wall_time_ms: None,
            allow_unenforced_provider: true,
            ledger: None,
        },
    )
    .expect("COSH hook runs through AW Core");

    assert!(!run.replacement_requested);
    assert_eq!(
        run.receipts.len(),
        1,
        "the current PostToolUse plan holds one step"
    );
    let receipt = &run.receipts[0];
    assert_eq!(receipt.provider_id.as_str(), "tokenless");
    assert_eq!(
        receipt.scope.execution_context_id.as_str(),
        "ctx_44444444-4444-4444-8444-444444444444"
    );
    assert_eq!(
        receipt.scope.turn_id.as_ref().map(|id| id.as_str()),
        Some("trn_22222222-2222-4222-8222-222222222222")
    );
    assert_eq!(
        receipt.scope.tool_use_id.as_ref().map(|id| id.as_str()),
        Some("tol_66666666-6666-4666-8666-666666666666")
    );
    let receipt_json = serde_json::to_string(receipt).expect("receipt serializes");
    assert!(!receipt_json.contains("scheduler trace retained"));

    let output: Value = serde_json::from_slice(&output).expect("COSH hook output is valid JSON");
    assert_eq!(
        output.get("systemMessage").and_then(Value::as_str),
        Some("AW · security · 2 checks unavailable")
    );
    assert!(output.get("hookSpecificOutput").is_none());
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("adapter crate is nested below the repository root")
        .to_path_buf()
}
