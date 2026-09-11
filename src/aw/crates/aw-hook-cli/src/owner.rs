//! Linux ancestry and incarnation checks for an explicitly bound Agent process.

use crate::{Error, Settings};
use std::{fs::File, io::Read};

/// Reads parent PID and start ticks from Linux procfs without interpreting comm.
///
/// # Errors
/// Rejects missing, overlong or malformed records and unsupported platforms.
pub fn process_identity(pid: u32) -> Result<(u32, u64), Error> {
    if !cfg!(target_os = "linux") || pid == 0 {
        return Err(Error::Identity);
    }
    let mut stat = String::new();
    File::open(format!("/proc/{pid}/stat"))
        .map_err(|_| Error::Identity)?
        .take(4097)
        .read_to_string(&mut stat)
        .map_err(|_| Error::Identity)?;
    if stat.len() > 4096 {
        return Err(Error::Identity);
    }
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .ok_or(Error::Identity)?
        .1
        .split_whitespace()
        .collect();
    Ok((
        fields
            .get(1)
            .ok_or(Error::Identity)?
            .parse()
            .map_err(|_| Error::Identity)?,
        fields
            .get(19)
            .ok_or(Error::Identity)?
            .parse()
            .map_err(|_| Error::Identity)?,
    ))
}

pub(crate) fn verify(settings: &Settings) -> Result<(), Error> {
    if settings.runtime["process_ref"]
        != format!("pid:{}@{}", settings.agent_pid, settings.agent_start_ticks)
        || settings.runtime["observation_source"] != "owned_child"
    {
        return Err(Error::Identity);
    }
    let mut pid = std::process::id();
    for _ in 0..128 {
        let (parent, start) = process_identity(pid)?;
        if pid == settings.agent_pid {
            return if start == settings.agent_start_ticks {
                Ok(())
            } else {
                Err(Error::Identity)
            };
        }
        if parent == 0 || parent == pid {
            break;
        }
        pid = parent;
    }
    Err(Error::Identity)
}
