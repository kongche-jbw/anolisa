//! Exercise the common Provider protocol through real callbacks and a private daemon.

use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::Write,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

const AW: &str = env!("CARGO_BIN_EXE_aw");
const ADAPTERS: [&str; 4] = ["qoder", "openclaw", "hermes", "qwenpaw"];
static NEXT: AtomicUsize = AtomicUsize::new(0);

// Faults affect invoke unless discovery/configuration failure is explicitly requested.
const FIXTURE: &str = r#"
import hashlib, json, os, sys, time
request = json.load(sys.stdin)
mode = os.environ.get('AW_TEST_MODE', 'allow')
method = request['method']
trace = os.open(sys.argv[1], os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
os.write(trace, (json.dumps({'pid': os.getpid(), 'request': request}) + '\n').encode())
os.close(trace)
response = {'api_version': 'aw-provider/v1alpha1', 'request_id': request['request_id'], 'status': 'ok'}
if method == 'describe':
    if mode == 'describe_wait':
        time.sleep(30)
    response['operations'] = [{'name': 'check', 'events': ['tool.before', 'tool.after'], 'effects': ['observe', 'block']}]
    if mode == 'describe_mismatch':
        response['operations'][0]['effects'] = ['observe']
elif method == 'validate_config':
    if mode == 'config_mismatch' or request['config'] != {'policy': 'fixture'}:
        response.update(status='error', error_code='invalid_configuration')
elif method == 'invoke':
    unsigned = dict(request)
    unsigned.pop('input_digest')
    digest = 'sha256:' + hashlib.sha256(json.dumps(unsigned, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
    assert request['input_digest'] == digest
    assert request['operation'] == 'check'
    assert request['config'] == {'policy': 'fixture'}
    assert request['budget_ms'] > 0
    assert request['config_revision']
    response.update(input_digest=digest, effects=[])
    if mode == 'block':
        response['effects'] = [{'type': 'block', 'reason_code': 'fixture.denied'}]
    elif mode == 'observe':
        response['effects'] = [{'type': 'observe'}]
    elif mode == 'wrong_id':
        response['request_id'] = 'wrong-request'
    elif mode == 'wrong_digest':
        response['input_digest'] = 'sha256:wrong'
    elif mode == 'illegal_effect':
        response['effects'] = [{'type': 'ask'}]
    elif mode == 'unknown_field':
        response['extra'] = 'not-admitted'
    elif mode == 'unknown_effect_field':
        response['effects'] = [{'type': 'observe', 'command': 'not-admitted'}]
    elif mode == 'missing_effects':
        response.pop('effects')
    elif mode == 'nonzero':
        sys.stderr.write('private-provider-diagnostic-sentinel')
        sys.exit(9)
    elif mode == 'timeout':
        time.sleep(30)
    elif mode == 'overlimit':
        print('x' * 8192)
        sys.exit(0)
    elif mode == 'malformed':
        print('not-json')
        sys.exit(0)
else:
    raise AssertionError('Unexpected method')
print(json.dumps(response))
"#;

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            // Each test command owns its process group; never signal unrelated processes.
            unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
        }
        let _ = self.0.wait();
    }
}

struct FixtureGroup {
    pid: u32,
    start_time: String,
}

fn process_start_time(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()?
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
}

impl Drop for FixtureGroup {
    fn drop(&mut self) {
        if process_start_time(self.pid).as_ref() == Some(&self.start_time) {
            // Failure cleanup must also cover the Provider's separately owned group.
            // The start time prevents signalling a reused PID after AW reaps it.
            unsafe { libc::kill(-(self.pid as i32), libc::SIGKILL) };
        }
    }
}

struct Lab {
    dir: PathBuf,
    config: PathBuf,
    socket: PathBuf,
    server: Option<OwnedChild>,
    records: Vec<Value>,
}

