//! Protocol conformance with synthetic and frozen native responses; no live scanner.

use aw_contracts::{canonical, Registry};
use aw_sec_core::{Error, PiiRequest, MAX_OUTPUT_BYTES, PROTOCOL_PROFILE};
use serde_json::{json, Value};
use std::sync::LazyLock;

static REGISTRY: LazyLock<Registry> = LazyLock::new(|| Registry::new().unwrap());

#[test]
fn frozen_native_cli_cases_project_without_running_a_scanner() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/native-pii.json")).unwrap();
    let names: Vec<_> = fixtures["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "clean",
            "unicode",
            "credential",
            "low_hidden",
            "low_included",
            "custom",
            "invalid_rules"
        ]
    );
    for case in fixtures["cases"].as_array().unwrap() {
        let request = PiiRequest::new(
            &REGISTRY,
            &input(
                case["content"].as_str().unwrap(),
                case["include_low_confidence"].as_bool().unwrap(),
            ),
        )
        .unwrap();
        let result = request.project(
            case["exit_code"].as_i64().unwrap() as i32,
            &serde_json::to_vec(&case["output"]).unwrap(),
        );
        if case["expected_aw_verdict"].is_null() {
            assert!(result.is_err(), "{}", case["name"]);
        } else {
            let output = result.unwrap_or_else(|error| panic!("{}: {error}", case["name"]));
            assert_eq!(
                output["inspection"]["verdict"], case["expected_aw_verdict"],
                "{}",
                case["name"]
            );
            REGISTRY
                .validate("security-content-inspect-output-v2", &output)
                .unwrap();
        }
    }
}

fn input(text: &str, low: bool) -> Value {
    json!({"artifact":{"id":"tool-result","content":text,"digest":canonical::digest(text.as_bytes()),
        "media_type":"text/plain","origin":"command_output"},"boundary":"post_tool",
        "constraints":{"include_low_confidence":low}})
}

fn response(text: &str, findings: Vec<Value>) -> Value {
    let verdict = if findings.iter().any(|f| f["severity"] == "deny") {
        "deny"
    } else if findings.is_empty() {
        "pass"
    } else {
        "warn"
    };
    let mut by_type = serde_json::Map::new();
    let mut by_category = serde_json::Map::new();
    let mut by_severity = serde_json::Map::new();
    for finding in &findings {
        for (field, counts) in [
            ("type", &mut by_type),
            ("category", &mut by_category),
            ("severity", &mut by_severity),
        ] {
            let count = counts
                .entry(finding[field].as_str().unwrap())
                .or_insert(json!(0));
            *count = json!(count.as_u64().unwrap() + 1);
        }
    }
    json!({"ok":true,"verdict":verdict,"findings":findings,"elapsed_ms":1,"summary":{
        "total":findings.len(),"by_type":by_type,"by_category":by_category,"by_severity":by_severity,
        "source":"tool_output","bytes_scanned":text.len(),"truncated":false,"custom_rules":{
            "status":"absent","rule_count":0,"runtime_error_count":0,"budget_exhausted":false,"truncated":false
        }
    }})
}

fn finding(kind: &str, category: &str, severity: &str, confidence: f64) -> Value {
    json!({"type":kind,"category":category,"severity":severity,"confidence":confidence,
        "evidence_redacted":"secret evidence must never appear in AW output","span":{"start":0,"end":1},
        "metadata":{"sensitive":"private detector metadata"}})
}

fn project(request: &PiiRequest<'_>, native: &Value) -> Result<Value, Error> {
    request.project(0, &serde_json::to_vec(native).unwrap())
}

