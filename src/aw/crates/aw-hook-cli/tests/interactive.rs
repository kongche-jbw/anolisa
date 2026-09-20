//! Stateful attachment regressions use the real bounded observer Host.

use aw_contracts::canonical;
use aw_hook_cli::{
    interactive::{callback, callback_with_cancellation, prepare, query, HerdrBridge},
    process_identity,
};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    thread,
    time::{Duration, Instant},
};

// Shared fixture directory owns its teardown; other helpers belong to old targets.
#[allow(dead_code)]
mod common;
use common::Directory;

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn config(dir: &Directory, code: &str) -> Value {
    let native = |program: &str, args: Value| {
        json!({
            "provider_id":"observer","provider_version":"1","program":program,
            "program_sha256":canonical::digest(&fs::read(program).unwrap()),
            "cwd":dir.0,"args":args,"environment":{},"pins":[],
            "limits":{"timeout_ms":500,"input_bytes":65536,"output_bytes":1024,"stderr_bytes":1024}
        })
    };
    json!({"format":1,"required_safety":false,"native_config_directory":dir.0,
        "qoder":{"program":"/bin/echo","program_sha256":canonical::digest(&fs::read("/bin/echo").unwrap())},
        "handler":native("/usr/bin/python3", json!(["-c",code]))})
}

fn fixture(code: &str) -> Directory {
    fixture_config(code, |_| {})
}

fn fixture_config(code: &str, update: impl FnOnce(&mut Value)) -> Directory {
    let dir = Directory::new();
    let mut value = config(&dir, code);
    update(&mut value);
    let path = dir.0.join("config.json");
    write(&path, &value);
    let digest = canonical::digest(&fs::read(&path).unwrap());
    prepare(&path, &digest, &dir.0.join("scope"), Path::new("/bin/echo")).unwrap();
    let prepared: Value =
        serde_json::from_slice(&fs::read(dir.0.join("scope/prepared.json")).unwrap()).unwrap();
    let pid = std::process::id();
    let (_, ticks) = process_identity(pid).unwrap();
    write(
        &dir.0.join("binding.json"),
        &json!({"prepared":prepared,"runtime_id":"runtime-one","agent_pid":pid,"agent_ticks":ticks}),
    );
    write(
        &dir.0.join("state.json"),
        &json!({"session_id":null,"attachment":0,"attached":false,"retired_sessions":[],"occurrences":0,"gap":false}),
    );
    write(&dir.0.join("state.lock"), &json!({}));
    fs::create_dir(dir.0.join("calls")).unwrap();
    if value["format"] == 2 {
        fs::create_dir(dir.0.join("notifications")).unwrap();
    }
    dir
}

fn event(dir: &Directory, name: &str, session: &str, tool: &str) -> Value {
    json!({"hook_event_name":name,"session_id":session,"source":"startup","cwd":dir.0,
        "tool_use_id":tool,"tool_name":"Bash","tool_input":{"command":"echo private"},
        "tool_response":"private result"})
}

const OBSERVER: &str = "import json,sys,pathlib\ne=json.load(sys.stdin)\nassert e['event']=='tool.result_observed'\nassert 'tool_input' not in e and 'tool_response' not in e\npathlib.Path('called').write_text(e['session_id'])\nprint('{\"format\":1,\"observed\":true}')";

#[test]
fn agent_pin_accepts_large_bundles_without_raising_the_host_limit() {
    use sha2::{Digest, Sha256};
    let dir = Directory::new();
    let program = dir.0.join("large-agent");
    let file = fs::File::create(&program).unwrap();
    file.set_len(65 * 1024 * 1024).unwrap();
    let mut digest = Sha256::new();
    for _ in 0..1040 {
        digest.update([0; 65536]);
    }
    let mut value = config(&dir, OBSERVER);
    value["qoder"] = json!({"program":program,"program_sha256":format!("{:x}",digest.finalize())});
    let path = dir.0.join("config.json");
    write(&path, &value);
    let pin = canonical::digest(&fs::read(&path).unwrap());
    prepare(&path, &pin, &dir.0.join("scope"), Path::new("/bin/echo")).unwrap();
    file.set_len(513 * 1024 * 1024).unwrap();
    assert!(prepare(
        &path,
        &pin,
        &dir.0.join("too-large"),
        Path::new("/bin/echo")
    )
    .is_err());
    assert!(!dir.0.join("too-large").exists());
}

#[test]
fn changed_optional_handler_preserves_native_execution() {
    let dir = fixture(OBSERVER);
    let path = dir.0.join("binding.json");
    let mut binding: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    binding["prepared"]["config"]["handler"]["program_sha256"] = json!("0".repeat(64));
    write(&path, &binding);
    callback(&dir.0, event(&dir, "SessionStart", "s1", "t1")).unwrap();
    callback(&dir.0, event(&dir, "PreToolUse", "s1", "t1")).unwrap();
    assert!(callback(&dir.0, event(&dir, "PostToolUse", "s1", "t1"))
        .unwrap()
        .get("systemMessage")
        .is_some());
    assert!(!dir.0.join("called").exists());
    assert_eq!(query(&dir.0).unwrap()["failed"], 1);
}