impl Lab {
    fn new(on_error: &str) -> Self {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/provider-tests");
        fs::create_dir_all(&base).unwrap();
        let dir = base.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let dir = dir.canonicalize().unwrap();
        let fixture = dir.join("provider.py");
        fs::write(&fixture, FIXTURE).unwrap();
        let config = dir.join("aw.json");
        let value = json!({
            "apiVersion": "aw/v1alpha1", "kind": "AWConfiguration", "metadata": {"name": "provider-test"},
            "spec": {
                "daemon": {"startup": "external", "endpoint": "auto", "state_dir": "auto"},
                "execution": {"guarantee": "native_hook", "default_event_budget_ms": 5000},
                "audit": {"enabled": true, "payload": "metadata_only"},
                "agents": {
                    "qoder": {"adapter": "qoder", "argv": ["/bin/sh", "-c", "printf started > host-started"]},
                    "openclaw": {"adapter": "openclaw", "argv": ["/bin/true"]},
                    "hermes": {"adapter": "hermes", "argv": ["/bin/true"]},
                    "qwenpaw": {"adapter": "qwenpaw", "argv": ["/bin/true"]},
                },
                "providers": {"policy": {
                    "protocol": "aw-provider/v1alpha1",
                    "transport": {"type": "stdio", "location": "agent", "argv": ["python3", fixture, dir.join("requests.jsonl")]},
                    "timeout_ms": 1000, "max_output_bytes": 4096, "config": {"policy": "fixture"},
                }},
                "events": {
                    "tool.before": {"enabled": true, "required": true, "steps": [{"id": "before", "provider": "policy", "operation": "check", "effects": ["observe", "block"], "on_error": on_error}]},
                    "tool.after": {"enabled": true, "required": true, "steps": [{"id": "after", "provider": "policy", "operation": "check", "effects": ["observe"], "on_error": "report"}]},
                },
            },
        });
        fs::write(&config, serde_json::to_vec(&value).unwrap()).unwrap();
        Self {
            socket: dir.join("aw.sock"),
            dir,
            config,
            server: None,
            records: Vec::new(),
        }
    }

    fn spawn(&mut self, command: &mut Command, name: &str, timeout_seconds: u64) -> OwnedChild {
        let record = json!({
            "command": format!("{command:?}"), "cwd": self.dir, "ports": [],
            "log": self.dir.join(format!("{name}.stderr")), "timeout_seconds": timeout_seconds,
        });
        self.records.push(record);
        self.write_records();
        let child = command.process_group(0).spawn().unwrap();
        let record = self.records.last_mut().unwrap();
        record["pid"] = json!(child.id());
        record["pgid"] = json!(child.id());
        record["stop"] = json!(format!("kill -TERM -- -{}", child.id()));
        self.write_records();
        OwnedChild(child)
    }

    fn write_records(&self) {
        fs::write(
            self.dir.join("processes.json"),
            serde_json::to_vec_pretty(&self.records).unwrap(),
        )
        .unwrap();
    }

    fn start(&mut self) {
        let mut command = Command::new(AW);
        command
            .args(["serve", "--config"])
            .arg(&self.config)
            .arg("--socket")
            .arg(&self.socket)
            .args(["--idle-timeout", "30"])
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .stdout(File::create(self.dir.join("server.stdout")).unwrap())
            .stderr(File::create(self.dir.join("server.stderr")).unwrap());
        self.server = Some(self.spawn(&mut command, "server", 30));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.socket.exists() {
            assert!(Instant::now() < deadline, "daemon readiness timeout");
            assert!(
                self.server
                    .as_mut()
                    .unwrap()
                    .0
                    .try_wait()
                    .unwrap()
                    .is_none(),
                "daemon exited: {}",
                fs::read_to_string(self.dir.join("server.stderr")).unwrap()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn run(&mut self, command: &mut Command, input: &[u8]) -> Output {
        let name = format!("call-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let stdout = self.dir.join(format!("{name}.stdout"));
        let stderr = self.dir.join(format!("{name}.stderr"));
        command
            .current_dir(&self.dir)
            .stdin(Stdio::piped())
            .stdout(File::create(&stdout).unwrap())
            .stderr(File::create(&stderr).unwrap());
        let mut child = self.spawn(command, &name, 6);
        child.0.stdin.take().unwrap().write_all(input).unwrap();
        let deadline = Instant::now() + Duration::from_secs(6);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "AW callback exceeded test deadline"
            );
            thread::sleep(Duration::from_millis(10));
        };
        Output {
            status,
            stdout: fs::read(stdout).unwrap(),
            stderr: fs::read(stderr).unwrap(),
        }
    }

    fn hook(&mut self, adapter: &str, event: &str, mode: &str, fallback: Option<&str>) -> Output {
        let mut command = Command::new(AW);
        command
            .args(["hook", "--socket"])
            .arg(&self.socket)
            .args(["--agent", adapter, "--event", event, "--provider", "policy"])
            .env("AW_TEST_MODE", mode);
        if let Some(policy) = fallback {
            command.args(["--adapter", adapter, "--on-error", policy]);
        }
        self.run(
            &mut command,
            &serde_json::to_vec(&payload(adapter, event)).unwrap(),
        )
    }

    fn requests(&self) -> Vec<Value> {
        fs::read_to_string(self.dir.join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()["request"].clone())
            .collect()
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        if self.server.is_some() {
            let mut command = Command::new(AW);
            command.args(["stop", "--socket"]).arg(&self.socket);
            let _ = self.run(&mut command, b"");
            let deadline = Instant::now() + Duration::from_secs(3);
            while self
                .server
                .as_mut()
                .unwrap()
                .0
                .try_wait()
                .ok()
                .flatten()
                .is_none()
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(10));
            }
            drop(self.server.take());
        }
        let socket_removed = !self.socket.exists();
        fs::remove_dir_all(&self.dir).unwrap();
        if !thread::panicking() {
            assert!(socket_removed, "daemon socket survived test teardown");
        }
    }
}

