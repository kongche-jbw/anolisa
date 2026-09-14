//! Dependency-only admission uses the execution Hosts without creating a session.

use aw_contracts::canonical;
use aw_core::ports::{Cancellation, NeverCancel};
use aw_hook_cli::preflight;
use serde_json::{json, Value};
use std::{fs, os::unix::fs::PermissionsExt, process::Command, sync::Arc};

// Other hook targets also use the payload and execution fixture helpers.
#[allow(dead_code)]
mod common;
use common::Directory;

fn config(dir: &Directory, name: &str, version: &str) -> Value {
    let code = format!(
        "import sys,pathlib,json\nif sys.argv[1:]==['--version']:\n assert not sys.stdin.read()\n pathlib.Path({name:?}).write_text('probed')\n print({version:?})\n sys.exit(0)\nassert {name:?}=='security'\nassert sys.argv[1:]==['scan-pii','--stdin','--format','json','--source','tool_output']\ntext=sys.stdin.read()\npathlib.Path('synthetic-scan').write_text(text)\nresponse=json.loads({response:?})\nresponse['summary']['bytes_scanned']=len(text.encode())\nprint(json.dumps(response))\n",
        response=json!({"ok":true,"verdict":"pass","findings":[],"elapsed_ms":0,
            "summary":{"total":0,"by_type":{},"by_category":{},"by_severity":{},
            "source":"tool_output","bytes_scanned":0,"truncated":false,
            "custom_rules":{"status":"absent","rule_count":0,"runtime_error_count":0,
            "budget_exhausted":false,"truncated":false}}}).to_string()
    );
    json!({"provider_id":name,"provider_version":"operator-release",
        "program":"/usr/bin/python3",
        "program_sha256":canonical::digest(&fs::read("/usr/bin/python3").unwrap()),
        "cwd":dir.0,"args":["-c",code],"environment":{
            "TOKENLESS_STATS_ENABLED":"0","TOKENLESS_SLS_ENABLED":"0",
            "TOKENLESS_COMPRESSION_ENABLED":"1"},"pins":[],
        "limits":{"timeout_ms":2000,"input_bytes":1024,"output_bytes":1024,"stderr_bytes":1024}})
}

fn settings(dir: &Directory, project: bool) -> Value {
    let mut value = json!({"mode":"inspect",
        "security":config(dir,"security","agent-sec-cli 0.12.0")});
    if project {
        value["mode"] = json!("project");
        value["tokenless"] = config(dir, "tokenless", "tokenless 0.8.1");
    }
    value
}

fn run(value: Value) -> Result<Value, preflight::Error> {
    preflight::run(
        serde_json::from_value(value).unwrap(),
        Arc::new(NeverCancel),
    )
}

#[test]
fn selected_dependencies_are_probed_with_synthetic_scan_but_without_session_state() {
    for project in [false, true] {
        let dir = Directory::new();
        let result = run(settings(&dir, project)).unwrap();
        assert_eq!(result["status"], "dependencies_ready");
        assert_eq!(result["agent_started"], false);
        assert_eq!(result["runtime_ready"], false);
        assert_eq!(
            result["providers"].as_array().unwrap().len(),
            if project { 2 } else { 1 }
        );
        assert!(dir.0.join("security").exists());
        assert_eq!(dir.0.join("tokenless").exists(), project);
        assert_eq!(
            fs::read_dir(&dir.0).unwrap().count(),
            if project { 3 } else { 2 }
        );
        assert_eq!(result["providers"][0]["protocol_probe"], "passed");
        assert_eq!(
            result["providers"][0]["protocol_profile"],
            "agent-sec.scan-pii/v1"
        );
        assert_eq!(result["providers"][0]["native_audit_possible"], true);
        assert_eq!(
            fs::read_to_string(dir.0.join("synthetic-scan")).unwrap(),
            "AW startup protocol probe."
        );
    }
}

#[test]
fn wrong_security_version_stops_before_tokenless_with_actionable_diagnostic() {
    let dir = Directory::new();
    let mut value = settings(&dir, true);
    value["security"] = config(&dir, "security", "agent-sec-cli 0.8.0");
    let error = run(value).unwrap_err().to_string();
    assert!(error.contains("SecCore preflight failed (expected CLI 0.12.0)"));
    assert!(error.contains("unsupported native CLI version"));
    assert!(!dir.0.join("tokenless").exists());
}

#[test]
fn changed_program_pin_prevents_execution() {
    let dir = Directory::new();
    let mut value = settings(&dir, false);
    value["security"]["program_sha256"] = json!("0".repeat(64));
    assert!(run(value)
        .unwrap_err()
        .to_string()
        .contains("program_changed"));
    assert!(!dir.0.join("security").exists());
}

#[test]
fn projection_uses_native_tokenless_controls_and_version_contract() {
    for invalid_controls in [false, true] {
        let dir = Directory::new();
        let mut value = settings(&dir, true);
        if invalid_controls {
            value["tokenless"]["environment"] = json!({});
        } else {
            value["tokenless"] = config(&dir, "tokenless", "tokenless 0.8.0");
        }
        let error = run(value).unwrap_err().to_string();
        assert!(error.contains("Tokenless preflight failed (expected CLI 0.8.1)"));
        assert_eq!(dir.0.join("tokenless").exists(), !invalid_controls);
    }
}