#[test]
fn request_preserves_unicode_bytes_and_selects_only_supported_native_flags() {
    let text = "中文\n exact whitespace \t";
    for low in [false, true] {
        let request = PiiRequest::new(&REGISTRY, &input(text, low)).unwrap();
        assert_eq!(request.stdin(), text.as_bytes());
        let mut expected = vec![
            "scan-pii",
            "--stdin",
            "--format",
            "json",
            "--source",
            "tool_output",
        ];
        if low {
            expected.push("--include-low-confidence");
        }
        assert_eq!(request.args(), expected);
        let output = project(&request, &response(text, vec![])).unwrap();
        assert_eq!(output["inspection"]["verdict"], "clean");
        assert_eq!(output["inspection"]["coverage"]["input_bytes"], text.len());
        assert_eq!(
            output["inspection"]["coverage"]["input_digest"],
            canonical::digest(text.as_bytes())
        );
    }
    for (pointer, value) in [
        ("/boundary", json!("pre_tool")),
        ("/artifact/digest", json!("0".repeat(64))),
        ("/artifact/media_type", json!("application/json")),
        ("/constraints/include_low_confidence", json!("false")),
    ] {
        let mut invalid = input(text, false);
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(matches!(
            PiiRequest::new(&REGISTRY, &invalid),
            Err(Error::InvalidInput)
        ));
    }
    let empty = PiiRequest::new(&REGISTRY, &input("", false)).unwrap();
    assert!(project(&empty, &response("", vec![])).is_ok());
}

#[test]
fn warn_deny_and_custom_findings_aggregate_without_evidence_or_metadata() {
    let request = PiiRequest::new(&REGISTRY, &input("safe input", true)).unwrap();
    for (severity, verdict, projected_severity) in [
        ("warn", "suspicious", "medium"),
        ("deny", "sensitive", "high"),
    ] {
        let native = response(
            "safe input",
            vec![finding("email", "personal_data", severity, 0.7); 2],
        );
        let output = project(&request, &native).unwrap();
        assert_eq!(output["inspection"]["verdict"], verdict);
        assert_eq!(
            output["inspection"]["findings"],
            json!([{"rule_id":"email","category":"personal_data",
            "severity":projected_severity,"confidence":"medium","count":2}])
        );
        let encoded = serde_json::to_string(&output).unwrap();
        for excluded in [
            "secret evidence",
            "private detector",
            "evidence_redacted",
            "metadata",
            "safe input",
        ] {
            assert!(!encoded.contains(excluded));
        }
    }
    let mut native = response(
        "safe input",
        vec![finding("company_marker", "custom", "deny", 1.0)],
    );
    native["summary"]["custom_rules"]["status"] = json!("loaded");
    native["summary"]["custom_rules"]["rule_count"] = json!(1);
    native["summary"]["custom_rules"]["ruleset_sha256"] = json!("a".repeat(64));
    let output = project(&request, &native).unwrap();
    assert_eq!(
        output["inspection"]["findings"][0]["rule_id"],
        "company_marker"
    );
    assert_eq!(output["inspection"]["findings"][0]["category"], "other");
    assert_eq!(
        output["inspection"]["coverage"]["ruleset_ids"],
        json!([
            PROTOCOL_PROFILE,
            format!("agent-sec.custom/sha256:{}", "a".repeat(64))
        ])
    );
}

#[test]
fn incomplete_or_invalid_custom_rules_never_become_clean() {
    let request = PiiRequest::new(&REGISTRY, &input("abc", false)).unwrap();
    for (field, value) in [
        ("status", json!("invalid")),
        ("runtime_error_count", json!(1)),
        ("budget_exhausted", json!(true)),
        ("truncated", json!(true)),
        ("error_code", json!("invalid_regex")),
        ("status", json!("unknown")),
        ("rule_count", json!(1)),
    ] {
        let mut native = response("abc", vec![]);
        native["summary"]["custom_rules"][field] = value;
        assert!(project(&request, &native).is_err(), "{field}");
    }
    let mut native = response("abc", vec![]);
    native["summary"]
        .as_object_mut()
        .unwrap()
        .remove("custom_rules");
    assert!(project(&request, &native).is_err());
}

