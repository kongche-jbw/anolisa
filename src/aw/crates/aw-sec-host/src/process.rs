//! Exchanges bounded pipes without helper threads or unbounded child waits.

use crate::Config;

#[cfg(target_os = "linux")]
mod linux {
    use super::Config;
    use std::{
        io::{Read, Write},
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            // The child creates a dedicated process group. Kill only that group;
            // descendants must not keep our protocol pipes alive after shutdown.
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }

    fn nonblocking(pipe: &impl AsRawFd) -> Result<(), &'static str> {
        let fd = pipe.as_raw_fd();
        // FDs are live owned pipes; F_GETFL/F_SETFL only change their I/O mode.
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
        // One read per iteration keeps continuous writers from starving deadline checks.
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

    pub(super) fn run(
        config: &Config,
        request: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>, &'static str> {
        if request.len() > config.limits.input_bytes {
            return Err("native_input_limit");
        }
        let start = Instant::now();
        let child = Command::new(&config.program)
            .args(&config.args)
            .env_clear()
            .envs(&config.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|_| "provider_spawn_failed")?;
        let mut child = OwnedChild(child);
        let stdin = child.0.stdin.take().ok_or("pipe_setup_failed")?;
        let mut stdout = child.0.stdout.take().ok_or("pipe_setup_failed")?;
        let mut stderr = child.0.stderr.take().ok_or("pipe_setup_failed")?;
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
            if start.elapsed() >= Duration::from_millis(timeout_ms) {
                return Err("provider_timeout");
            }
            if let Some(pipe) = stdin.as_mut() {
                match pipe.write(&request[sent..]) {
                    Ok(n) => sent += n,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(_) => return Err("pipe_write_failed"),
                }
                if sent == request.len() {
                    stdin.take();
                }
            }
            if !out_done {
                out_done = drain(&mut stdout, &mut output, config.limits.output_bytes)?;
            }
            if !err_done {
                err_done = drain(&mut stderr, &mut diagnostics, config.limits.stderr_bytes)?;
            }
            if out_done && err_done {
                // Observe without reaping: the group leader retains its PID
                // until Drop kills the owned group and then waits for it.
                let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
                let status = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        child.0.id(),
                        &mut info,
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                };
                if status < 0 {
                    return Err("provider_wait_failed");
                }
                if unsafe { info.si_pid() } != 0 {
                    if info.si_code != libc::CLD_EXITED || unsafe { info.si_status() } != 0 {
                        return Err("provider_exit_failed");
                    }
                    if sent != request.len() {
                        return Err("provider_input_incomplete");
                    }
                    return Ok(output);
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
}

pub(crate) fn run(
    config: &Config,
    request: &[u8],
    timeout_ms: u64,
) -> Result<Vec<u8>, &'static str> {
    #[cfg(target_os = "linux")]
    {
        linux::run(config, request, timeout_ms)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, request, timeout_ms);
        Err("unsupported_platform")
    }
}