fn payload(adapter: &str, event: &str) -> Value {
    let before = event == "tool.before";
    let arguments = json!({"key": "$(literal)", "nested": {"flag": true}});
    let result = json!({"content": [{"type": "text", "text": "tool-result-sentinel"}]});
    match adapter {
        "qoder" => {
            json!({"hook_event_name": if before {"PreToolUse"} else {"PostToolUse"}, "tool_name": "custom-tool", "tool_input": arguments, "tool_use_id": "call-1", "session_id": "session-1", "tool_response": result})
        }
        "openclaw" => {
            json!({"hook": if before {"before_tool_call"} else {"after_tool_call"}, "event": {"toolName": "custom-tool", "params": arguments, "toolCallId": "call-1", "result": result}, "context": {"sessionId": "session-1"}})
        }
        "hermes" => {
            json!({"hook_event_name": if before {"pre_tool_call"} else {"post_tool_call"}, "tool_name": "custom-tool", "tool_input": arguments, "session_id": "session-1", "extra": {"tool_call_id": "call-1", "result": result.to_string(), "status": "ok"}})
        }
        "qwenpaw" => {
            json!({"event": event, "tool_call": {"name": "custom-tool", "input": arguments.to_string(), "id": "call-1"}, "tool_response": result})
        }
        _ => panic!("test adapter is unsupported"),
    }
}

fn assert_neutral(output: &Output) {
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"{}\n");
}

fn assert_blocked(adapter: &str, output: &Output) {
    if matches!(adapter, "qoder" | "qwenpaw") {
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("AW policy blocked this tool call")
        );
    } else {
        assert!(output.status.success(), "{output:?}");
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        if adapter == "hermes" {
            assert_eq!(response["action"], "block");
            assert_eq!(response["message"], "AW policy blocked this tool call");
        } else {
            assert_eq!(response["block"], true);
            assert_eq!(response["blockReason"], "AW policy blocked this tool call");
        }
    }
}

#[test]
fn one_provider_receives_common_events_and_projects_all_four_native_responses() {
    let mut lab = Lab::new("block");
    lab.start();
    for adapter in ADAPTERS {
        assert_neutral(&lab.hook(adapter, "tool.before", "allow", None));
        assert_blocked(adapter, &lab.hook(adapter, "tool.before", "block", None));
        assert_neutral(&lab.hook(adapter, "tool.after", "observe", None));
    }
    let requests = lab.requests();
    assert_eq!(requests.len(), 36);
    for transaction in requests.chunks_exact(3) {
        assert_eq!(transaction[0]["method"], "describe");
        assert_eq!(transaction[1]["method"], "validate_config");
        assert_eq!(transaction[2]["method"], "invoke");
        let event = &transaction[2]["event"];
        assert_eq!(event["tool"]["name"], "custom-tool");
        assert_eq!(event["tool"]["native_name"], "custom-tool");
        assert_eq!(
            event["tool"]["input"],
            json!({"key": "$(literal)", "nested": {"flag": true}})
        );
        assert_eq!(event["tool"]["call_id"], "call-1");
        let adapter = event["agent"]["adapter"].as_str().unwrap();
        assert_eq!(
            event["native"],
            payload(adapter, event["name"].as_str().unwrap())
        );
        if event["name"] == "tool.after" {
            assert!(!event["tool"]["result"].is_null());
            assert_eq!(event["tool"]["result"].is_string(), adapter == "hermes");
        } else {
            assert!(event["tool"]["result"].is_null());
        }
    }
    let audit = fs::read_to_string(lab.dir.join("audit.jsonl")).unwrap();
    assert_eq!(audit.lines().count(), 12);
    assert!(!audit.contains("tool-result-sentinel"));
    assert!(!audit.contains("$(literal)"));
    assert_eq!(
        audit
            .lines()
            .filter(|line| line.contains("\"disposition\":\"block\""))
            .count(),
        4
    );
}

#[test]
fn invalid_provider_replies_and_execution_failures_apply_declared_error_policy() {
    for policy in ["block", "report"] {
        let mut lab = Lab::new(policy);
        lab.start();
        for mode in [
            "wrong_id",
            "wrong_digest",
            "illegal_effect",
            "unknown_field",
            "unknown_effect_field",
            "missing_effects",
            "nonzero",
            "timeout",
            "overlimit",
            "malformed",
        ] {
            for adapter in ADAPTERS {
                let output = lab.hook(adapter, "tool.before", mode, None);
                if policy == "block" {
                    assert_blocked(adapter, &output);
                } else {
                    assert_neutral(&output);
                    assert!(String::from_utf8_lossy(&output.stderr).contains("on_error=report"));
                }
                assert!(!String::from_utf8_lossy(&output.stderr)
                    .contains("private-provider-diagnostic-sentinel"));
            }
        }
        let audit = fs::read_to_string(lab.dir.join("audit.jsonl")).unwrap();
        assert!(!audit.contains("private-provider-diagnostic-sentinel"));
        assert!(audit
            .lines()
            .all(|line| serde_json::from_str::<Value>(line).unwrap()["disposition"] == "error"));
    }
}

