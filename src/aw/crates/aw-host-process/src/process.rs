//! Bounded native pipes and Linux process-group cleanup without helper threads.

use crate::{Config, Error};
use aw_core::ports::Cancellation;

/// Native exit status and bounded stdout after verified process-group cleanup.
pub struct Output {
    /// Native exit code, or -1 for termination by a signal.
    pub exit_code: i32,
    /// Exact captured native stdout; stderr is discarded.
    pub stdout: Vec<u8>,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{Cancellation, Config, Output};
    use std::{
        fs,
        io::{Read, Write},
        os::{fd::AsRawFd, unix::process::CommandExt},
        path::Path,
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    const POLL_INTERVAL: Duration = Duration::from_millis(2);
    const CLEANUP_GRACE: Duration = Duration::from_secs(1);
    const MAX_SCAN_ENTRIES: usize = 65_536;

    struct OwnedChild {
        child: Child,
        reaped: bool,
    }

    impl OwnedChild {
        fn signal_group(&self) -> Result<(), &'static str> {
            // The unreaped group leader reserves this PID, preventing reuse by
            // an unrelated process group before the last group-directed signal.
            if unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) } < 0
                && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            {
                return Err("provider_cleanup_failed");
            }
            Ok(())
        }

        fn observe_exit(&self) -> Result<Option<i32>, &'static str> {
            // waitid writes a live siginfo_t and WNOWAIT keeps our child unreaped.
            let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    return Ok(None);
                }
                return Err("provider_wait_failed");
            }
            // A successful waitid populates these fields, or leaves si_pid zero.
            if unsafe { info.si_pid() } == 0 {
                Ok(None)
            } else if info.si_code == libc::CLD_EXITED {
                Ok(Some(unsafe { info.si_status() }))
            } else {
                Ok(Some(-1))
            }
        }

        fn cleanup(&mut self) -> Result<(), &'static str> {
            let deadline = Instant::now() + CLEANUP_GRACE;
            self.signal_group()?;
            loop {
                if Instant::now() >= deadline {
                    return Err("provider_cleanup_failed");
                }
                if self
                    .observe_exit()
                    .map_err(|_| "provider_cleanup_failed")?
                    .is_some()
                    && group_stopped(self.child.id(), deadline)?
                {
                    match self.child.try_wait() {
                        Ok(Some(_)) => {
                            self.reaped = true;
                            return Ok(());
                        }
                        _ => return Err("provider_cleanup_failed"),
                    }
                }
                thread::sleep(
                    POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
    }

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if !self.reaped {
                // Explicit cleanup reports failures. Unwinding only attempts
                // nonblocking cleanup; a kernel-stuck child cannot be awaited here.
                let _ = self.signal_group();
                let _ = self.child.try_wait();
            }
        }
    }

    fn charge_scan(entries: &mut usize, deadline: Instant) -> Result<(), &'static str> {
        *entries += 1;
        if *entries > MAX_SCAN_ENTRIES || Instant::now() >= deadline {
            return Err("provider_cleanup_failed");
        }
        Ok(())
    }

    fn group_state(path: &Path, group: u32) -> Result<Option<char>, &'static str> {
        let stat = match fs::read(path) {
            Ok(stat) => stat,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(libc::ESRCH) =>
            {
                return Ok(None)
            }
            Err(_) => return Err("provider_cleanup_failed"),
        };
        // comm may contain arbitrary bytes or parentheses; its suffix is ASCII.
        let end = stat
            .iter()
            .rposition(|byte| *byte == b')')
            .ok_or("provider_cleanup_failed")?;
        let fields =
            std::str::from_utf8(&stat[end + 1..]).map_err(|_| "provider_cleanup_failed")?;
        let mut fields = fields.split_whitespace();
        let state = fields.next().ok_or("provider_cleanup_failed")?;
        let pgid = fields
            .nth(1)
            .ok_or("provider_cleanup_failed")?
            .parse::<u32>()
            .map_err(|_| "provider_cleanup_failed")?;
        if pgid == group {
            Ok(state.chars().next())
        } else {
            Ok(None)
        }
    }

    fn group_stopped(group: u32, deadline: Instant) -> Result<bool, &'static str> {
        let mut scanned = 0;
        for entry in fs::read_dir("/proc").map_err(|_| "provider_cleanup_failed")? {
            charge_scan(&mut scanned, deadline)?;
            let entry = entry.map_err(|_| "provider_cleanup_failed")?;
            if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
                continue;
            }
            match group_state(&entry.path().join("stat"), group)? {
                None => continue,
                Some('Z' | 'X') => {}
                Some(_) => return Ok(false),
            }
            // A zombie thread-group leader can still have live threads. Verify
            // matching groups' tasks before releasing the reserved leader PID.
            let tasks = match fs::read_dir(entry.path().join("task")) {
                Ok(tasks) => tasks,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err("provider_cleanup_failed"),
            };
            for task in tasks {
                charge_scan(&mut scanned, deadline)?;
                let task = task.map_err(|_| "provider_cleanup_failed")?;
                if !matches!(
                    group_state(&task.path().join("stat"), group)?,
                    None | Some('Z' | 'X')
                ) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn nonblocking(pipe: &impl AsRawFd) -> Result<(), &'static str> {
        let fd = pipe.as_raw_fd();
        // These are live owned pipe descriptors; fcntl changes only their I/O mode.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err("pipe_setup_failed");
        }
        Ok(())
    }

    fn drain(
        pipe: &mut impl Read,
        bytes: &mut Vec<u8>,
        limit: usize,
    ) -> Result<bool, &'static str> {
        let mut buffer = [0u8; 8192];
        // One bounded read per iteration prevents continuous output starving deadlines.
        match pipe.read(&mut buffer) {
            Ok(0) => Ok(true),
            Ok(n) => {
                if n > limit.saturating_sub(bytes.len()) {
                    return Err("provider_output_limit");
                }
                bytes.extend_from_slice(&buffer[..n]);
                Ok(false)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(false)
            }
            Err(_) => Err("pipe_read_failed"),
        }
    }

    fn exchange(
        child: &mut OwnedChild,
        config: &Config,
        request: &[u8],
        start: Instant,
        timeout_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<Output, &'static str> {
        let stdin = child.child.stdin.take().ok_or("pipe_setup_failed")?;
        let mut stdout = child.child.stdout.take().ok_or("pipe_setup_failed")?;
        let mut stderr = child.child.stderr.take().ok_or("pipe_setup_failed")?;
        nonblocking(&stdin)?;
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let mut stdin = Some(stdin);
        let mut sent = 0;
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let mut out_done = false;
        let mut err_done = false;
        loop {
            if cancellation.is_cancelled() {
                return Err("provider_cancelled");
            }
            if start.elapsed() >= Duration::from_millis(timeout_ms) {
                return Err("provider_timeout");
            }
            if sent == request.len() {
                stdin.take();
            }
            if let Some(pipe) = stdin.as_mut() {
                match pipe.write(&request[sent..]) {
                    Ok(0) => return Err("pipe_write_failed"),
                    Ok(n) => sent += n,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(_) => return Err("pipe_write_failed"),
                }
            }
            if !out_done {
                out_done = drain(&mut stdout, &mut output, config.limits.output_bytes)?;
            }
            if !err_done {
                err_done = drain(&mut stderr, &mut diagnostics, config.limits.stderr_bytes)?;
            }
            if out_done && err_done {
                if let Some(exit_code) = child.observe_exit()? {
                    if sent != request.len() {
                        return Err("provider_input_incomplete");
                    }
                    return Ok(Output {
                        exit_code,
                        stdout: output,
                    });
                }
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    pub(super) fn run(
        config: &Config,
        argv: &[&str],
        request: &[u8],
        timeout_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<Output, &'static str> {
        if cancellation.is_cancelled() {
            return Err("provider_cancelled");
        }
        if request.len() > config.limits.input_bytes {
            return Err("native_input_limit");
        }
        if timeout_ms == 0 {
            return Err("provider_timeout");
        }
        let start = Instant::now();
        let child = Command::new(&config.program)
            .args(&config.args)
            .args(argv)
            .current_dir(&config.cwd)
            .env_clear()
            .envs(&config.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|_| "provider_spawn_failed")?;
        let mut child = OwnedChild {
            child,
            reaped: false,
        };
        let result = exchange(&mut child, config, request, start, timeout_ms, cancellation);
        // Success is unavailable until owned-group cleanup has been verified.
        // Cleanup errors take precedence over a native response or timeout.
        child.cleanup()?;
        result
    }
}

/// Exchanges native pipes using an already validated, caller-pinned configuration.
///
/// The caller owns protocol selection, checks pins before/after the exchange, and
/// exclusively owns child reaping. This function enforces configured stream limits
/// and the supplied timeout, then verifies owned-group cleanup with a one-second
/// grace. It installs no signal handlers and does not contain setsid escapes.
///
/// # Errors
/// Returns bounded errors for spawn/I/O, stream limits, timeout, cancellation,
/// unverifiable cleanup or unsupported platforms. Kernel-blocked operations have
/// no hard realtime completion guarantee; Drop never performs a blocking wait.
pub fn run(
    config: &Config,
    argv: &[&str],
    request: &[u8],
    timeout_ms: u64,
    cancellation: &dyn Cancellation,
) -> Result<Output, Error> {
    #[cfg(target_os = "linux")]
    {
        linux::run(config, argv, request, timeout_ms, cancellation).map_err(Error::Process)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, argv, request, timeout_ms, cancellation);
        Err(Error::Process("unsupported_platform"))
    }
}
