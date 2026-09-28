//! Check generated host configuration with bounded local fake Agents.
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

const AW: &str = env!("CARGO_BIN_EXE_aw");
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/launch-tests");
        fs::create_dir_all(&root).unwrap();
        let path = root.canonicalize().unwrap().join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        assert!(path.join("aw.sock").as_os_str().len() < 108);
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
        assert!(!self.0.exists());
    }
}

struct OwnedProcess {
    child: Child,
    ledger: PathBuf,
}

impl OwnedProcess {
    fn spawn(command: &mut Command, directory: &Path, name: &str) -> Self {
        let stdout = directory.join(format!("{name}.stdout"));
        let stderr = directory.join(format!("{name}.stderr"));
        command
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(File::create(&stdout).unwrap())
            .stderr(File::create(&stderr).unwrap())
            .process_group(0);
        let child = command.spawn().unwrap();
        let ledger = directory.join("processes.jsonl");
        let record = json!({
            "command": std::iter::once(command.get_program())
                .chain(command.get_args()).map(|arg| arg.to_string_lossy()).collect::<Vec<_>>(),
            "cwd": directory, "pid": child.id(), "process_group": child.id(),
            "ports": [], "stdout": stdout, "stderr": stderr,
            "deadline_seconds": 10, "stop_command": format!("kill -TERM -- -{}", child.id())
        });
        Self::record(&ledger, &record);
        eprintln!("owned launcher test process: {record}");
        Self { child, ledger }
    }

    fn record(ledger: &Path, value: &Value) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(ledger)
            .unwrap();
        writeln!(file, "{value}").unwrap();
    }

    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "owned process deadline exceeded");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            // The child leads the process group created by process_group(0).
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGTERM) };
        }
        assert!(self.wait().success(), "owned daemon did not stop cleanly");
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // Only this test's recorded group is signalled on assertion failure.
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.child.try_wait().ok().flatten().is_none() {
                if Instant::now() >= deadline {
                    unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        let status = self.child.wait().unwrap();
        Self::record(
            &self.ledger,
            &json!({"pid": self.child.id(), "reaped": true, "exit_code": status.code()}),
        );
    }
}

struct Lab {
    directory: Directory,
    config: PathBuf,
    native: PathBuf,
    socket: PathBuf,
    state: PathBuf,
    capture: PathBuf,
}

impl Lab {
    fn new(adapter: &str, native: Value, provider_ms: u64, configured_args: &[&str]) -> Self {
        let directory = Directory::new();
        let config = directory.0.join("aw.json");
        let native_path = directory.0.join("native.json");
        let socket = directory.0.join("aw.sock");
        let state = directory.0.join("state");
        let capture = directory.0.join("captured.json");
        let agent = directory.0.join("agent.sh");
        fs::write(
            &agent,
            r#"#!/bin/sh
set -eu
if [ -n "${OPENCLAW_CONFIG_PATH:-}" ]; then
  cat "$OPENCLAW_CONFIG_PATH" > "$AW_TEST_CAPTURE"
  python3 - <<'PY'
import json, os
from pathlib import Path
env = {name: os.environ.get(name) for name in ('OPENCLAW_HOME', 'OPENCLAW_STATE_DIR', 'OPENCLAW_CONFIG_PATH')}
Path(os.environ['AW_TEST_CAPTURE'] + '.env').write_text(json.dumps(env))
profile = Path(os.environ['OPENCLAW_STATE_DIR'])
with (profile / 'session-marker').open('a') as f:
    f.write('session\n')
config = json.loads(Path(os.environ['OPENCLAW_CONFIG_PATH']).read_text())
hooks = config['plugins']['entries']['aw-native-hooks']['config']['hooks']
Path(os.environ['AW_READY_FILE']).write_text(json.dumps({
    'version': 1, 'adapter': 'openclaw', 'pid': os.getppid(),
    'token': os.environ['AW_READY_TOKEN'], 'hooks': sum(map(len, hooks.values()))}))
PY
  sleep 0.15
elif [ -n "${HERMES_HOME:-}" ]; then
  cat "$HERMES_HOME/config.yaml" > "$AW_TEST_CAPTURE"
else
  printf '{}' > "$AW_TEST_CAPTURE"
fi
"#,
        )
        .unwrap();
        let mut argv = vec!["/bin/sh".to_owned(), agent.to_string_lossy().into_owned()];
        argv.extend(configured_args.iter().map(|arg| (*arg).to_owned()));
        let value = json!({
            "apiVersion":"aw/v1alpha1", "kind":"AWConfiguration",
            "metadata":{"name":"launcher-regression"},
            "spec": {
                "daemon":{"startup":"external","endpoint":"auto","state_dir":"auto"},
                "execution":{"guarantee":"native_hook","default_event_budget_ms":5000},
                "audit":{"enabled":true,"payload":"metadata_only"},
                "agents":{"test":{"adapter":adapter,"argv":argv}},
                "providers":{"callback": {
                    "protocol":"native-hook/v1alpha1",
                    "transport":{"type":"stdio","location":"agent","argv":["/bin/true"]},
                    "timeout_ms":provider_ms,"max_output_bytes":4096,"config":{}
                }},
                "events":{"tool.before":{"enabled":true,"required":true,
                    "steps":[{"id":"before","provider":"callback","native":{}}]}}
            }
        });
        fs::write(&config, serde_json::to_vec(&value).unwrap()).unwrap();
        fs::write(&native_path, serde_json::to_vec(&native).unwrap()).unwrap();
        Self {
            directory,
            config,
            native: native_path,
            socket,
            state,
            capture,
        }
    }

