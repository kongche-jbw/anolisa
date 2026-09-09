//! Direct hook fixtures validate serial Core orchestration, not a real Agent.

use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static SERIAL: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tokenless-hook-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn execute_with_consent(
    host: &str,
    sec_failed: bool,
    disposition: &str,
    consent: Option<bool>,
) -> (std::process::Output, Vec<Value>) {
    let temp = Fixture::new();
    let fixture: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/contracts.json")).unwrap();
    let pid = std::process::id();
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let started: u64 = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    let sec = if sec_failed {
        "import sys;sys.exit(7)".to_owned()
    } else {
        "import sys,json\nr=json.load(sys.stdin)\nprint(json.dumps({'protocol_version':1,'operation':'content_inspect','disposition':'completed','findings_total':0,'scanned_bytes':len(r['content'].encode()),'truncated':False,'verdict':'clean','findings':[],'engine':'pii-regex'}))".into()
    };
    let tokenless=format!("import sys,json\nr=json.load(sys.stdin)\nx={{'protocol_version':2,'operation':'post_tool','attribution':r['attribution'],'result':{{'output':'short','disposition':{disposition:?},'applied_operations':['json_cleanup'],'recoverability':'lossless','before_tokens':5,'after_tokens':2,'stash_keys':[],'tokenizer_id':'fixture'}}}}\nif x['result']['disposition']!='applied': x['result']['applied_operations']=[]\nprint(json.dumps(x))");
    let provider = |id: &str, script: &str| json!({"provider_id":id,"provider_version":"fixture-1","program":"/usr/bin/python3","args":["-c",script],"environment":BTreeMap::<String,String>::new()});
    let mut settings = json!({"runtime":fixture["runtime-binding-v1"],"scope":fixture["capability-plan-v1"]["scope"],
        "agent_pid":pid,"agent_start_ticks":started,"qoder_single_turn_id":"turn-1",
        "journal":temp.0.join("journal"),"evidence":temp.0.join("evidence"),
        "provider":provider("sec-fixture",&sec),"tokenless":provider("tokenless-fixture",&tokenless)});
    if let Some(consent) = consent {
        settings["tokenless"]["allow_unrecoverable"] = json!(consent);
    }
    let path = temp.0.join("settings.json");
    fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let payload = json!({"hook_event_name":"PostToolUse","session_id":"session-1","tool_use_id":"tool-1","turn_id":"turn-1",
        "tool_name":"Bash","tool_input":{"command":"printf fixture"},"tool_response":"alpha alpha alpha\n"});
    let mut child = Command::new("timeout")
        .arg("20")
        .arg(env!("CARGO_BIN_EXE_aw-hook-cli"))
        .arg(host)
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&payload).unwrap())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    let evidence = fs::read_dir(temp.0.join("evidence"))
        .map(|dir| {
            dir.map(|entry| {
                serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap()
            })
            .collect()
        })
        .unwrap_or_default();
    (result, evidence)
}
fn execute(host: &str, sec_failed: bool, disposition: &str) -> (std::process::Output, Vec<Value>) {
    execute_with_consent(host, sec_failed, disposition, Some(true))
}
#[test]
fn omitted_or_false_information_loss_consent_rejects_before_invocation() {
    for consent in [None, Some(false)] {
        let (result, evidence) = execute_with_consent("qoder", false, "applied", consent);
        assert!(!result.status.success());
        assert!(evidence.is_empty());
    }
}

#[test]
fn qoder_serial_inspection_then_projection_returns_candidate_without_adoption() {
    let (result, evidence) = execute("qoder", false, "applied");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(output["hookSpecificOutput"]["updatedToolOutput"], "short");
    assert_eq!(evidence.len(), 1);
    let evidence = &evidence[0];
    assert_eq!(evidence["calls"].as_array().unwrap().len(), 2);
    assert_eq!(
        evidence["calls"][0]["receipt"]["capability"],
        "security.content.inspect/v2"
    );
    assert_eq!(
        evidence["calls"][1]["receipt"]["capability"],
        "context.projection.prepare/v2"
    );
    assert_eq!(evidence["adoption"], "not_observed");
    assert_eq!(evidence["delivery"], "prepared_for_return");
    assert_eq!(evidence["candidate_bytes"], 5);
    assert!(evidence["calls"][1]["invocation"]["input"]["artifact"]
        .get("content")
        .is_none());
    assert_eq!(evidence["tokenless_mapping"]["native_claim"], "lossless");
}
#[test]
fn required_inspection_failure_skips_tokenless() {
    let (result, evidence) = execute("qoder", true, "applied");
    assert!(result.status.success());
    let output: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(output.get("hookSpecificOutput").is_none());
    assert_eq!(evidence[0]["calls"].as_array().unwrap().len(), 1);
    assert_eq!(evidence[0]["delivery"], "original_retained");
}
#[test]
fn no_savings_retains_original_and_zero_candidates() {
    let (result, evidence) = execute("qoder", false, "no_savings");
    assert!(result.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        json!({})
    );
    assert_eq!(
        evidence[0]["calls"][1]["receipt"]["disposition"],
        "bypassed"
    );
    assert!(evidence[0]["candidate_bytes"].is_null());
}
#[test]
fn codex_rejects_projection_before_provider_invocation() {
    let (result, evidence) = execute("codex", false, "applied");
    assert!(!result.status.success());
    assert!(evidence.is_empty());
}
