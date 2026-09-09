//! Synthetic Python protocol peers exercise the real subprocess transport.
//! These fixtures are not the SecCore scanner or native Agent acceptance tests.

use aw_contracts::{canonical, Registry};
use aw_core::ports::ProviderHost;
use aw_sec_host::{Config, Limits, SecHost};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn host(script: &str) -> SecHost {
    host_with_timeout(script, 5000)
}

fn host_with_timeout(script: &str, timeout_ms: u64) -> SecHost {
    SecHost::new(Config {
        provider_id: "sec-fixture".into(),
        provider_version: "fixture-1".into(),
        program: PathBuf::from("/usr/bin/python3"),
        args: vec!["-c".into(), script.into()],
        environment: BTreeMap::new(),
        limits: Limits {
            timeout_ms,
            input_bytes: 8192,
            output_bytes: 8192,
            stderr_bytes: 1024,
        },
    })
    .unwrap()
}

fn invocation(host: &SecHost) -> Value {
    let registry = Registry::new().unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/contracts.json")).unwrap();
    let mut call = fixture["capability-invocation-v1"].clone();
    let descriptor = host.descriptor("sec-fixture").unwrap();
    for field in ["provider_id", "provider_version", "manifest_digest"] {
        call[field] = descriptor[field].clone();
    }
    let cap = &descriptor["capabilities"][0];
    for field in ["capability", "input_schema", "output_schema"] {
        call[field] = cap[field].clone();
    }
    call["input"] = fixture["security-content-inspect-input-v2"].clone();
    call["input_digest"] = json!(canonical::document_digest(&call["input"]).unwrap());
    call["deadline_at_ms"] = json!(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 10000
    );
    call["budget"]["wall_time_ms"] = json!(5000);
    registry
        .validate("capability-invocation-v1", &call)
        .unwrap();
    call
}

const PREFIX: &str = "import sys,json\nr=json.load(sys.stdin)\nassert r == {'protocol_version':1,'operation':'content_inspect','content':'alpha alpha alpha\\n','source':'tool_output','include_low_confidence':False}\nn=len(r['content'].encode('utf-8'))\nx={'protocol_version':1,'operation':'content_inspect','disposition':'completed','findings_total':0,'scanned_bytes':n,'truncated':False,'verdict':'clean','findings':[],'engine':'pii-regex'}\n";

fn invoke_script(suffix: &str) -> aw_core::ports::ProviderResult {
    let mut host = host(&format!("{PREFIX}{suffix}"));
    let call = invocation(&host);
    host.invoke(&call).unwrap()
}

#[test]
fn binds_real_process_receipt_and_full_declared_coverage() {
    let mut host = host(&format!("{PREFIX}print(json.dumps(x))"));
    let call = invocation(&host);
    let result = host.invoke(&call).unwrap();
    assert_eq!(
        result.receipt["disposition"], "produced",
        "{}",
        result.receipt
    );
    assert_eq!(result.receipt["input_digest"], call["input_digest"]);
    assert_eq!(result.receipt["scope"], call["scope"]);
    assert_eq!(result.receipt["plan_ref"], call["plan_ref"]);
    assert!(
        result.receipt["completed_at_ms"].as_u64().unwrap()
            >= result.receipt["started_at_ms"].as_u64().unwrap()
    );
    let output = result.output.unwrap();
    assert_eq!(output["inspection"]["coverage"]["scanned_bytes"], 18);
    assert_eq!(output["inspection"]["coverage"]["complete"], true);
    assert_eq!(
        result.receipt["output"]["digest"],
        canonical::document_digest(&output).unwrap()
    );
    assert_eq!(
        result.receipt["output"]["bytes"],
        canonical::bytes(&output).unwrap().len()
    );
    assert_eq!(result.receipt["evidence"], json!([]));
}

#[test]
fn preserves_partial_scanner_coverage_and_findings() {
    let result = invoke_script("x.update(scanned_bytes=9,truncated=True,verdict='sensitive',findings_total=1,findings=[{'rule_id':'test.secret','category':'secret','severity':'high','confidence':'high','count':1}])\nprint(json.dumps(x))");
    assert_eq!(
        result.receipt["disposition"], "produced",
        "{}",
        result.receipt
    );
    let output = result.output.unwrap();
    assert_eq!(output["inspection"]["coverage"]["scanned_bytes"], 9);
    assert_eq!(output["inspection"]["coverage"]["complete"], false);
    assert_eq!(output["inspection"]["findings"][0]["count"], 1);
}

#[test]
fn rejects_inconsistent_coverage_without_output() {
    for update in [
        "scanned_bytes=n+1",
        "scanned_bytes=n-1",
        "truncated=True",
        "scanned_bytes=True",
    ] {
        let result = invoke_script(&format!("x.update({update})\nprint(json.dumps(x))"));
        assert_eq!(result.receipt["disposition"], "failed", "{update}");
        assert_eq!(result.receipt["error_code"], "invalid_native_coverage");
        assert!(result.output.is_none());
    }
}

#[test]
fn rejects_unknown_keys_duplicate_json_and_wrong_finding_totals() {
    for suffix in [
        "x['matched_content']='must not escape'\nprint(json.dumps(x))",
        "x['findings_total']=1\nprint(json.dumps(x))",
        "print('{\"protocol_version\":1,\"protocol_version\":1}')",
    ] {
        let result = invoke_script(suffix);
        assert_eq!(result.receipt["disposition"], "failed");
        assert!(result.output.is_none());
        assert!(!result.receipt.to_string().contains("must not escape"));
    }
}

