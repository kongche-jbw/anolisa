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
            "#!/bin/sh\nset -eu\nif [ -n \"${OPENCLAW_CONFIG_PATH:-}\" ]; then\n  cat \"$OPENCLAW_CONFIG_PATH\" > \"$AW_TEST_CAPTURE\"\nelif [ -n \"${HERMES_HOME:-}\" ]; then\n  cat \"$HERMES_HOME/config.yaml\" > \"$AW_TEST_CAPTURE\"\nelse\n  printf '{}' > \"$AW_TEST_CAPTURE\"\nfi\n",
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
            .arg(&self.socket)
            .arg("--")
            .args(extra)
            .env("AW_TEST_CAPTURE", &self.capture)
            .env_remove("OPENCLAW_CONFIG_PATH")
            .env_remove("HERMES_HOME");
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
        let (status, stderr) = self.run(&[]);
        assert!(status.success(), "launcher failed: {stderr}");
        let value = serde_json::from_slice(&fs::read(&self.capture).unwrap()).unwrap();
        daemon.stop();
        assert!(!self.socket.exists());
        value
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
