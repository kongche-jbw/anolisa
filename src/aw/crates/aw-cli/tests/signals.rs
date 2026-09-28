//! Exercise launcher process-group ownership and foreground terminal handoff.
#![cfg(target_os = "linux")]

use serde_json::json;
use std::{
    fs,
    io::Write,
    os::{
        fd::FromRawFd,
        unix::{fs::PermissionsExt, process::CommandExt},
    },
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

const AW: &str = env!("CARGO_BIN_EXE_aw");
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Lab {
    dir: PathBuf,
    server: Child,
    processes: Vec<serde_json::Value>,
}
impl Lab {
    fn new(argv: &[&str]) -> Self {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/signal-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let dir = dir.canonicalize().unwrap();
        let config = json!({"apiVersion":"aw/v1alpha1","kind":"AWConfiguration","metadata":{"name":"signals"},"spec":{
            "daemon":{"startup":"external","endpoint":"auto","state_dir":"auto"},
            "execution":{"guarantee":"native_hook","default_event_budget_ms":5000},
            "audit":{"enabled":true,"payload":"metadata_only"},
            "agents":{"qoder":{"adapter":"qoder","argv":argv}},
            "providers":{"command":{"protocol":"native-hook/v1alpha1","transport":{"type":"stdio","location":"agent","argv":["/bin/true"]},"timeout_ms":1000,"max_output_bytes":4096,"config":{}}},
            "events":{"tool.before":{"enabled":true,"required":true,"steps":[{"id":"one","provider":"command","native":{}}]}}
        }});
        fs::write(
            dir.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let mut command = Command::new(AW);
        command
            .args(["serve", "--config"])
            .arg(dir.join("config.json"))
            .arg("--socket")
            .arg(dir.join("aw.sock"))
            .args(["--idle-timeout", "30"])
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(dir.join("daemon.log")).unwrap())
            .process_group(0);
        let server = command.spawn().unwrap();
        let mut lab = Self {
            dir,
            server,
            processes: Vec::new(),
        };
        lab.record(&command, lab.server.id(), "daemon.log");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !lab.dir.join("aw.sock").exists() {
            assert!(Instant::now() < deadline, "daemon readiness timeout");
            assert!(lab.server.try_wait().unwrap().is_none(), "daemon exited");
            thread::sleep(Duration::from_millis(10));
        }
        lab
    }
    fn record(&mut self, command: &Command, pid: u32, log: &str) {
        self.processes.push(
            json!({"command":format!("{command:?}"),"cwd":self.dir,"pid":pid,
            "pgid":pid,"ports":[],"log":self.dir.join(log),"deadline_seconds":30,
            "stop":format!("kill -TERM -- -{pid}; wait {pid}")}),
        );
        fs::write(
            self.dir.join("processes.json"),
            serde_json::to_vec_pretty(&self.processes).unwrap(),
        )
        .unwrap();
    }
    fn command(&self) -> Command {
        let mut command = Command::new(AW);
        command
            .args(["run", "qoder", "--config"])
            .arg(self.dir.join("config.json"))
            .arg("--socket")
            .arg(self.dir.join("aw.sock"))
            .arg("--state-dir")
            .arg(self.dir.join("state"))
            .current_dir(&self.dir);
        command
    }
    fn spawn(&mut self) -> Running {
        let mut command = self.command();
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(self.dir.join("agent.log")).unwrap())
            .process_group(0);
        let child = command.spawn().unwrap();
        self.record(&command, child.id(), "agent.log");
        Running(child)
    }
    fn wait_file(&self, name: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(value) = fs::read_to_string(self.dir.join(name)) {
                if !value.is_empty() {
                    return value;
                }
            }
            assert!(Instant::now() < deadline, "missing {name}");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn assert_no_bindings(&self) {
        assert_eq!(fs::read_dir(self.dir.join("state")).unwrap().count(), 0);
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        unsafe { libc::kill(-(self.server.id() as i32), libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.server.try_wait().ok().flatten().is_none() {
            if Instant::now() >= deadline {
                let _ = self.server.kill();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.server.wait();
        assert!(!self.dir.join("aw.sock").exists(), "daemon socket leaked");
        fs::remove_dir_all(&self.dir).unwrap();
    }
}

struct Running(Child);
impl Running {
    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "launcher exit timeout");
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_some() {
            return;
        }
        unsafe { libc::kill(-(self.0.id() as i32), libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.0.try_wait().ok().flatten().is_none() {
            if Instant::now() >= deadline {
                let _ = self.0.kill();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.0.wait();
    }
}

// Capture the kernel start time before cleanup so test failure cleanup never
// signals a reused PID. A live group member also reserves its process-group ID.
struct Descendant {
    pid: u32,
    start: String,
    group: i32,
}
impl Descendant {
    fn observe(pid: &str) -> Self {
        let pid = pid.trim().parse().unwrap();
        let fields = Self::fields(pid).unwrap();
        Self {
            pid,
            start: fields[19].clone(),
            group: fields[2].parse().unwrap(),
        }
    }
    fn fields(pid: u32) -> Option<Vec<String>> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        Some(
            stat.rsplit_once(')')?
                .1
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        )
    }
    fn running(&self) -> bool {
        Self::fields(self.pid)
            .is_some_and(|fields| fields[19] == self.start && fields[0] != "Z" && fields[0] != "X")
    }
}
impl Drop for Descendant {
    fn drop(&mut self) {
        if self.running() {
            unsafe { libc::kill(-self.group, libc::SIGKILL) };
        }
    }
}

#[test]
fn signal_reaches_wrapper_group_and_bindings_outlive_descendants() {
    let mut lab = Lab::new(&["/bin/sh", "-c", "sleep 30 & echo $! > child.pid; wait"]);
    let mut run = lab.spawn();
    let descendant = Descendant::observe(&lab.wait_file("child.pid"));
    assert_eq!(unsafe { libc::kill(run.0.id() as i32, libc::SIGTERM) }, 0);
    assert_eq!(run.wait().code(), Some(143));
    assert!(
        !descendant.running(),
        "wrapper descendant survived launcher exit"
    );
    lab.assert_no_bindings();
}

#[test]
fn normal_exit_preserves_status_and_cleans_term_ignoring_descendant() {
    let mut lab = Lab::new(&["/bin/sh", "-c", "python3 -c 'import os,signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); open(\"child.pid\",\"w\").write(str(os.getpid())); time.sleep(30)' & sleep 0.2; exit 17"]);
    let mut run = lab.spawn();
    let descendant = Descendant::observe(&lab.wait_file("child.pid"));
    assert_eq!(run.wait().code(), Some(17));
    assert!(
        !descendant.running(),
        "ignored TERM prevented descendant cleanup"
    );
    lab.assert_no_bindings();
}

#[test]
fn ignored_launcher_signal_has_bounded_kill_fallback_without_a_tty() {
    let mut lab = Lab::new(&["python3", "-c", "import os,signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); open('child.pid','w').write(str(os.getpid())); time.sleep(30)"]);
    let mut run = lab.spawn();
    let descendant = Descendant::observe(&lab.wait_file("child.pid"));
    assert_eq!(unsafe { libc::kill(run.0.id() as i32, libc::SIGTERM) }, 0);
    assert_eq!(run.wait().code(), Some(137));
    assert!(!descendant.running());
    lab.assert_no_bindings();
}

#[test]
fn interactive_agent_owns_terminal_and_ctrl_c_then_restores_foreground() {
    let mut lab = Lab::new(&["python3", "-c", "import os,signal,time; from pathlib import Path; signal.signal(signal.SIGINT,lambda *_: os._exit(23)); Path('child.pid').write_text(str(os.getpid())); Path('foreground').write_text(str(os.tcgetpgrp(0)==os.getpgrp())); line=input(); Path('input').write_text(line); time.sleep(30)"]);
    let mut master = 0;
    let mut slave = 0;
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    let mut master = unsafe { fs::File::from_raw_fd(master) };
    let slave = unsafe { fs::File::from_raw_fd(slave) };
    let driver = "import os,subprocess,sys; from pathlib import Path; child=subprocess.Popen(sys.argv[1:]); Path('launcher.pid').write_text(str(child.pid)); code=child.wait(timeout=15); Path('restored').write_text(str(os.tcgetpgrp(0)==os.getpgrp())); sys.exit(code)";
    let native = lab.command();
    let mut command = Command::new("python3");
    command
        .args(["-c", driver])
        .arg(AW)
        .args(native.get_args())
        .current_dir(&lab.dir)
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    lab.record(&command, child.id(), "pty");
    let mut run = Running(child);
    let descendant = Descendant::observe(&lab.wait_file("child.pid"));
    assert_eq!(lab.wait_file("foreground"), "True");
    master.write_all(b"native-input-sentinel\n").unwrap();
    assert_eq!(lab.wait_file("input"), "native-input-sentinel");
    master.write_all(&[3]).unwrap();
    assert_eq!(run.wait().code(), Some(23));
    assert_eq!(lab.wait_file("restored"), "True");
    assert!(!descendant.running());
    lab.assert_no_bindings();
}