#[test]
fn reset_fences_old_events_and_query_never_executes_a_handler() {
    let dir = fixture(OBSERVER);
    callback(&dir.0, event(&dir, "SessionStart", "s1", "t1")).unwrap();
    callback(&dir.0, event(&dir, "PreToolUse", "s1", "t1")).unwrap();
    callback(&dir.0, event(&dir, "PostToolUse", "s1", "t1")).unwrap();
    assert_eq!(query(&dir.0).unwrap()["observed"], 1);
    fs::remove_file(dir.0.join("called")).unwrap();
    assert_eq!(query(&dir.0).unwrap()["observed"], 1);
    assert!(!dir.0.join("called").exists());
    let mut reset = event(&dir, "SessionStart", "s2", "t2");
    reset["source"] = json!("clear");
    callback(&dir.0, reset).unwrap();
    assert!(callback(&dir.0, event(&dir, "PostToolUse", "s1", "t1")).is_err());
    assert_eq!(query(&dir.0).unwrap()["observed"], 0);
    callback(&dir.0, event(&dir, "PreToolUse", "s2", "t2")).unwrap();
    callback(&dir.0, event(&dir, "PostToolUse", "s2", "t2")).unwrap();
    assert_eq!(query(&dir.0).unwrap()["observed"], 1);
    assert_eq!(query(&dir.0).unwrap()["attachment"], 2);
}

#[test]
fn duplicate_or_unbound_post_tool_never_reexecutes() {
    let dir = fixture(OBSERVER);
    callback(&dir.0, event(&dir, "SessionStart", "s1", "t1")).unwrap();
    assert!(callback(&dir.0, event(&dir, "PostToolUse", "s1", "t1")).is_err());
    callback(&dir.0, event(&dir, "PreToolUse", "s1", "t1")).unwrap();
    callback(&dir.0, event(&dir, "PostToolUse", "s1", "t1")).unwrap();
    fs::remove_file(dir.0.join("called")).unwrap();
    assert!(callback(&dir.0, event(&dir, "PostToolUse", "s1", "t1")).is_err());
    assert!(callback(&dir.0, event(&dir, "PreToolUse", "s1", "t1")).is_err());
    assert!(!dir.0.join("called").exists());
}

#[test]
fn reset_during_handler_keeps_completion_in_old_attachment() {
    let code = "import pathlib,time\npathlib.Path('entered').write_text('yes')\ntime.sleep(.2)\nprint('{\"format\":1,\"observed\":true}')";
    let dir = fixture(code);
    callback(&dir.0, event(&dir, "SessionStart", "s1", "t1")).unwrap();
    callback(&dir.0, event(&dir, "PreToolUse", "s1", "t1")).unwrap();
    let root = dir.0.clone();
    let post = event(&dir, "PostToolUse", "s1", "t1");
    let child = thread::spawn(move || callback(&root, post));
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dir.0.join("entered").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    callback(&dir.0, event(&dir, "SessionStart", "s2", "t2")).unwrap();
    assert!(child
        .join()
        .unwrap()
        .unwrap()
        .get("systemMessage")
        .is_some());
    assert_eq!(query(&dir.0).unwrap()["calls"], 0);
}

#[test]
fn optional_timeout_reclaims_handler_and_reports_failure() {
    let dir = fixture("import pathlib,os,time\npathlib.Path('handler-pid').write_text(str(os.getpid()))\ntime.sleep(10)");
    callback(&dir.0, event(&dir, "SessionStart", "s1", "t1")).unwrap();
    callback(&dir.0, event(&dir, "PreToolUse", "s1", "t1")).unwrap();
    assert!(callback(&dir.0, event(&dir, "PostToolUse", "s1", "t1"))
        .unwrap()
        .get("systemMessage")
        .is_some());
    let pid = fs::read_to_string(dir.0.join("handler-pid")).unwrap();
    assert!(!Path::new("/proc").join(pid).exists());
    assert_eq!(query(&dir.0).unwrap()["failed"], 1);
}