#[test]
fn scanner_error_and_skip_are_not_success() {
    for (terminal, field, value, expected) in [
        ("error", "error_code", "scanner_failed", "failed"),
        ("skipped", "skip_reason", "not_applicable", "bypassed"),
    ] {
        let result = invoke_script(&format!("print(json.dumps({{'protocol_version':1,'operation':'content_inspect','disposition':'{terminal}','findings_total':0,'scanned_bytes':0,'{field}':'{value}'}}))"));
        assert_eq!(result.receipt["disposition"], expected);
        assert!(result.output.is_none());
    }
}

#[test]
fn timeout_is_bounded_and_does_not_claim_success() {
    let mut host = host_with_timeout("import time;time.sleep(30)", 1500);
    let call = invocation(&host);
    let start = Instant::now();
    let result = host.invoke(&call).unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(result.receipt["error_code"], "provider_timeout");
    assert!(result.output.is_none());
}

#[test]
fn output_and_stderr_floods_are_bounded_and_private() {
    for script in [
        "import sys;sys.stdin.read();sys.stdout.write('x'*20000)",
        "import sys;sys.stdin.read();sys.stderr.write('private'*2000)",
    ] {
        let mut host = host(script);
        let call = invocation(&host);
        let result = host.invoke(&call).unwrap();
        assert_eq!(result.receipt["error_code"], "provider_output_limit");
        assert!(!result.receipt.to_string().contains("private"));
    }
}

#[test]
fn bad_json_and_exit_status_have_failed_receipts() {
    for (script, code) in [
        (
            "import sys;sys.stdin.read();print('no-json')",
            "invalid_native_json",
        ),
        (
            "import sys;sys.stdin.read();sys.exit(5)",
            "provider_exit_failed",
        ),
    ] {
        let mut host = host(script);
        let call = invocation(&host);
        assert_eq!(host.invoke(&call).unwrap().receipt["error_code"], code);
    }
}

#[test]
fn rejects_unpinned_invocation_before_process_launch() {
    let mut host = host("raise RuntimeError('must not start')");
    let mut call = invocation(&host);
    call["provider_version"] = json!("another-version");
    assert_eq!(
        host.invoke(&call).unwrap_err().code,
        "provider_binding_mismatch"
    );
}

#[test]
fn input_digest_and_output_budget_fail_visibly() {
    let mut host = host(&format!("{PREFIX}print(json.dumps(x))"));
    let mut call = invocation(&host);
    call["input"]["artifact"]["content"] = json!("changed");
    assert_eq!(
        host.invoke(&call).unwrap().receipt["error_code"],
        "input_digest_mismatch"
    );
    let mut call = invocation(&host);
    call["budget"]["output_bytes"] = json!(1);
    assert_eq!(
        host.invoke(&call).unwrap().receipt["error_code"],
        "output_budget_exceeded"
    );
}

#[test]
fn configuration_changes_are_pinned_and_environment_is_explicit() {
    let one = host("import sys;sys.stdin.read()");
    let two = host("import sys;sys.stdin.read();print('different')");
    assert_ne!(
        one.descriptor("sec-fixture").unwrap()["manifest_digest"],
        two.descriptor("sec-fixture").unwrap()["manifest_digest"]
    );
    let result = invoke_script("import os\nassert 'HOME' not in os.environ\nprint(json.dumps(x))");
    assert_eq!(
        result.receipt["disposition"], "produced",
        "{}",
        result.receipt
    );
}

#[test]
fn coverage_counts_utf8_bytes_and_accepts_empty_complete_input() {
    for content in ["你好\n", ""] {
        let mut host = host("import sys,json\nr=json.load(sys.stdin)\nprint(json.dumps({'protocol_version':1,'operation':'content_inspect','disposition':'completed','findings_total':0,'scanned_bytes':len(r['content'].encode('utf-8')),'truncated':False,'verdict':'clean','findings':[],'engine':'pii-regex'}))");
        let mut call = invocation(&host);
        call["input"]["artifact"]["content"] = json!(content);
        call["input"]["artifact"]["digest"] = json!(canonical::digest(content.as_bytes()));
        call["input_digest"] = json!(canonical::document_digest(&call["input"]).unwrap());
        let result = host.invoke(&call).unwrap();
        assert_eq!(
            result.receipt["disposition"], "produced",
            "{}",
            result.receipt
        );
        assert_eq!(
            result.output.unwrap()["inspection"]["coverage"]["scanned_bytes"],
            content.len()
        );
    }
}

#[test]
fn exhausted_input_budget_does_not_start_provider() {
    let mut host = host("raise RuntimeError('must not start')");
    let mut call = invocation(&host);
    call["budget"]["input_bytes"] = json!(1);
    let result = host.invoke(&call).unwrap();
    assert_eq!(result.receipt["error_code"], "input_budget_exceeded");
    assert!(result.output.is_none());
}

#[test]
fn inherited_output_pipe_does_not_escape_timeout() {
    let mut host = host_with_timeout("import os,sys,time\nsys.stdin.read()\nif os.fork() == 0:\n time.sleep(30)\nelse:\n os._exit(0)", 1500);
    let call = invocation(&host);
    let start = Instant::now();
    let result = host.invoke(&call).unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(result.receipt["error_code"], "provider_timeout");
    assert!(result.output.is_none());
}
