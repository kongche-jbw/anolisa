//! Private, bounded snapshots and a bounded lock for short state transitions.

use super::{evidence, Error};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
    thread,
    time::{Duration, Instant},
};

pub(super) fn read<T: DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let bytes = crate::input::read_private(path, crate::MAX_INPUT_BYTES).map_err(evidence)?;
    serde_json::from_value(aw_contracts::canonical::parse(&bytes).map_err(evidence)?)
        .map_err(evidence)
}

pub(super) fn create(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let bytes = serde_json::to_vec(value).map_err(evidence)?;
    if bytes.len() > crate::MAX_INPUT_BYTES {
        return Err(Error::Evidence);
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(&bytes)?;
    Ok(())
}

pub(super) fn replace(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    create(&temporary, value)?;
    let result = fs::rename(&temporary, path);
    if result.is_err() {
        fs::remove_file(&temporary)?;
    }
    result.map_err(Error::Io)
}

pub(super) fn lock(root: &Path) -> Result<File, Error> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join("state.lock"))?;
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        // The open file remains alive until the caller finishes its transition.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EWOULDBLOCK) || Instant::now() >= deadline {
            return Err(Error::Profile("state lock unavailable"));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

pub(super) fn directory(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)?;
    Ok(())
}

pub(super) fn descendant(pid: u32, ticks: u64) -> Result<(), Error> {
    let mut current = std::process::id();
    for _ in 0..128 {
        let (parent, start) = crate::process_identity(current).map_err(evidence)?;
        if current == pid {
            return if ticks == start {
                Ok(())
            } else {
                Err(Error::Evidence)
            };
        }
        if parent <= 1 || parent == current {
            break;
        }
        current = parent;
    }
    Err(Error::Evidence)
}