    fn run(&self, extra: &[&str]) -> (ExitStatus, String) {
        self.run_with_safe_mode(extra, None)
    }

    fn run_with_safe_mode(&self, extra: &[&str], safe_mode: Option<&str>) -> (ExitStatus, String) {
        self.run_with_profile(extra, safe_mode, None)
    }

    fn run_with_profile(
        &self,
        extra: &[&str],
        safe_mode: Option<&str>,
        profile: Option<&Path>,
    ) -> (ExitStatus, String) {
        let original = fs::read(&self.native).unwrap();
        let mut command = Command::new(AW);
        command
            .args(["run", "test", "--config"])
            .arg(&self.config)
            .arg("--native-config")
            .arg(&self.native)
            .arg("--state-dir")
            .arg(&self.state)
            .arg("--socket")
            .arg(&self.socket);
        if let Some(profile) = profile {
            command.arg("--native-state-dir").arg(profile);
        }
        command
            .arg("--")
            .args(extra)
            .env("AW_TEST_CAPTURE", &self.capture)
            .env("OPENCLAW_HOME", self.directory.0.join("original-home"))
            .env_remove("OPENCLAW_CONTAINER")
            .env_remove("OPENCLAW_CONFIG_PATH")
            .env_remove("HERMES_HOME");
        if let Some(value) = safe_mode {
            command.env("HERMES_SAFE_MODE", value);
        } else {
            command.env_remove("HERMES_SAFE_MODE");
        }
        let mut process = OwnedProcess::spawn(&mut command, &self.directory.0, "launch");
        let status = process.wait();
        assert_eq!(fs::read(&self.native).unwrap(), original);
        if self.state.exists() {
            assert_eq!(fs::read_dir(&self.state).unwrap().count(), 0);
        }
        let stderr = fs::read_to_string(self.directory.0.join("launch.stderr")).unwrap();
        (status, stderr)
    }

    fn capture(&self) -> Value {
        self.capture_with_profile(None)
    }

    fn capture_with_profile(&self, profile: Option<&Path>) -> Value {
        let (status, stderr) = self.with_daemon(profile);
        assert!(status.success(), "launcher failed: {stderr}");
        serde_json::from_slice(&fs::read(&self.capture).unwrap()).unwrap()
    }

    fn with_daemon(&self, profile: Option<&Path>) -> (ExitStatus, String) {
        let mut command = Command::new(AW);
        command
            .args(["serve", "--config"])
            .arg(&self.config)
            .arg("--socket")
            .arg(&self.socket)
            .args(["--idle-timeout", "10"]);
        let mut daemon = OwnedProcess::spawn(&mut command, &self.directory.0, "daemon");
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.socket.exists() {
            assert!(Instant::now() < deadline, "daemon readiness deadline");
            assert!(daemon.child.try_wait().unwrap().is_none(), "daemon exited");
            thread::sleep(Duration::from_millis(10));
        }
        let (status, stderr) = self.run_with_profile(&[], None, profile);
        daemon.stop();
        assert!(!self.socket.exists());
        (status, stderr)
    }

    fn rejects(&self, extra: &[&str], reason: &str) {
        let (status, stderr) = self.run(extra);
        assert_eq!(status.code(), Some(125), "{stderr}");
        assert!(stderr.contains(reason), "wrong rejection: {stderr}");
        assert!(!self.capture.exists(), "fake Agent was launched");
        assert!(!self.socket.exists(), "rejected launch started a daemon");
    }
}

#[test]
fn hermes_safe_mode_aliases_cannot_silently_disable_hooks() {
    for value in ["1", "true", "YES", " on ", "false", "0"] {
        let lab = Lab::new("hermes", json!({}), 1000, &[]);
        let (status, stderr) = lab.run_with_safe_mode(&[], Some(value));
        assert!(!status.success());
        assert_eq!(
            stderr.contains("HERMES_SAFE_MODE"),
            !matches!(value, "false" | "0"),
            "{value}: {stderr}"
        );
        assert!(!lab.capture.exists());
    }
}

