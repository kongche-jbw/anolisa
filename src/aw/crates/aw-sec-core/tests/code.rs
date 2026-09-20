//! Native scanner verdicts and malformed output must never silently allow.
use aw_sec_core::code::CodeRequest;
use serde_json::{json, Value};
fn result(verdict: &str) -> Value {
    json!({"ok":true,"verdict":verdict,"language":"bash","engine_version":"0.12.0",
        "summary":"","elapsed_ms":0,"findings":[]})
}
#[test]
fn strict_policy_accepts_only_consistent_pass() {
    let request = CodeRequest::new("printf 'do not execute this'").unwrap();
    assert_eq!(request.args()[2], "printf 'do not execute this'");
    assert!(request
        .allows(0, &serde_json::to_vec(&result("pass")).unwrap())
        .unwrap());
    for severity in ["warn", "deny"] {
        let mut value = result(severity);
        value["findings"] =
            json!([{"rule_id":"rule","severity":severity,"desc_en":"","desc_zh":"","evidence":[]}]);
        assert!(!request
            .allows(0, &serde_json::to_vec(&value).unwrap())
            .unwrap());
        value["verdict"] = json!("pass");
        assert!(request
            .allows(0, &serde_json::to_vec(&value).unwrap())
            .is_err());
    }
}
#[test]
fn failed_unknown_duplicate_or_incomplete_results_are_errors() {
    let request = CodeRequest::new("echo ok").unwrap();
    for bytes in [b"{}".as_slice(), b"", b"{\"ok\":true,\"ok\":false}"] {
        assert!(request.allows(0, bytes).is_err());
    }
    for (field, value) in [
        ("ok", json!(false)),
        ("verdict", json!("error")),
        ("language", json!("python")),
        ("extra", json!(true)),
    ] {
        let mut output = result("pass");
        output[field] = value;
        assert!(request
            .allows(0, &serde_json::to_vec(&output).unwrap())
            .is_err());
    }
    assert!(request
        .allows(1, &serde_json::to_vec(&result("pass")).unwrap())
        .is_err());
    assert!(CodeRequest::new("").is_err());
    assert!(CodeRequest::new("x\0y").is_err());
}
