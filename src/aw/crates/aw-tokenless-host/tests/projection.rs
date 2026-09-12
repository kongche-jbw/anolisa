//! Real bounded subprocess exchange against a synthetic protocol peer, not Tokenless.

mod common;
use aw_contracts::{canonical, Registry};
use aw_core::ports::ProviderHost;
use aw_tokenless_host::TokenlessHost;
use common::{invocation, Fixture};
use serde_json::{json, Value};
use std::{fs, sync::atomic::Ordering};

const SOURCE: &str = "\u{1b}[32m done 中文 with extra padding \u{1b}[0m\n";

#[test]
fn candidate_binding_and_native_controls_are_exact() {
    let fixture = Fixture::new("applied");
    let mut host = TokenlessHost::new(fixture.config.clone()).unwrap();
    let invocation = invocation(&host, SOURCE);
    let result = host.invoke(&invocation).unwrap();
    Registry::new()
        .unwrap()
        .validate_result(&invocation, &result.receipt, result.output.as_ref())
        .unwrap();
    assert_eq!(result.receipt["disposition"], "produced");
    let candidate = &result.output.as_ref().unwrap()["candidate"];
    assert_eq!(
        candidate["source_digest"],
        invocation["input"]["artifact"]["digest"]
    );
    assert_eq!(candidate["reversibility"], "unrecoverable");
    assert_eq!(candidate["recovery"], json!({"mode":"none"}));
    assert_eq!(candidate["content"], "done\n");
    assert_eq!(result.receipt["meters"][0]["value"], SOURCE.len());
    assert_eq!(result.receipt["meters"][1]["value"], 5);
    for key in [
        "scope",
        "plan_ref",
        "input_digest",
        "invocation_id",
        "manifest_digest",
    ] {
        assert_eq!(result.receipt[key], invocation[key]);
    }
    assert_eq!(
        host.descriptor("test-tokenless").unwrap()["guarantee"],
        "declared"
    );
    let called: Value =
        serde_json::from_slice(&fs::read(fixture.path.join("called.json")).unwrap()).unwrap();
    assert_eq!(called["request"]["input"]["content"], SOURCE);
    assert_eq!(
        called["request"]["input"]["capabilities"]["recovery"],
        json!({"kind":"none"})
    );
    assert_eq!(called["request"]["input"]["output_optimization"], "none");
    assert_eq!(
        called["request"]["attribution"]["agent_id"],
        invocation["scope"]["actor_id"]
    );
    for name in ["HOME", "PATH", "HTTP_PROXY", "HTTPS_PROXY"] {
        assert!(called["environment"].get(name).is_none());
    }
    assert!(!result.receipt.to_string().contains("private"));
}

#[test]
fn preserved_or_no_savings_native_results_are_bypassed() {
    for mode in [
        "passthrough",
        "no_savings",
        "dry_run",
        "recoverability_unavailable",
    ] {
        let fixture = Fixture::new(mode);
        let mut host = TokenlessHost::new(fixture.config.clone()).unwrap();
        let result = host.invoke(&invocation(&host, SOURCE)).unwrap();
        assert_eq!(result.receipt["disposition"], "bypassed", "{mode}");
        assert!(result.output.is_none());
        assert_eq!(result.receipt["meters"], json!([]));
    }
}

#[test]
fn contradictory_native_claims_cannot_produce_candidates() {
    for mode in [
        "failure",
        "bad_preserve",
        "attribution",
        "retrievable",
        "unrecoverable",
        "stash",
        "operations",
        "no_reduction",
        "measurement",
        "unknown_tokenizer",
        "tool_error",
        "version",
        "operation",
        "duplicate",
        "unknown",
    ] {
        let fixture = Fixture::new(mode);
        let mut host = TokenlessHost::new(fixture.config.clone()).unwrap();
        let result = host.invoke(&invocation(&host, SOURCE)).unwrap();
        assert_eq!(result.receipt["disposition"], "failed", "{mode}");
        assert!(result.output.is_none(), "{mode}");
        assert!(!result.receipt.to_string().contains("private"));
    }
}