#[test]
fn qwenpaw_rejects_entrypoints_that_skip_hook_plugin_loading() {
    for args in [
        vec!["qwenpaw"],
        vec!["qwenpaw", "tui"],
        vec!["qwenpaw", "acp"],
        vec!["qwenpaw", "/tmp/project"],
        vec!["qwenpaw", "app"],
    ] {
        let lab = Lab::new("qwenpaw", json!({}), 1000, &[]);
        let mut config: Value = serde_json::from_slice(&fs::read(&lab.config).unwrap()).unwrap();
        config["spec"]["agents"]["test"]["argv"] = json!(args);
        fs::write(&lab.config, serde_json::to_vec(&config).unwrap()).unwrap();
        let mut command = Command::new(AW);
        command.args(["plan", "test", "--config"]).arg(&lab.config);
        let mut process = OwnedProcess::spawn(&mut command, &lab.directory.0, "qwen-entry");
        assert_eq!(process.wait().success(), args.get(1) == Some(&"app"));
        if args.get(1) != Some(&"app") {
            assert!(
                fs::read_to_string(lab.directory.0.join("qwen-entry.stderr"))
                    .unwrap()
                    .contains("ACP/TUI")
            );
        }
    }
}

#[test]
fn openclaw_missing_allow_stays_unrestricted() {
    let value = Lab::new("openclaw", json!({}), 1000, &[]).capture();
    assert!(
        value["plugins"].get("allow").is_none(),
        "generated plugins: {}",
        value["plugins"]
    );
    assert_eq!(
        value["plugins"]["entries"]["aw-native-hooks"]["enabled"],
        true
    );
}

#[test]
fn openclaw_empty_allow_stays_unrestricted() {
    let value = Lab::new("openclaw", json!({"plugins":{"allow":[]}}), 1000, &[]).capture();
    assert_eq!(value["plugins"]["allow"], json!([]));
    assert_eq!(
        value["plugins"]["entries"]["aw-native-hooks"]["enabled"],
        true
    );
}

#[test]
fn openclaw_restrictive_allow_appends_bridge_once() {
    for allow in [json!(["existing"]), json!(["existing", "aw-native-hooks"])] {
        let value = Lab::new("openclaw", json!({"plugins":{"allow":allow}}), 1000, &[]).capture();
        assert_eq!(
            value["plugins"]["allow"],
            json!(["existing", "aw-native-hooks"])
        );
    }
}

#[test]
fn hermes_default_callback_limit_includes_rounding_and_cleanup() {
    let value = Lab::new("hermes", json!({}), 28_000, &[]).capture();
    assert_eq!(value["hooks"]["pre_tool_call"][0]["timeout"], 30);
    Lab::new("hermes", json!({}), 28_001, &[]).rejects(&[], "hook_callback_timeout");
}

#[test]
fn hermes_explicit_callback_limit_controls_provider_budget() {
    let value = Lab::new(
        "hermes",
        json!({"plugins":{"hook_callback_timeout":32}}),
        30_000,
        &[],
    )
    .capture();
    assert_eq!(value["plugins"]["hook_callback_timeout"], 32);
    assert_eq!(value["hooks"]["pre_tool_call"][0]["timeout"], 32);
    let native = json!({"plugins":{"hook_callback_timeout":7.5}});
    let value = Lab::new("hermes", native.clone(), 5000, &[]).capture();
    assert_eq!(value["hooks"]["pre_tool_call"][0]["timeout"], 7);
    Lab::new("hermes", native, 5001, &[]).rejects(&[], "hook_callback_timeout");
}

#[test]
fn hermes_zero_callback_limit_keeps_native_unlimited_setting() {
    let value = Lab::new(
        "hermes",
        json!({"plugins":{"hook_callback_timeout":0}}),
        298_000,
        &[],
    )
    .capture();
    assert_eq!(value["plugins"]["hook_callback_timeout"], 0);
    assert_eq!(value["hooks"]["pre_tool_call"][0]["timeout"], 300);
}

#[test]
fn qoder_setting_sources_rejected_in_configured_argv() {
    for arguments in [
        vec!["--setting-sources", "user"],
        vec!["--setting-sources=user"],
    ] {
        Lab::new("qoder", json!({}), 1000, &arguments).rejects(&[], "--setting-sources");
    }
}

#[test]
fn qoder_setting_sources_rejected_in_forwarded_arguments() {
    for arguments in [
        vec!["--setting-sources", "user"],
        vec!["--setting-sources=user"],
    ] {
        Lab::new("qoder", json!({}), 1000, &[]).rejects(&arguments, "--setting-sources");
    }
}

