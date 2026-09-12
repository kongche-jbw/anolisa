//! Bounded unbuffered stdout delivery; backpressure cannot bypass cancellation.

use aw_hook_cli::Error;
use serde_json::Value;
use std::{
    io::ErrorKind,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

struct Flags {
    previous: libc::c_int,
    restored: bool,
}
impl Drop for Flags {
    fn drop(&mut self) {
        // Restore the inherited open-file-description flags on every exit path.
        if !self.restored {
            unsafe {
                libc::fcntl(libc::STDOUT_FILENO, libc::F_SETFL, self.previous);
            }
        }
    }
}

/// Writes one JSON response without a buffered flush or an unbounded pipe wait.
pub(super) fn response(value: &Value) -> Result<(), Error> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| Error::Input)?;
    bytes.push(b'\n');
    // The CLI owns stdout for the duration of this call; no other thread writes it.
    let previous = unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_GETFL) };
    if previous < 0 {
        return Err(Error::Input);
    }
    let mut flags = Flags {
        previous,
        restored: false,
    };
    if unsafe {
        libc::fcntl(
            libc::STDOUT_FILENO,
            libc::F_SETFL,
            previous | libc::O_NONBLOCK,
        )
    } < 0
    {
        return Err(Error::Input);
    }
    let start = Instant::now();
    let mut offset = 0;
    while offset < bytes.len() {
        if crate::CANCELLED.load(Ordering::Relaxed) {
            return Err(Error::Execution);
        }
        let remaining = Duration::from_secs(5)
            .checked_sub(start.elapsed())
            .ok_or(Error::Input)?;
        let mut descriptor = libc::pollfd {
            fd: libc::STDOUT_FILENO,
            events: libc::POLLOUT,
            revents: 0,
        };
        // A short poll interval also bounds cancellation when a signal preceded poll.
        let ready = unsafe {
            libc::poll(
                &mut descriptor,
                1,
                remaining.as_millis().clamp(1, 50) as i32,
            )
        };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(Error::Input);
        }
        if ready == 0 {
            continue;
        }
        if descriptor.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(Error::Input);
        }
        // The remaining slice stays live; the descriptor is nonblocking and owned.
        let count = unsafe {
            libc::write(
                libc::STDOUT_FILENO,
                bytes[offset..].as_ptr().cast(),
                bytes.len() - offset,
            )
        };
        if count < 0 {
            match std::io::Error::last_os_error().kind() {
                ErrorKind::Interrupted | ErrorKind::WouldBlock => continue,
                _ => return Err(Error::Input),
            }
        }
        if count == 0 {
            return Err(Error::Input);
        }
        offset += count as usize;
    }
    if crate::CANCELLED.load(Ordering::Relaxed) {
        return Err(Error::Execution);
    }
    if unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_SETFL, previous) } < 0 {
        return Err(Error::Input);
    }
    flags.restored = true;
    Ok(())
}