#[test]
fn native_operations_respect_the_request_profile() {
    for (operation, allow_text_reencoding, tool_name, permitted) in [
        ("schema_compression", true, "Bash", false),
        ("search_path_sharing", true, "Bash", false),
        ("build_log_reduction", true, "Bash", false),
        ("tabular_row_reduction", true, "Bash", false),
        ("json_record_reduction", true, "Bash", false),
        ("json_truncation", true, "Bash", false),
        ("toon", false, "Bash", false),
        ("tabular_compaction", false, "Bash", false),
        ("toon", true, "Bash", true),
        ("tabular_compaction", true, "Bash", true),
        ("terminal_cleanup", true, "Grep", false),
    ] {
        let fixture = Fixture::new(&format!("operation_{operation}"));
        let mut host = TokenlessHost::new(fixture.config.clone()).unwrap();
        let mut invocation = invocation(&host, SOURCE);
        invocation["input"]["artifact"]["tool_name"] = json!(tool_name);
        invocation["input"]["constraints"]["allow_text_reencoding"] = json!(allow_text_reencoding);
        invocation["input_digest"] =
            json!(canonical::document_digest(&invocation["input"]).unwrap());
        let result = host.invoke(&invocation).unwrap();
        assert_eq!(
            result.receipt["disposition"],
            if permitted { "produced" } else { "failed" },
            "{operation} allow_reencoding={allow_text_reencoding} tool={tool_name}: {}",
            result.receipt["error_code"]
        );
        assert_eq!(result.output.is_some(), permitted);
        if permitted {
            assert_eq!(
                result.output.as_ref().unwrap()["candidate"]["reversibility"],
                "unrecoverable"
            );
        } else {
            assert_eq!(result.receipt["error_code"], "invalid_native_candidate");
        }
    }
}

#[test]
fn invalid_admission_never_launches_compression() {
    let fixture = Fixture::new("applied");
    let mut host = TokenlessHost::new(fixture.config.clone()).unwrap();
    for (pointer, value) in [
        ("/provider_id", json!("other")),
        ("/manifest_digest", json!("0".repeat(64))),
        ("/input_digest", json!("0".repeat(64))),
        ("/deadline_at_ms", json!(1)),
        ("/budget/input_bytes", json!(1)),
        ("/input/boundary", json!("pre_tool")),
        ("/input/artifact/origin", json!("unspecified")),
        ("/input/artifact/media_type", json!("application/json")),
        (
            "/input/constraints/accepted_reversibility",
            json!(["lossless"]),
        ),
    ] {
        let mut invocation = invocation(&host, SOURCE);
        *invocation.pointer_mut(pointer).unwrap() = value;
        if pointer.starts_with("/input/") {
            invocation["input_digest"] =
                json!(canonical::document_digest(&invocation["input"]).unwrap());
        }
        if let Ok(result) = host.invoke(&invocation) {
            assert_eq!(result.receipt["disposition"], "failed", "{pointer}");
            assert!(result.output.is_none());
        }
        assert!(!fixture.called(), "{pointer}");
    }
}

#[test]
fn version_controls_and_pins_are_checked_before_launch() {
    let mut fixture = Fixture::new("wrong_version");
    assert!(TokenlessHost::new(fixture.config.clone()).is_err());
    assert!(!fixture.called());
    for key in [
        "TOKENLESS_STATS_ENABLED",
        "TOKENLESS_SLS_ENABLED",
        "TOKENLESS_COMPRESSION_ENABLED",
    ] {
        let mut config = fixture.config.clone();
        config.environment.remove(key);
        assert!(TokenlessHost::new(config).is_err());
    }
    fixture
        .config
        .environment
        .insert("MODE".into(), "applied".into());
    let mut host = TokenlessHost::new(fixture.config.clone()).unwrap();
    fs::write(fixture.path.join("native.py"), "modified before launch").unwrap();
    let result = host.invoke(&invocation(&host, SOURCE)).unwrap();
    assert_eq!(result.receipt["disposition"], "failed");
    assert!(!fixture.called());
}

#[test]
fn pin_drift_and_timeout_fail_without_output() {
    for mode in ["pin_drift", "timeout"] {
        let fixture = Fixture::new(mode);
        let mut host = TokenlessHost::new(fixture.config.clone()).unwrap();
        let mut invocation = invocation(&host, SOURCE);
        if mode == "timeout" {
            invocation["budget"]["wall_time_ms"] = json!(1000);
        }
        let result = host.invoke(&invocation).unwrap();
        assert_eq!(result.receipt["disposition"], "failed", "{mode}");
        assert!(result.output.is_none());
        assert!(fixture.called());
    }
}