#[test]
fn cancellation_reclaims_an_active_observer_without_replaying_it() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let dir = fixture("import pathlib,os,time\npathlib.Path('handler-pid').write_text(str(os.getpid()))\ntime.sleep(10)");
    callback(&dir.0, event(&dir, "SessionStart", "s1", "t1")).unwrap();
    callback(&dir.0, event(&dir, "PreToolUse", "s1", "t1")).unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let root = dir.0.clone();
    let post = event(&dir, "PostToolUse", "s1", "t1");
    let worker = thread::spawn(move || {
        callback_with_cancellation(&root, post, &|| flag.load(Ordering::Relaxed))
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dir.0.join("handler-pid").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    cancelled.store(true, Ordering::Relaxed);
    assert!(worker
        .join()
        .unwrap()
        .unwrap()
        .get("systemMessage")
        .is_some());
    let pid = fs::read_to_string(dir.0.join("handler-pid")).unwrap();
    assert!(!Path::new("/proc").join(pid).exists());
    assert_eq!(query(&dir.0).unwrap()["failed"], 1);
    assert!(callback(&dir.0, event(&dir, "PostToolUse", "s1", "t1")).is_err());
}

#[test]
fn trust_and_required_safety_fail_before_creating_resources() {
    let dir = Directory::new();
    let path = dir.0.join("config.json");
    let mut value = config(&dir, OBSERVER);
    write(&path, &value);
    let root = dir.0.join("scope");
    assert!(prepare(&path, "wrong", &root, Path::new("/bin/echo")).is_err());
    value["required_safety"] = json!(true);
    write(&path, &value);
    let digest = canonical::digest(&fs::read(&path).unwrap());
    assert!(prepare(&path, &digest, &root, Path::new("/bin/echo")).is_err());
    assert!(!root.exists());
}

#[test]
fn mismatched_owner_and_ambiguous_reset_are_not_accepted() {
    let dir = fixture(OBSERVER);
    callback(&dir.0, event(&dir, "SessionStart", "s1", "t1")).unwrap();
    let mut reset = event(&dir, "SessionStart", "s1", "t1");
    reset["source"] = json!("clear");
    assert!(callback(&dir.0, reset).is_err());
    assert_eq!(query(&dir.0).unwrap()["attached"], false);
    let path = dir.0.join("binding.json");
    let mut binding: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    binding["agent_ticks"] = json!(0);
    write(&path, &binding);
    assert!(callback(&dir.0, event(&dir, "PreToolUse", "s1", "t1")).is_err());
    assert!(!dir.0.join("called").exists());
}

#[test]
fn missing_herdr_socket_has_bounded_drop_and_no_handler_calls() {
    let dir = fixture(OBSERVER);
    let start = Instant::now();
    let bridge = HerdrBridge::start(
        dir.0.join("scope"),
        dir.0.join("absent.sock"),
        "pane".into(),
    )
    .unwrap();
    drop(bridge);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!dir.0.join("called").exists());
}

#[test]
fn herdr_publishes_only_verified_pane_observation_counters() {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        sync::mpsc,
    };
    let dir = fixture(OBSERVER);
    callback(&dir.0, event(&dir, "SessionStart", "s1", "t1")).unwrap();
    callback(&dir.0, event(&dir, "PreToolUse", "s1", "t1")).unwrap();
    callback(&dir.0, event(&dir, "PostToolUse", "s1", "t1")).unwrap();
    let run = dir.0.join("scope/run-test");
    fs::create_dir(&run).unwrap();
    for name in ["binding.json", "state.json", "state.lock", "calls"] {
        fs::rename(dir.0.join(name), run.join(name)).unwrap();
    }
    let socket = dir.0.join("herdr.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (sender, receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        for _ in 0..2 {
            let mut connection = loop {
                match listener.accept() {
                    Ok((connection, _)) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline);
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            };
            connection
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut line = String::new();
            BufReader::new(connection.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["params"]["pane_id"], "test-pane");
            let result = match request["method"].as_str().unwrap() {
                "pane.process_info" => json!({"process_info":{"shell_pid":std::process::id()}}),
                "pane.report_metadata" => {
                    sender.send(request["params"].clone()).unwrap();
                    json!({})
                }
                _ => panic!("viewer attempted a non-observation RPC"),
            };
            writeln!(
                connection,
                "{}",
                json!({"id":"aw-interactive","result":result})
            )
            .unwrap();
        }
    });
    let bridge = HerdrBridge::start(dir.0.join("scope"), socket, "test-pane".into()).unwrap();
    let report = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(
        report["tokens"]["aw_observation"],
        "1 observed / 0 failed / 0 pending"
    );
    assert_eq!(report["ttl_ms"], 3000);
    drop(bridge);
    server.join().unwrap();
    assert_eq!(fs::read_to_string(dir.0.join("called")).unwrap(), "s1");
}

#[path = "interactive/lifecycle.rs"]
mod lifecycle;

#[path = "interactive/input_response.rs"]
mod input_response;
#[path = "interactive/runtime_events.rs"]
mod runtime_events;
#[path = "interactive/tool_guard.rs"]
mod tool_guard;

#[path = "interactive/stop_response.rs"]
mod stop_response;

#[path = "interactive/tool_response.rs"]
mod tool_response;
