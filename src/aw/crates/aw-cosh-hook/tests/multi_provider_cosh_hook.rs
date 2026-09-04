//! Real Agent Sec and Tokenless coverage for the COSH-to-AW boundary.

use std::fs;
use std::path::{Path, PathBuf};

use aw_contracts::context::ContextReversibility;
use aw_contracts::ledger::{LedgerEventKind, PostToolUsePlanBody};
use aw_contracts::provider::{
    ProviderDisposition, ProviderMeasurementKind, ProviderMeter, ProviderReceipt,
};
use aw_contracts::security::{SecurityDetectedLanguage, SecurityInspectionVerdict};
use aw_core::CapabilityPreferences;
use aw_cosh_hook::{
    local_host_target, run_cosh_post_tool_use, CoshHookConfig, LedgerAssurance, LedgerSpec,
};
use aw_ledger::LedgerStore;
use aw_provider_host::{ProviderAdmissionOptions, ProviderManifestSource};
use serde_json::{json, Value};

const EXPECTED_REPLACEMENT: &str = "builds[6]{id,project,status,duration_ms,owner}:\n  \
    \"build-101\",\"checkout-service\",passed,48231,\"example-team\"\n  \
    \"build-102\",\"catalog-service\",passed,39502,\"example-team\"\n  \
    \"build-103\",\"inventory-service\",failed,61402,\"example-team\"\n  \
    \"build-104\",\"payment-service\",passed,52710,\"example-team\"\n  \
    \"build-105\",\"notification-service\",passed,44617,\"example-team\"\n  \
    \"build-106\",\"reporting-service\",passed,37331,\"example-team\"\n\
    page: 1\npage_size: 6";

#[test]
#[ignore = "requires built Tokenless and agent-sec-core executables for this platform"]
fn cosh_requests_a_lossless_replacement_after_two_clean_observations() {
    let root = repository_root();
    let fixture: Value = serde_json::from_slice(
        &fs::read(
            root.join("providers/tokenless/fixtures/context-projection-prepare-lossless.json"),
        )
        .expect("lossless context fixture is readable"),
    )
    .expect("lossless context fixture is valid JSON");
    let source = fixture
        .pointer("/artifact/content")
        .and_then(Value::as_str)
        .expect("fixture has model-visible content");
    let source_digest = fixture
        .pointer("/artifact/digest")
        .and_then(Value::as_str)
        .expect("fixture has a source digest");
    assert_eq!(source.len(), 693, "the example's byte count is intentional");
    let tool_use_id = "tol_66666666-6666-4666-8666-666666666666";
    let input = json!({
        "session_id": "11111111-1111-4111-8111-111111111111",
        "hook_event_name": "PostToolUse",
        "tool_use_id": "provider-call-demo",
        "tool_name": "list_recent_builds",
        "tool_response": {
            "llmContent": source,
            "returnDisplay": source,
        },
        "execution_scope": {
            "environment_id": "env_33333333-3333-4333-8333-333333333333",
            "execution_context_id": "ctx_44444444-4444-4444-8444-444444444444",
            "actor_id": "act_55555555-5555-4555-8555-555555555555",
            "agent_session_id": "ags_11111111-1111-4111-8111-111111111111",
            "turn_id": "trn_22222222-2222-4222-8222-222222222222",
            "tool_use_id": tool_use_id
        }
    });
    let ledger_dir = tempfile::tempdir().expect("temporary Ledger directory is available");
    let mut output = Vec::new();

    let run = run_cosh_post_tool_use(
        serde_json::to_vec(&input)
            .expect("hook input serializes")
            .as_slice(),
        &mut output,
        &CoshHookConfig {
            provider_source: ProviderManifestSource::Directory(root.join("providers")),
            provider_admission: ProviderAdmissionOptions {
                executable_roots: vec![
                    root.join("src/tokenless/target/debug"),
                    root.join("src/agent-sec-core/agent-sec-cli/.venv/bin"),
                ],
            },
            target: local_host_target("test-host").expect("target is valid"),
            preferences: CapabilityPreferences::default(),
            provider_wall_time_ms: Some(5_000),
            allow_unenforced_provider: true,
            ledger: Some(LedgerSpec {
                root: ledger_dir.path().to_path_buf(),
                assurance: LedgerAssurance::Required,
            }),
        },
    )
    .expect("COSH runs the real three-step AW plan");

    assert!(
        run.replacement_requested,
        "the hook asks COSH to replace the result; COSH still owns final delivery"
    );
    assert!(!run.ledger_unavailable);
    assert!(run.observation_gaps.is_empty());
    assert_eq!(run.receipts.len(), 3, "two Observe and one Advise receipts");
    assert_security_receipt(&run.receipts[0], "security.content.inspect", source.len());
    assert_security_receipt(&run.receipts[1], "security.code.inspect", source.len());
    assert_tokenless_receipt(&run.receipts[2]);

    let response: Value = serde_json::from_slice(&output).expect("COSH hook output is valid JSON");
    assert_eq!(
        response.get("suppressOutput").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        response.get("systemMessage").and_then(Value::as_str),
        Some("AW · tokenless · estimated context 174→110 tokens · saved 37%")
    );
    assert_eq!(
        response
            .pointer("/hookSpecificOutput/hookEventName")
            .and_then(Value::as_str),
        Some("PostToolUse")
    );
    assert_eq!(
        response
            .pointer("/hookSpecificOutput/updatedToolResponse")
            .and_then(Value::as_str),
        Some(EXPECTED_REPLACEMENT)
    );

    let record = run
        .ledger
        .as_ref()
        .expect("the required Ledger append settled");
    assert_eq!(record.sequence, 0);
    let store = LedgerStore::open(ledger_dir.path()).expect("Ledger reopens");
    assert_eq!(
        aw_ledger::verify_chain(&store).expect("Ledger chain verifies"),
        1
    );
    let stored = store
        .record_by_id(&record.event_id)
        .expect("Ledger query succeeds")
        .expect("hook record is present");
    assert_eq!(stored.header.kind, LedgerEventKind::PostToolUsePlan);
    assert_eq!(stored.record_digest, record.record_digest);
    assert_eq!(
        stored
            .scope
            .as_ref()
            .and_then(|scope| scope.tool_use_id.as_ref())
            .map(|id| id.as_str()),
        Some(tool_use_id)
    );

    let body_bytes = store
        .record_body_bytes(&record.event_id)
        .expect("Ledger body is readable");
    let body: PostToolUsePlanBody =
        serde_json::from_slice(&body_bytes).expect("Ledger body follows its typed contract");
    assert_eq!(body.source_digest.as_str(), source_digest);
    assert!(body.observation_gaps.is_empty());
    assert_eq!(body.observations.len(), 2);
    let content_observation = &body.observations[0];
    assert_eq!(
        content_observation.capability.id.as_str(),
        "security.content.inspect"
    );
    assert_clean_observation(content_observation, source.len(), None);
    let code_observation = &body.observations[1];
    assert_eq!(
        code_observation.capability.id.as_str(),
        "security.code.inspect"
    );
    assert_clean_observation(
        code_observation,
        source.len(),
        Some(SecurityDetectedLanguage::Mixed),
    );
    assert!(body.projection.candidate_offered);
    assert_eq!(
        body.projection
            .media_type
            .as_ref()
            .map(|value| value.as_str()),
        Some("text/plain")
    );
    assert_eq!(
        body.projection
            .transform_chain
            .iter()
            .map(|value| value.as_str())
            .collect::<Vec<_>>(),
        ["toon"]
    );
    assert_eq!(
        body.projection.reversibility,
        Some(ContextReversibility::Lossless)
    );
    assert_eq!(body.projection.invocation.provider_id.as_str(), "tokenless");
    assert_eq!(
        body.projection.invocation.disposition,
        ProviderDisposition::Produced
    );

    let stored_body = String::from_utf8(body_bytes).expect("canonical Ledger bytes are UTF-8");
    assert!(!stored_body.contains(source));
    assert!(!stored_body.contains(EXPECTED_REPLACEMENT));
    assert!(
        !stored_body.contains("\"content\""),
        "the Ledger stores digests and shape metadata, never result content: {stored_body}"
    );
}