#[test]
fn cancellation_prevents_native_launch() {
    use aw_core::ports::Cancellation;
    use std::sync::{atomic::AtomicBool, Arc};
    struct Cancel(AtomicBool);
    impl Cancellation for Cancel {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }
    let fixture = Fixture::new("applied");
    let cancel = Arc::new(Cancel(AtomicBool::new(false)));
    let mut host = TokenlessHost::new_cancellable(fixture.config.clone(), cancel.clone()).unwrap();
    cancel.0.store(true, Ordering::Relaxed);
    let result = host.invoke(&invocation(&host, SOURCE)).unwrap();
    assert_eq!(result.receipt["disposition"], "failed");
    assert_eq!(result.receipt["error_code"], "provider_cancelled");
    assert!(!fixture.called());
}

#[test]
fn frozen_real_tokenless_responses_map_without_a_runtime_dependency() {
    use aw_tokenless_host::{FilePin, PinState};
    let golden: Value = serde_json::from_slice(include_bytes!(
        "../../../tests/fixtures/tokenless-native.json"
    ))
    .unwrap();
    let cases = golden["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let mut fixture = Fixture::new("golden");
        let response = serde_json::to_vec(&case["response"]).unwrap();
        let response_path = fixture.path.join("response.json");
        fs::write(&response_path, &response).unwrap();
        fixture.config.pins.push(FilePin {
            path: response_path,
            state: PinState::Sha256(canonical::digest(&response)),
        });
        let native = std::env::var_os("AW_TOKENLESS_NATIVE_BINARY");
        if let Some(program) = &native {
            fixture.config.program = std::path::PathBuf::from(program);
            assert!(
                fixture.config.program.is_absolute(),
                "native binary must be absolute"
            );
            fixture.config.program_sha256 =
                canonical::digest(&fs::read(&fixture.config.program).unwrap());
            // Debug native binaries require full executable pin checks before
            // and after invocation; the synthetic peer's 2s budget is too small.
            fixture.config.limits.timeout_ms = 15_000;
            fixture.config.args.clear();
            fixture.config.pins.clear();
            fixture
                .config
                .environment
                .retain(|key, _| key.starts_with("TOKENLESS_"));
        }
        let mut host = TokenlessHost::new(fixture.config.clone()).unwrap();
        let request = &case["request"];
        let mut invocation = invocation(&host, request["input"]["content"].as_str().unwrap());
        for (aw, native) in [
            ("actor_id", "agent_id"),
            ("session_id", "session_id"),
            ("tool_use_id", "tool_use_id"),
        ] {
            invocation["scope"][aw] = request["attribution"][native].clone();
        }
        invocation["input"]["constraints"]["allow_text_reencoding"] =
            request["input"]["capabilities"]["replace_with_text"].clone();
        invocation["input_digest"] =
            json!(canonical::document_digest(&invocation["input"]).unwrap());
        if native.is_some() {
            invocation["budget"]["wall_time_ms"] = json!(15_000);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            invocation["deadline_at_ms"] = json!(now + 15_000);
        }
        let result = host.invoke(&invocation).unwrap();
        let applied = case["response"]["result"]["disposition"] == "applied";
        assert_eq!(
            result.receipt["disposition"],
            if applied { "produced" } else { "bypassed" },
            "{}: error={} started={} completed={}",
            case["name"],
            result.receipt["error_code"],
            result.receipt["started_at_ms"],
            result.receipt["completed_at_ms"]
        );
        if applied {
            assert_eq!(
                result.output.as_ref().unwrap()["candidate"]["content"],
                case["response"]["result"]["output"]
            );
            assert_eq!(
                result.output.as_ref().unwrap()["candidate"]["reversibility"],
                "unrecoverable"
            );
        } else {
            assert!(result.output.is_none());
        }
        if native.is_none() {
            let called: Value =
                serde_json::from_slice(&fs::read(fixture.path.join("called.json")).unwrap())
                    .unwrap();
            assert_eq!(&called["request"], request);
        }
        Registry::new()
            .unwrap()
            .validate_result(&invocation, &result.receipt, result.output.as_ref())
            .unwrap();
    }
}