#[test]
fn dependency_selection_cannot_silently_fall_back() {
    let dir = Directory::new();
    let mut value = settings(&dir, true);
    value["mode"] = json!("inspect");
    assert!(serde_json::from_value::<preflight::Settings>(value).is_err());
    let mut value = settings(&dir, false);
    value["mode"] = json!("project");
    assert!(serde_json::from_value::<preflight::Settings>(value).is_err());
    let mut value = settings(&dir, true);
    value["tokenless"]["provider_id"] = value["security"]["provider_id"].clone();
    assert!(run(value).is_err());
    assert!(!dir.0.join("security").exists());
}

#[test]
fn private_settings_and_cli_failures_do_not_disclose_native_output() {
    let dir = Directory::new();
    let path = dir.0.join("settings.json");
    let mut value = settings(&dir, false);
    value["security"]["args"] = json!(["-c", "import sys;print('private-token');sys.exit(3)"]);
    fs::write(&path, value.to_string()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(preflight::read_settings(&path).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_aw-preflight-cli"))
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("SecCore preflight failed"));
    assert!(!error.contains("private-token"));
    fs::write(&path, "{\"mode\":\"inspect\",\"mode\":\"project\"}").unwrap();
    assert!(preflight::read_settings(&path).is_err());
}

struct Cancelled;
impl Cancellation for Cancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

#[test]
fn cancellation_before_admission_never_starts_a_provider() {
    let dir = Directory::new();
    let result = preflight::run(
        serde_json::from_value(settings(&dir, true)).unwrap(),
        Arc::new(Cancelled),
    );
    assert!(matches!(result, Err(preflight::Error::Cancelled)));
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
}

struct ProbeStarted(std::path::PathBuf);
impl Cancellation for ProbeStarted {
    fn is_cancelled(&self) -> bool {
        self.0.exists()
    }
}

#[test]
fn stalled_version_probe_is_reaped_on_timeout_or_cancellation() {
    for cancel in [false, true] {
        let dir = Directory::new();
        let mut value = settings(&dir, true);
        value["security"]["args"] = json!(["-c",
            "import os,pathlib,time;pathlib.Path('pid').write_text(str(os.getpid()));time.sleep(30)"]);
        value["security"]["limits"]["timeout_ms"] = json!(500);
        let cancellation: Arc<dyn Cancellation + Send + Sync> = if cancel {
            Arc::new(ProbeStarted(dir.0.join("pid")))
        } else {
            Arc::new(NeverCancel)
        };
        let started = std::time::Instant::now();
        let error = preflight::run(serde_json::from_value(value).unwrap(), cancellation)
            .unwrap_err()
            .to_string();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(
            error.contains(if cancel {
                "provider_cancelled"
            } else {
                "provider_timeout"
            }),
            "{error}"
        );
        let pid: u32 = fs::read_to_string(dir.0.join("pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        assert!(!dir.0.join("tokenless").exists());
    }
}

#[test]
fn readiness_output_has_a_deadline_and_restores_inherited_pipe_flags() {
    let dir = Directory::new();
    let path = dir.0.join("settings.json");
    fs::write(&path, settings(&dir, false).to_string()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    // The pipe starts full, so even a small readiness response must handle
    // backpressure. The child is always reaped, including assertion failures.
    let script = r#"
import fcntl,os,subprocess,sys,time
reader,writer=os.pipe()
child=None
try:
 fcntl.fcntl(writer,fcntl.F_SETPIPE_SZ,4096)
 original=fcntl.fcntl(writer,fcntl.F_GETFL)
 os.write(writer,b'x'*4096)
 started=time.monotonic()
 child=subprocess.Popen(sys.argv[1:],stdout=writer,stderr=subprocess.PIPE)
 _,error=child.communicate(timeout=10)
 assert child.returncode==1,error
 assert 4 <= time.monotonic()-started < 10
 assert b'preflight output unavailable' in error,error
 assert fcntl.fcntl(writer,fcntl.F_GETFL)==original
finally:
 if child is not None and child.poll() is None:
  child.terminate()
  try:child.wait(timeout=3)
  except subprocess.TimeoutExpired:child.kill();child.wait(timeout=3)
 os.close(reader)
 os.close(writer)
"#;
    let output = Command::new("/usr/bin/python3")
        .args(["-B", "-c", script, env!("CARGO_BIN_EXE_aw-preflight-cli")])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.0.join("security").exists());
}

#[test]
fn same_version_without_scan_protocol_never_reports_readiness_or_probes_tokenless() {
    let dir = Directory::new();
    let path = dir.0.join("settings.json");
    let mut value = settings(&dir, true);
    value["security"]["args"] = json!([
        "-c",
        "import sys;print('agent-sec-cli 0.12.0') if sys.argv[1:]==['--version'] else sys.exit(2)"
    ]);
    fs::write(&path, value.to_string()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_aw-preflight-cli"))
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("protocol probe failed: native_failed"));
    assert!(!dir.0.join("tokenless").exists());
}