#[test]
fn malformed_findings_and_contradictory_statistics_are_rejected() {
    let request = PiiRequest::new(&REGISTRY, &input("abc", false)).unwrap();
    let native = response("abc", vec![finding("email", "personal_data", "warn", 0.82)]);
    for (pointer, value) in [
        ("/summary/total", json!(0)),
        ("/summary/by_type/email", json!(2)),
        ("/summary/by_category/personal_data", json!(0)),
        ("/summary/by_severity/warn", json!(0)),
        ("/summary/bytes_scanned", json!(2)),
        ("/summary/truncated", json!(true)),
        ("/summary/source", json!("manual")),
        ("/verdict", json!("pass")),
        ("/ok", json!(false)),
        ("/findings/0/category", json!("unknown")),
        ("/findings/0/severity", json!("critical")),
        ("/findings/0/type", json!("email@bad")),
        ("/findings/0/confidence", json!(1.1)),
        ("/findings/0/confidence", json!(0.3)),
        ("/findings/0/span/end", json!(4)),
        ("/findings/0/span/start", json!(1)),
    ] {
        let mut invalid = native.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(project(&request, &invalid).is_err(), "{pointer}");
    }
    let included = PiiRequest::new(&REGISTRY, &input("abc", true)).unwrap();
    let low = response("abc", vec![finding("email", "personal_data", "warn", 0.3)]);
    assert_eq!(
        project(&included, &low).unwrap()["inspection"]["findings"][0]["confidence"],
        "low"
    );
}

#[test]
fn bounded_strict_decode_rejects_failures_duplicates_and_excess_disclosure() {
    let request = PiiRequest::new(&REGISTRY, &input("abc", false)).unwrap();
    let native = response("abc", vec![]);
    assert_eq!(
        request.project(1, b"private scanner exception"),
        Err(Error::NativeFailure)
    );
    assert_eq!(
        request.project(0, &vec![b' '; MAX_OUTPUT_BYTES + 1]),
        Err(Error::OutputLimit)
    );
    let json = serde_json::to_string(&native).unwrap();
    let duplicate = json.replacen("\"ok\":true", "\"ok\":false,\"ok\":true", 1);
    assert_eq!(
        request.project(0, duplicate.as_bytes()),
        Err(Error::InvalidResponse)
    );
    let populated = response("abc", vec![finding("email", "personal_data", "warn", 0.8)]);
    let counters = serde_json::to_string(&populated).unwrap();
    let duplicate_counter = counters.replacen(
        "\"by_type\":{\"email\":1}",
        "\"by_type\":{\"email\":0,\"email\":1}",
        1,
    );
    assert_ne!(duplicate_counter, counters);
    assert_eq!(
        request.project(0, duplicate_counter.as_bytes()),
        Err(Error::InvalidResponse)
    );
    let mut disclosed = native.clone();
    disclosed["redacted_text"] = json!("unexpected text");
    assert!(project(&request, &disclosed).is_err());
    let mut disclosed = response("abc", vec![finding("email", "personal_data", "warn", 0.8)]);
    disclosed["findings"][0]["raw_evidence"] = json!("sensitive");
    assert!(project(&request, &disclosed).is_err());
    let encoded = serde_json::to_string(&disclosed).unwrap();
    let error = request.project(0, encoded.as_bytes()).unwrap_err();
    assert!(!format!("{error:?} {error}").contains("sensitive"));
}

#[test]
fn too_many_distinct_findings_cannot_be_silently_dropped() {
    let request = PiiRequest::new(&REGISTRY, &input("abc", false)).unwrap();
    let findings = (0..65)
        .map(|index| finding(&format!("custom_{index}"), "custom", "deny", 1.0))
        .collect();
    let mut native = response("abc", findings);
    native["summary"]["custom_rules"]["status"] = json!("loaded");
    native["summary"]["custom_rules"]["rule_count"] = json!(65);
    native["summary"]["custom_rules"]["ruleset_sha256"] = json!("a".repeat(64));
    assert_eq!(project(&request, &native), Err(Error::InvalidResponse));
}