#[test]
fn qoder_settings_rejected_in_configured_argv() {
    for arguments in [vec!["--settings", "{}"], vec!["--settings={}"]] {
        Lab::new("qoder", json!({}), 1000, &arguments).rejects(&[], "--native-config");
    }
}

#[test]
fn qoder_settings_rejected_in_forwarded_arguments() {
    for arguments in [vec!["--settings", "{}"], vec!["--settings={}"]] {
        Lab::new("qoder", json!({}), 1000, &[]).rejects(&arguments, "--native-config");
    }
}

#[test]
fn openclaw_state_and_native_home_survive_repeated_launches() {
    let lab = Lab::new(
        "openclaw",
        json!({"plugins":{"load":{"paths":["./existing-plugin"]}}}),
        1000,
        &[],
    );
    let profile = lab.directory.0.join("native-profile");
    fs::create_dir(&profile).unwrap();
    let auth = profile.join("auth.json");
    fs::write(&auth, "native-auth-sentinel").unwrap();
    for _ in 0..2 {
        let value = lab.capture_with_profile(Some(&profile));
        assert_eq!(value["plugins"]["load"]["paths"][0], "./existing-plugin");
        let env: Value =
            serde_json::from_slice(&fs::read(format!("{}.env", lab.capture.display())).unwrap())
                .unwrap();
        assert_eq!(
            env["OPENCLAW_STATE_DIR"],
            profile.to_string_lossy().as_ref()
        );
        assert_eq!(
            env["OPENCLAW_HOME"],
            lab.directory
                .0
                .join("original-home")
                .to_string_lossy()
                .as_ref()
        );
        assert_eq!(fs::read_to_string(&auth).unwrap(), "native-auth-sentinel");
    }
    assert_eq!(
        fs::read_to_string(profile.join("session-marker")).unwrap(),
        "session\nsession\n"
    );
}

#[test]
fn openclaw_rejects_disabled_plugins_and_unresolved_includes() {
    for value in [
        json!({"plugins":{"enabled":false}}),
        json!({"plugins":{"deny":[" aw-native-hooks "]}}),
    ] {
        Lab::new("openclaw", value, 1000, &[]).rejects(&[], "disables the AW plugin");
    }
    for value in [
        json!({"$include":"./base.json"}),
        json!({"agents":{"defaults":{"$include":"./agent.json"}}}),
    ] {
        Lab::new("openclaw", value, 1000, &[]).rejects(&[], "$include");
    }
}

#[test]
fn openclaw_rejects_native_ownership_overrides() {
    for flag in [
        "--profile",
        "--profile=dev",
        "--dev",
        "--reset",
        "--container=test",
        "--force",
    ] {
        Lab::new("openclaw", json!({}), 1000, &[flag]).rejects(&[], "conflicts with AW-owned");
        Lab::new("openclaw", json!({}), 1000, &[]).rejects(&[flag], "conflicts with AW-owned");
    }
}

#[test]
fn native_state_override_is_not_silently_ignored() {
    for adapter in ["qoder", "hermes", "qwenpaw"] {
        let lab = Lab::new(adapter, json!({}), 1000, &[]);
        let (status, stderr) =
            lab.run_with_profile(&[], None, Some(&lab.directory.0.join("profile")));
        assert_eq!(status.code(), Some(125));
        assert!(stderr.contains("supported only for OpenClaw"), "{stderr}");
        assert!(!lab.capture.exists());
    }
}

#[test]
fn openclaw_missing_or_invalid_registration_cannot_report_success() {
    for replacement in [
        "raise SystemExit(0)",
        "os.environ['AW_READY_TOKEN'] = 'stale-token'",
        "os.getppid = lambda: 1",
    ] {
        let lab = Lab::new("openclaw", json!({}), 1000, &[]);
        let script = lab.directory.0.join("agent.sh");
        let source = fs::read_to_string(&script).unwrap();
        fs::write(
            &script,
            source.replace(
                "hooks = config[",
                &format!("{replacement}\nhooks = config["),
            ),
        )
        .unwrap();
        let (status, stderr) = lab.with_daemon(None);
        assert_eq!(status.code(), Some(125), "{stderr}");
        assert!(stderr.contains("registration"), "{stderr}");
    }
}

#[test]
fn openclaw_profile_lock_prevents_concurrent_aw_ownership() {
    use std::os::fd::AsRawFd;
    let lab = Lab::new("openclaw", json!({}), 1000, &[]);
    let profile = lab.directory.0.join("profile");
    fs::create_dir(&profile).unwrap();
    let lock = File::create(profile.join(".aw-launch.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let (status, stderr) = lab.run_with_profile(&[], None, Some(&profile));
    assert_eq!(status.code(), Some(125));
    assert!(stderr.contains("already in use"), "{stderr}");
    assert!(!lab.socket.exists());
    drop(lock);
    lab.capture_with_profile(Some(&profile));
}
