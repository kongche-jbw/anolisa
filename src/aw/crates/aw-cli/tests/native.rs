//! Exercise real Unix sockets and subprocesses without installing an Agent.
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};
const AW: &str = env!("CARGO_BIN_EXE_aw");
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Lab {
    dir: PathBuf,
    socket: PathBuf,
    server: Child,
}
impl Lab {
    fn new(command: Vec<&str>, timeout: u64, limit: u64) -> Self {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/native-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.join("aw.sock");
        let config = dir.join("aw.json");
        let value = json!({"apiVersion":"aw/v1alpha1","kind":"AWConfiguration","metadata":{"name":"native-test"},"spec":{
            "daemon":{"startup":"external","endpoint":"auto","state_dir":"auto"},"execution":{"guarantee":"native_hook","default_event_budget_ms":5000},"audit":{"enabled":true,"payload":"metadata_only"},
            "agents":{"qoder":{"adapter":"qoder","argv":["/bin/true"]}},
            "providers":{"command":{"protocol":"native-hook/v1alpha1","transport":{"type":"stdio","location":"agent","argv":command},"timeout_ms":timeout,"max_output_bytes":limit,"config":{}}},
            "events":{"tool.before":{"enabled":true,"required":true,"steps":[{"id":"one","provider":"command","native":{}}]}}}});
        fs::write(&config, serde_json::to_vec(&value).unwrap()).unwrap();
        let mut server = Command::new(AW)
            .arg("serve")
            .arg("--config")
            .arg(config)
            .arg("--socket")
            .arg(&socket)
            .args(["--idle-timeout", "30"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "daemon readiness timeout");
            assert!(server.try_wait().unwrap().is_none(), "daemon exited");
            thread::sleep(Duration::from_millis(10));
        }
        Self {
            dir,
            socket,
            server,
        }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(AW);
        c.args(["hook", "--socket"])
            .arg(&self.socket)
            .args([
                "--agent",
                "qoder",
                "--event",
                "tool.before",
                "--provider",
                "command",
            ])
            .current_dir(&self.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }
    fn call(&self, input: &[u8]) -> std::process::Output {
        let mut c = self.command().spawn().unwrap();
        c.stdin.take().unwrap().write_all(input).unwrap();
        c.wait_with_output().unwrap()
    }
    fn audit(&self) -> Vec<Value> {
        fs::read_to_string(self.dir.join("audit.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = Command::new(AW)
            .args(["stop", "--socket"])
            .arg(&self.socket)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.server.try_wait().ok().flatten().is_none() {
            if Instant::now() > deadline {
                let _ = self.server.kill();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.server.wait();
        assert!(!self.socket.exists());
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
#[test]
fn arbitrary_script_preserves_stdin_stdout_stderr_exit_and_caller_environment() {
    let lab = Lab::new(
        vec![
            "/bin/sh",
            "-c",
            "cat; printf '%s' \"$NATIVE_TEST_ENV\" >&2; exit 7",
        ],
        1000,
        4096,
    );
    let input=b"{\"command\":\"$(touch should-not-exist); 'quoted'\",\"unicode\":\"\xe5\xae\x89\xe5\x85\xa8\"}";
    let mut child = lab
        .command()
        .env("NATIVE_TEST_ENV", "native-env-sentinel")
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(out.stdout, input);
    assert_eq!(out.stderr, b"native-env-sentinel");
    assert!(!lab.dir.join("should-not-exist").exists());
    let audit = lab.audit();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0]["exit_code"], 7);
    assert!(!serde_json::to_string(&audit).unwrap().contains("sentinel"));
}
#[test]
fn command_need_not_read_stdin_and_literal_arguments_are_not_shell_expanded() {
    let lab = Lab::new(
        vec!["/usr/bin/printf", "%s", "literal $(no-command) 'quoted'"],
        1000,
        4096,
    );
    let out = lab.call(&vec![b'x'; 65536]);
    assert!(out.status.success(), "{:?}", out.stderr);
    assert_eq!(out.stdout, b"literal $(no-command) 'quoted'");
}
#[test]
fn callbacks_overlap_without_an_aw_serial_queue() {
    let lab = Lab::new(
        vec!["/bin/sh", "-c", "cat >/dev/null; sleep 0.25; printf done"],
        2000,
        4096,
    );
    let start = Instant::now();
    let mut children = Vec::new();
    for _ in 0..4 {
        let mut c = lab.command().spawn().unwrap();
        drop(c.stdin.take());
        children.push(c);
    }
    for c in children {
        assert!(c.wait_with_output().unwrap().status.success());
    }
    let audit = lab.audit();
    assert_eq!(audit.len(), 4);
    assert!(
        start.elapsed() < Duration::from_millis(900),
        "callbacks unexpectedly serialized"
    );
}
#[test]
fn timeout_reaps_owned_group_and_audits_failure() {
    let lab = Lab::new(
        vec![
            "/bin/sh",
            "-c",
            "echo $$ > leader; sleep 30 & echo $! > child; wait",
        ],
        120,
        4096,
    );
    let out = lab.call(b"{}");
    assert_eq!(out.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&out.stderr).contains("timeout"));
    for name in ["leader", "child"] {
        let pid = fs::read_to_string(lab.dir.join(name)).unwrap();
        let path = format!("/proc/{}/stat", pid.trim());
        if let Ok(stat) = fs::read_to_string(path) {
            assert!(
                stat.rsplit_once(')')
                    .unwrap()
                    .1
                    .trim_start()
                    .starts_with('Z'),
                "owned child still running"
            );
        }
    }
    assert_eq!(lab.audit()[0]["exit_code"], 125);
}
#[test]
fn output_flood_is_bounded_and_wrong_binding_does_not_execute() {
    let lab = Lab::new(
        vec!["/bin/sh", "-c", "printf marker > invoked; yes flood"],
        2000,
        128,
    );
    let out = lab.call(b"{}");
    assert_eq!(out.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&out.stderr).contains("output_limit"));
    fs::remove_file(lab.dir.join("invoked")).unwrap();
    let out = Command::new(AW)
        .args(["hook", "--socket"])
        .arg(&lab.socket)
        .args([
            "--agent",
            "qoder",
            "--event",
            "tool.after",
            "--provider",
            "command",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(125));
    assert!(!lab.dir.join("invoked").exists());
}

#[test]
fn provider_signal_preserves_shell_exit_convention() {
    let lab = Lab::new(vec!["/bin/sh", "-c", "kill -TERM $$"], 1000, 4096);
    let out = lab.call(b"{}");
    assert_eq!(out.status.code(), Some(143));
    assert_eq!(lab.audit()[0]["exit_code"], 143);
}
