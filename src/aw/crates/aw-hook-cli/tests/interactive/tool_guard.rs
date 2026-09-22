//! Guard failures must produce native denial, never a clean optional notification.

use super::lifecycle::{lifecycle_config, native};
use super::*;

pub(super) const SCANNER: &str = r#"import json,sys,pathlib
if sys.argv[-1]=='--version':
 print('agent-sec-cli 0.12.0');sys.exit(0)
assert sys.argv[1:3]==['scan-code','--code']
assert sys.argv[4:]==['--language','bash','--mode','regex']
command=sys.argv[3]
pathlib.Path('scanned').write_text(command)
findings=[] if 'forbidden' not in command else [{'rule_id':'fixture','severity':'deny','desc_zh':'','desc_en':'','evidence':[]}]
print(json.dumps({'ok':True,'verdict':'deny' if findings else 'pass','summary':'','findings':findings,'language':'bash','engine_version':'0.12.0','elapsed_ms':0}))
"#;

pub(super) fn fixture_guard(transform: Option<&str>, scanner: &str) -> Directory {
    fixture_guard_budget(transform, scanner, 500)
}

fn fixture_guard_budget(transform: Option<&str>, scanner: &str, scanner_ms: u64) -> Directory {
    lifecycle_config("print('{\"format\":1,\"observed\":true}')", |config| {
        let mut scan = config["notifications"]["tool.before"][0].clone();
        scan["provider_id"] = json!("code-scanner");
        let script = |name: &str, code: &str| {
            let path = Path::new(scan["cwd"].as_str().unwrap()).join(name);
            let content = format!("#!/usr/bin/python3\n{code}");
            fs::write(&path, &content).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            (path, canonical::digest(content.as_bytes()))
        };
        let (scanner_path, scanner_digest) = script("scanner.py", scanner);
        let transform_program = transform.map(|code| script("transform.py", code));
        scan["program"] = json!(scanner_path);
        scan["program_sha256"] = json!(scanner_digest);
        scan["args"] = json!([]);
        config["notifications"] = json!({});
        let transforms: Vec<Value> = transform_program
            .into_iter()
            .map(|(path, digest)| {
                let mut step = scan.clone();
                step["provider_id"] = json!("transform");
                step["program"] = json!(path);
                step["program_sha256"] = json!(digest);
                step
            })
            .collect();
        scan["limits"]["timeout_ms"] = json!(scanner_ms);
        config["tool_guard"] = json!({"transforms":transforms,"scanner":scan});
    })
}
fn start(dir: &Directory) {
    callback(&dir.0, native(dir, "SessionStart", "s1", "t1")).unwrap();
}
fn deny(value: &Value) {
    assert_eq!(value["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(value["hookSpecificOutput"].get("updatedInput").is_none());
}

#[test]
fn dedicated_guard_entry_rejects_missing_configuration_and_wrong_events() {
    use aw_hook_cli::interactive::guard_callback_with_cancellation as guard;
    let dir = fixture_guard(None, SCANNER);
    assert!(guard(&dir.0, json!({}), &|| false).is_err());
    assert!(guard(&dir.0, native(&dir, "SessionStart", "s1", "t1"), &|| false).is_err());
    start(&dir);
    let response = guard(&dir.0, native(&dir, "PreToolUse", "s1", "t1"), &|| false).unwrap();
    assert_eq!(response, json!({}));
    let path = dir.0.join("binding.json");
    let mut binding: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    binding["prepared"]["config"]
        .as_object_mut()
        .unwrap()
        .remove("tool_guard");
    write(&path, &binding);
    assert!(guard(&dir.0, native(&dir, "PreToolUse", "s1", "t2"), &|| false).is_err());
    fs::remove_file(path).unwrap();
    assert!(guard(&dir.0, native(&dir, "PreToolUse", "s1", "t3"), &|| false).is_err());
}

#[test]
fn transformed_command_is_checked_and_original_fields_are_preserved() {
    let transform="import json,sys\ne=json.load(sys.stdin)\nassert e['candidate']['tool_input']['command']=='echo private'\nprint(json.dumps({'format':1,'command':'printf checked'}))";
    let dir = fixture_guard(Some(transform), SCANNER);
    start(&dir);
    let mut before = native(&dir, "PreToolUse", "s1", "t1");
    before["tool_input"]["description"] = json!("keep this");
    let response = callback(&dir.0, before.clone()).unwrap();
    assert_eq!(
        response["hookSpecificOutput"]["updatedInput"],
        json!({"command":"printf checked","description":"keep this"})
    );
    assert!(response["hookSpecificOutput"]
        .get("permissionDecision")
        .is_none());
    assert_eq!(
        fs::read_to_string(dir.0.join("scanned")).unwrap(),
        "printf checked"
    );
    fs::remove_file(dir.0.join("scanned")).unwrap();
    deny(&callback(&dir.0, before).unwrap());
    assert!(!dir.0.join("scanned").exists());
    assert_eq!(query(&dir.0).unwrap()["required_safety"], "unsupported");
    assert_eq!(
        query(&dir.0).unwrap()["effect"],
        "experimental_native_bash_guard"
    );
}

#[test]
fn unchanged_candidates_are_scanned_without_overriding_native_approval() {
    let identity = "import json,sys\ne=json.load(sys.stdin)\nprint(json.dumps({'format':1,'command':e['candidate']['tool_input']['command']}))";
    for transform in [None, Some(identity)] {
        let dir = fixture_guard(transform, SCANNER);
        start(&dir);
        let mut before = native(&dir, "PreToolUse", "s1", "unchanged");
        before["tool_input"]["description"] = json!("preserve native metadata");
        assert_eq!(callback(&dir.0, before).unwrap(), json!({}));
        assert_eq!(
            fs::read_to_string(dir.0.join("scanned")).unwrap(),
            "echo private"
        );
        let view = query(&dir.0).unwrap();
        assert_eq!(view["effects"]["checks"]["passed"], 1);
        assert_eq!(view["effects"]["native_execution"], "unconfirmed");
    }
}

#[test]
fn a_violation_introduced_by_transform_is_denied() {
    let dir = fixture_guard(
        Some("print('{\"format\":1,\"command\":\"echo forbidden\"}')"),
        SCANNER,
    );
    start(&dir);
    deny(&callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap());
    assert_eq!(
        fs::read_to_string(dir.0.join("scanned")).unwrap(),
        "echo forbidden"
    );
    let view = query(&dir.0).unwrap();
    assert_eq!(view["observation_gap"], false);
    assert_eq!(view["effects"]["status"], "available");
    assert_eq!(view["effects"]["checks"]["denied"], 1);
    assert_eq!(view["effects"]["checks"]["failed"], 0);
}

#[test]
fn scanner_errors_empty_output_and_inconsistent_verdicts_never_allow() {
    for body in ["sys.exit(1)","sys.exit(0)","print('{}')", "print(json.dumps({'ok':True,'verdict':'pass','summary':'','findings':[{'rule_id':'x','severity':'deny','desc_zh':'','desc_en':'','evidence':[]}],'language':'bash','engine_version':'0.12.0','elapsed_ms':0}))"] {
        let scanner=format!("import sys,json\nif sys.argv[-1]=='--version':\n print('agent-sec-cli 0.12.0');sys.exit(0)\n{body}");
        let dir=fixture_guard(None,&scanner);start(&dir);
        deny(&callback(&dir.0,native(&dir,"PreToolUse","s1","t1")).unwrap());
    }
}

#[test]
fn later_pin_failure_prevents_transform_side_effects() {
    let dir = fixture_guard(
        Some("import pathlib\npathlib.Path('transformed').write_text('bad')"),
        SCANNER,
    );
    let path = dir.0.join("binding.json");
    let mut binding: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    binding["prepared"]["config"]["tool_guard"]["scanner"]["program_sha256"] =
        json!("0".repeat(64));
    write(&path, &binding);
    start(&dir);
    deny(&callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap());
    assert!(!dir.0.join("transformed").exists());
    let view = query(&dir.0).unwrap();
    assert_eq!(view["observation_gap"], true);
    assert_eq!(view["effects"], json!({"status":"unavailable"}));
    callback(&dir.0, native(&dir, "SessionStart", "s2", "unused")).unwrap();
    let view = query(&dir.0).unwrap();
    assert_eq!(view["effects"]["status"], "available");
    assert_eq!(view["effects"]["checks"]["failed"], 0);
    // A late callback from the retired session cannot taint the new attachment.
    deny(&callback(&dir.0, native(&dir, "PreToolUse", "s1", "late")).unwrap());
    assert_eq!(query(&dir.0).unwrap()["effects"]["status"], "available");
}

#[test]
fn timeout_reaps_scanner_and_denies() {
    let scanner="import sys,os,pathlib,time\nif sys.argv[-1]=='--version':\n print('agent-sec-cli 0.12.0');sys.exit(0)\npathlib.Path('scanner-pid').write_text(str(os.getpid()))\ntime.sleep(10)";
    let dir = fixture_guard(None, scanner);
    start(&dir);
    deny(&callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap());
    let pid = fs::read_to_string(dir.0.join("scanner-pid")).unwrap();
    assert!(!Path::new("/proc").join(pid).exists());
}

#[test]
fn version_and_scan_can_share_more_than_one_second() {
    let scanner = format!("import time\ntime.sleep(0.6)\n{SCANNER}");
    let dir = fixture_guard_budget(None, &scanner, 2000);
    start(&dir);
    let response = callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap();
    assert_eq!(response, json!({}));
}

#[test]
fn scanner_does_not_restart_the_chain_budget_after_transform() {
    let transform =
        "import time\ntime.sleep(0.35)\nprint('{\"format\":1,\"command\":\"echo checked\"}')";
    let scanner = format!("import time,os,pathlib\npathlib.Path('scanner-pid').write_text(str(os.getpid()))\ntime.sleep(0.9)\n{SCANNER}");
    let dir = fixture_guard_budget(Some(transform), &scanner, 2000);
    start(&dir);
    deny(&callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap());
    assert!(!dir.0.join("scanned").exists());
    let pid = fs::read_to_string(dir.0.join("scanner-pid")).unwrap();
    assert!(!Path::new("/proc").join(pid).exists());
}

#[test]
fn admission_keeps_transform_and_scanner_budget_caps_distinct() {
    let dir = fixture_guard_budget(Some("print('{}')"), SCANNER, 2000);
    let path = dir.0.join("config.json");
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for (name, field, limit) in [
        ("scanner", "/tool_guard/scanner/limits/timeout_ms", 2001),
        (
            "transform",
            "/tool_guard/transforms/0/limits/timeout_ms",
            1001,
        ),
    ] {
        let mut config = original.clone();
        *config.pointer_mut(field).unwrap() = json!(limit);
        write(&path, &config);
        let digest = canonical::digest(&fs::read(&path).unwrap());
        assert!(prepare(&path, &digest, &dir.0.join(name), Path::new("/bin/echo")).is_err());
    }
}

#[test]
fn reset_during_transform_never_returns_a_candidate() {
    let transform =
        "import pathlib,time\npathlib.Path('entered').write_text('yes')\ntime.sleep(10)";
    let dir = fixture_guard(Some(transform), SCANNER);
    start(&dir);
    let root = dir.0.clone();
    let event = native(&dir, "PreToolUse", "s1", "t1");
    let worker = thread::spawn(move || callback(&root, event));
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dir.0.join("entered").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    callback(&dir.0, native(&dir, "SessionStart", "s2", "t2")).unwrap();
    deny(&worker.join().unwrap().unwrap());
    assert!(!dir.0.join("scanned").exists());
}

#[test]
fn malformed_transform_and_cancelled_callbacks_deny() {
    let dir = fixture_guard(
        Some("print('{\"format\":1,\"command\":\"echo ok\",\"continue\":true}')"),
        SCANNER,
    );
    start(&dir);
    deny(&callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap());
    deny(
        &callback_with_cancellation(&dir.0, native(&dir, "PreToolUse", "s1", "t2"), &|| true)
            .unwrap(),
    );
    assert!(!dir.0.join("scanned").exists());
}

#[test]
fn concurrent_calls_keep_separate_decisions_and_queries_do_not_rescan() {
    let dir = fixture_guard(None, SCANNER);
    start(&dir);
    let allowed = native(&dir, "PreToolUse", "s1", "allow-tool");
    let mut denied = native(&dir, "PreToolUse", "s1", "deny-tool");
    denied["tool_input"]["command"] = json!("echo forbidden");
    let root = dir.0.clone();
    let worker = thread::spawn(move || callback(&root, denied));
    let response = callback(&dir.0, allowed).unwrap();
    deny(&worker.join().unwrap().unwrap());
    assert_eq!(response, json!({}));
    fs::remove_file(dir.0.join("scanned")).unwrap();
    query(&dir.0).unwrap();
    query(&dir.0).unwrap();
    assert!(!dir.0.join("scanned").exists());
    for entry in fs::read_dir(dir.0.join("tool-check-journal")).unwrap() {
        let text = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!text.contains("echo private"));
        assert!(!text.contains("echo forbidden"));
    }
}

#[test]
fn command_bytes_respect_scanner_input_budget_even_with_argv_transport() {
    let dir = fixture_guard(None, SCANNER);
    let path = dir.0.join("binding.json");
    let mut binding: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    binding["prepared"]["config"]["tool_guard"]["scanner"]["limits"]["input_bytes"] = json!(1);
    write(&path, &binding);
    start(&dir);
    deny(&callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap());
    assert!(!dir.0.join("scanned").exists());
}
