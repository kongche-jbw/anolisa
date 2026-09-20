//! Track only instance-owned processes and restore the caller's native terminal.

use super::rpc;
use nix::libc;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::PathBuf,
    process::Child,
    thread,
    time::{Duration, Instant},
};
use wait_timeout::ChildExt;

pub(crate) fn process_identity(pid: u32) -> Result<u64, String> {
    process_stat(pid).map(|value| value.2)
}

fn process_stat(pid: u32) -> Result<(u32, u32, u64), String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    let (_, fields) = stat.rsplit_once(')').ok_or("invalid process stat")?;
    let fields: Vec<_> = fields.split_whitespace().collect();
    if fields.len() < 20 || matches!(fields[0], "Z" | "X") {
        return Err("process no longer running".into());
    }
    Ok((
        fields[1].parse().map_err(|_| "invalid process parent")?,
        fields[3].parse().map_err(|_| "invalid process session")?,
        fields[19].parse().map_err(|_| "invalid process identity")?,
    ))
}

pub(super) struct OwnedSession {
    pub server: Child,
    pub client: Option<Child>,
    pub socket: PathBuf,
    pub workspace: Option<String>,
    tracked: BTreeMap<u32, u64>,
    server_start_ticks: Option<u64>,
    closed: bool,
}

impl OwnedSession {
    pub fn new(server: Child, socket: PathBuf) -> Self {
        let server_start_ticks = process_identity(server.id()).ok();
        Self {
            server,
            client: None,
            socket,
            workspace: None,
            tracked: BTreeMap::new(),
            server_start_ticks,
            closed: false,
        }
    }

    pub fn register_pane(&mut self, pid: u32) -> Result<(), String> {
        let (parent, _, ticks) = process_stat(pid)?;
        if parent != self.server.id() || self.server_start_ticks != process_identity(parent).ok() {
            return Err("Herdr pane is not owned by this server".into());
        }
        self.tracked.insert(pid, ticks);
        Ok(())
    }

    pub fn check_server(&mut self) -> Result<(), String> {
        if self.server.try_wait().map_err(|e| e.to_string())?.is_some() {
            Err("Herdr server exited unexpectedly".into())
        } else {
            Ok(())
        }
    }

    pub fn client_exited(&mut self) -> Result<bool, String> {
        match self.client.as_mut() {
            Some(client) => client
                .try_wait()
                .map(|value| value.is_some())
                .map_err(|e| e.to_string()),
            None => Ok(false),
        }
    }

    pub fn observe_descendants(&mut self) -> Result<(), String> {
        let mut processes = BTreeMap::new();
        for entry in fs::read_dir("/proc").map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() {
                if let Ok(stat) = process_stat(pid) {
                    processes.insert(pid, stat);
                }
            }
        }
        let deadline = Instant::now() + Duration::from_millis(100);
        for _ in 0..64 {
            let mut changed = false;
            for (&pid, &(parent, _, ticks)) in &processes {
                if (parent == self.server.id()
                    && self.server_start_ticks.is_some()
                    && processes.get(&parent).map(|stat| stat.2) == self.server_start_ticks)
                    || self.tracked.get(&parent).is_some_and(|start| {
                        processes.get(&parent).is_some_and(|stat| stat.2 == *start)
                    })
                {
                    changed |= self.tracked.insert(pid, ticks) != Some(ticks);
                }
            }
            if !changed {
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        Err("Herdr descendant registration exceeded its bound".into())
    }

    pub fn close(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let mut errors = Vec::new();
        if let Err(error) = self.observe_descendants() {
            errors.push(error);
        }
        if let Some(workspace) = self.workspace.take() {
            // Herdr owns PTY/session teardown and reaping its pane children.
            let _ = rpc(
                &self.socket,
                "workspace.close",
                json!({"workspace_id":workspace}),
            );
        }
        if let Some(client) = self.client.as_mut() {
            if let Err(error) = stop_child(client) {
                errors.push(error);
            }
        }
        for signal in [libc::SIGTERM, libc::SIGKILL] {
            for (&pid, &ticks) in &self.tracked {
                if let Err(error) = signal_identity(pid, ticks, signal) {
                    errors.push(error);
                }
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if self
                    .tracked
                    .iter()
                    .all(|(&pid, &ticks)| process_identity(pid) != Ok(ticks))
                {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
        if let Err(error) = stop_child(&mut self.server) {
            errors.push(error);
        }
        let alive: Vec<_> = self
            .tracked
            .iter()
            .filter(|(pid, ticks)| process_identity(**pid) == Ok(**ticks))
            .map(|(pid, _)| pid.to_string())
            .collect();
        if !alive.is_empty() {
            errors.push(format!(
                "Herdr owned processes survived cleanup: {}",
                alive.join(",")
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

impl Drop for OwnedSession {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            eprintln!("AW Herdr cleanup failed: {error}");
        }
    }
}

fn signal_identity(pid: u32, ticks: u64, signal: i32) -> Result<(), String> {
    // pidfd pins the target through the identity check, avoiding PID reuse.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if fd < 0 {
        return if process_identity(pid) != Ok(ticks) {
            Ok(())
        } else {
            Err(format!(
                "open owned process {pid}: {}",
                std::io::Error::last_os_error()
            ))
        };
    }
    let matches = process_identity(pid) == Ok(ticks);
    let result = if matches {
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd,
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        }
    } else {
        0
    };
    let error = std::io::Error::last_os_error();
    unsafe { libc::close(fd) };
    if result < 0 && error.raw_os_error() != Some(libc::ESRCH) {
        Err(format!("signal owned process {pid}: {error}"))
    } else {
        Ok(())
    }
}

fn stop_child(child: &mut Child) -> Result<(), String> {
    if child.try_wait().map_err(|e| e.to_string())?.is_some() {
        return Ok(());
    }
    // An unreaped direct Child cannot have its PID reused.
    if unsafe { libc::kill(child.id() as i32, libc::SIGTERM) } < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error.to_string());
        }
    }
    if child
        .wait_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?
        .is_none()
    {
        child.kill().map_err(|e| e.to_string())?;
        if child
            .wait_timeout(Duration::from_secs(2))
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Err(format!("owned Herdr child {} did not exit", child.id()));
        }
    }
    Ok(())
}

pub(super) struct TerminalRestore(libc::termios);
impl TerminalRestore {
    pub fn capture() -> Result<Self, String> {
        // tcgetattr writes the entire initialized termios object on success.
        let mut saved = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut saved) } < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Self(saved))
    }
}
impl Drop for TerminalRestore {
    fn drop(&mut self) {
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.0) } < 0 {
            eprintln!(
                "AW terminal restore failed: {}",
                std::io::Error::last_os_error()
            );
        }
        let mut out = std::io::stdout().lock();
        let _=out.write_all(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?2004l\x1b[?1004l\x1b[?2031l\x1b[<u\x1b[?7h\x1b[?1049l\x1b[?25h\x1b[0m");
        let _ = out.flush();
    }
}