fn assert_security_receipt(receipt: &ProviderReceipt, capability: &str, scanned_bytes: usize) {
    assert_eq!(receipt.provider_id.as_str(), "agent-sec-core");
    assert_eq!(receipt.capability.id.as_str(), capability);
    assert_eq!(receipt.disposition, ProviderDisposition::Produced);
    assert!(receipt.error.is_none());
    assert_tool_scope(receipt);
    assert_eq!(meter(receipt, "security.findings_total").value, 0);
    assert_eq!(
        meter(receipt, "security.scanned_bytes").value,
        scanned_bytes as u64
    );
    let encoded = serde_json::to_string(receipt).expect("receipt serializes");
    assert!(!encoded.contains("checkout-service"));
}

fn assert_tokenless_receipt(receipt: &ProviderReceipt) {
    assert_eq!(receipt.provider_id.as_str(), "tokenless");
    assert_eq!(receipt.capability.id.as_str(), "context.projection.prepare");
    assert_eq!(receipt.disposition, ProviderDisposition::Produced);
    assert!(receipt.error.is_none());
    assert_tool_scope(receipt);
    for (meter_id, expected) in [
        ("context.source_tokens", 174),
        ("context.prepared_tokens", 110),
    ] {
        let observed = meter(receipt, meter_id);
        assert_eq!(observed.value, expected);
        assert_eq!(
            observed.method.as_ref().map(|value| value.as_str()),
            Some("heuristic-v1")
        );
        assert_eq!(observed.measurement_kind, ProviderMeasurementKind::Estimate);
    }
    let encoded = serde_json::to_string(receipt).expect("receipt serializes");
    assert!(!encoded.contains(EXPECTED_REPLACEMENT));
}

fn assert_clean_observation(
    observation: &aw_contracts::ledger::LedgerObservation,
    scanned_bytes: usize,
    language: Option<SecurityDetectedLanguage>,
) {
    assert_eq!(observation.verdict, SecurityInspectionVerdict::Clean);
    assert!(observation.findings.is_empty());
    assert_eq!(observation.scanned_bytes, scanned_bytes as u64);
    assert!(!observation.truncated);
    assert_eq!(observation.language_detected, language);
    assert_eq!(
        observation.invocation.disposition,
        ProviderDisposition::Produced
    );
}

fn meter<'a>(receipt: &'a ProviderReceipt, meter_id: &str) -> &'a ProviderMeter {
    receipt
        .meters
        .iter()
        .find(|meter| meter.meter_id.as_str() == meter_id)
        .unwrap_or_else(|| panic!("receipt is missing `{meter_id}`"))
}

fn assert_tool_scope(receipt: &ProviderReceipt) {
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
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("adapter crate is nested below the repository root")
        .to_path_buf()
}