#[test]
fn after_rejects_control_effect_without_pretending_to_undo_the_tool() {
    let mut lab = Lab::new("block");
    lab.start();
    for adapter in ADAPTERS {
        let output = lab.hook(adapter, "tool.after", "block", None);
        assert_neutral(&output);
        assert!(String::from_utf8_lossy(&output.stderr).contains("on_error=report"));
    }
}

#[test]
fn missing_daemon_is_translated_by_callback_fallback_for_every_adapter() {
    let mut lab = Lab::new("block");
    for adapter in ADAPTERS {
        assert_blocked(
            adapter,
            &lab.hook(adapter, "tool.before", "allow", Some("block")),
        );
        for event in ["tool.before", "tool.after"] {
            let output = lab.hook(adapter, event, "allow", Some("report"));
            assert_neutral(&output);
            assert!(String::from_utf8_lossy(&output.stderr).contains("on_error=report"));
        }
    }
    assert!(!lab.dir.join("requests.jsonl").exists());
}

#[test]
fn check_and_run_reject_discovery_or_configuration_mismatch_before_host_start() {
    let mut lab = Lab::new("block");
    for mode in ["describe_mismatch", "config_mismatch"] {
        for operation in ["check", "run"] {
            let mut command = Command::new(AW);
            command
                .args([operation, "qoder", "--config"])
                .arg(&lab.config)
                .env("AW_TEST_MODE", mode);
            let output = lab.run(&mut command, b"");
            assert!(!output.status.success(), "{mode}: {output:?}");
            assert!(!lab.dir.join("host-started").exists());
        }
    }
    let mut command = Command::new(AW);
    command
        .args(["check", "qoder", "--config"])
        .arg(&lab.config)
        .env("AW_TEST_MODE", "allow");
    assert!(lab.run(&mut command, b"").status.success());
    assert!(lab
        .requests()
        .iter()
        .all(|request| request["method"] != "invoke"));
    assert!(!lab.socket.exists());
}

#[test]
fn cancelling_admission_reaps_the_provider_before_host_launch() {
    for (signal, operation) in [
        (libc::SIGINT, "check"),
        (libc::SIGTERM, "check"),
        (libc::SIGINT, "run"),
        (libc::SIGTERM, "run"),
    ] {
        let mut lab = Lab::new("block");
        let mut config: Value = serde_json::from_slice(&fs::read(&lab.config).unwrap()).unwrap();
        config["spec"]["providers"]["policy"]["timeout_ms"] = json!(10_000);
        fs::write(&lab.config, serde_json::to_vec(&config).unwrap()).unwrap();
        let mut command = Command::new(AW);
        command
            .args([operation, "qoder", "--config"])
            .arg(&lab.config)
            .env("AW_TEST_MODE", "describe_wait")
            .current_dir(&lab.dir)
            .stdin(Stdio::null())
            .stdout(File::create(lab.dir.join("admission.stdout")).unwrap())
            .stderr(File::create(lab.dir.join("admission.stderr")).unwrap());
        let mut child = lab.spawn(&mut command, "admission", 6);
        let deadline = Instant::now() + Duration::from_secs(3);
        let provider_pid = loop {
            let pid = fs::read_to_string(lab.dir.join("requests.jsonl"))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(text.trim()).ok())
                .and_then(|record| record["pid"].as_u64())
                .and_then(|pid| u32::try_from(pid).ok());
            if let Some(pid) = pid {
                break pid;
            }
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "admission exited before discovery"
            );
            assert!(
                Instant::now() < deadline,
                "Provider discovery readiness timeout"
            );
            thread::sleep(Duration::from_millis(10));
        };
        let _provider = FixtureGroup {
            pid: provider_pid,
            start_time: process_start_time(provider_pid)
                .expect("the discovery fixture is sleeping"),
        };
        assert_eq!(unsafe { libc::kill(child.0.id() as i32, signal) }, 0);
        let deadline = Instant::now() + Duration::from_secs(3);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "cancelled admission did not exit"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert!(!status.success());
        assert!(
            process_start_time(provider_pid).is_none(),
            "cancelled discovery Provider survived AW"
        );
        assert!(!lab.dir.join("host-started").exists());
        assert!(lab
            .requests()
            .iter()
            .all(|request| request["method"] == "describe"));
    }
}
