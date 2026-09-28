//! Real-socket lease and control-deadline tests; no model or Agent installation.
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

const AW: &str = env!("CARGO_BIN_EXE_aw");
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/lifecycle-tests");
        fs::create_dir_all(&root).unwrap();
        let path = root.canonicalize().unwrap().join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct OwnedProcess(Child);

impl OwnedProcess {
    fn wait(&mut self, duration: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + duration;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "owned process did not exit before deadline"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn request(socket: &Path, operation: &str, pid: u32) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let body = serde_json::to_vec(&json!({"op": operation, "pid": pid})).unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
    let mut size = [0; 4];
    stream.read_exact(&mut size).unwrap();
    let size = u32::from_be_bytes(size) as usize;
    assert!(size <= 4096);
    let mut body = vec![0; size];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

struct Lab {
    process: OwnedProcess,
    socket: PathBuf,
    _directory: Directory,
}

impl Lab {
    fn new() -> Self {
        let directory = Directory::new();
        let socket = directory.0.join("aw.sock");
        let config = directory.0.join("aw.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "apiVersion":"aw/v1alpha1", "kind":"AWConfiguration",
                "metadata":{"name":"lease-test"},
                "spec":{
                    "daemon":{"startup":"external","endpoint":"auto","state_dir":"auto"},
                    "execution":{"guarantee":"native_hook","default_event_budget_ms":5000},
                    "audit":{"enabled":true,"payload":"metadata_only"},
                    "agents":{"qoder":{"adapter":"qoder","argv":["/bin/true"]}},
                    "providers":{}, "events":{"tool.before":{"enabled":true,"steps":[]}}
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let mut command = Command::new(AW);
        command
            .arg("serve")
            .arg("--config")
            .arg(&config)
            .arg("--socket")
            .arg(&socket)
            .args(["--idle-timeout", "1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut process = OwnedProcess(command.spawn().unwrap());
        eprintln!(
            "owned {:?}; cwd={:?}; pid={}; ports=none; lifetime<8s; stop=kill -TERM {}",
            command,
            std::env::current_dir().unwrap(),
            process.0.id(),
            process.0.id()
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "daemon readiness deadline");
            assert!(process.0.try_wait().unwrap().is_none(), "daemon exited");
            thread::sleep(Duration::from_millis(10));
        }
        Self {
            process,
            socket,
            _directory: directory,
        }
    }

    fn status(&self) -> Value {
        let reply = request(&self.socket, "status", 0);
        assert_eq!(reply["code"], 0);
        let bytes: Vec<u8> = serde_json::from_value(reply["stdout"].clone()).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
}

#[test]
fn live_lease_survives_idle_then_release_expires() {
    let mut lab = Lab::new();
    assert_eq!(request(&lab.socket, "lease", std::process::id())["code"], 0);
    thread::sleep(Duration::from_millis(1200));
    assert!(lab.process.0.try_wait().unwrap().is_none());
    assert_eq!(lab.status()["leases"], 1);
    assert_eq!(
        request(&lab.socket, "release", std::process::id())["code"],
        0
    );
    assert!(lab.process.wait(Duration::from_secs(3)).success());
    assert!(!lab.socket.exists());
}

#[test]
fn crashed_owner_is_pruned_without_release() {
    let mut lab = Lab::new();
    let mut owner = OwnedProcess(Command::new("/bin/sleep").arg("10").spawn().unwrap());
    eprintln!(
        "owned /bin/sleep 10; pid={}; ports=none; lifetime<4s; stop=kill -TERM {}",
        owner.0.id(),
        owner.0.id()
    );
    assert_eq!(request(&lab.socket, "lease", owner.0.id())["code"], 0);
    assert_eq!(lab.status()["leases"], 1);
    owner.0.kill().unwrap();
    owner.wait(Duration::from_secs(1));
    assert!(lab.process.wait(Duration::from_secs(3)).success());
    assert!(!lab.socket.exists());
}

#[test]
fn control_request_has_short_deadline_on_silent_listener() {
    let directory = Directory::new();
    let socket = directory.0.join("silent.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    assert_control_deadline(&socket);
}

#[test]
fn control_request_has_short_deadline_on_full_backlog() {
    let directory = Directory::new();
    let socket = directory.0.join("full.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    // Linux accepts one queued connection with backlog zero; leave it unaccepted.
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
    let _queued = UnixStream::connect(&socket).unwrap();
    assert_control_deadline(&socket);
}

fn assert_control_deadline(socket: &Path) {
    let started = Instant::now();
    let mut client = OwnedProcess(
        Command::new(AW)
            .args(["status", "--socket"])
            .arg(socket)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    eprintln!(
        "owned aw status --socket {:?}; pid={}; ports=none; lifetime<3s; stop=kill -TERM {}",
        socket,
        client.0.id(),
        client.0.id()
    );
    assert_eq!(client.wait(Duration::from_secs(3)).code(), Some(125));
    assert!(started.elapsed() < Duration::from_secs(2));
}
