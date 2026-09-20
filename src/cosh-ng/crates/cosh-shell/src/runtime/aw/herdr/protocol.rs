//! Bounded native Herdr JSON RPC over the private instance socket.

use nix::libc;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    os::{
        fd::FromRawFd,
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn rpc(path: &Path, method: &str, params: Value) -> Result<Value, String> {
    let mut stream = connect(path).map_err(|e| format!("Herdr {method}: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let request = serde_json::to_vec(&json!({"id":"cosh-aw","method":method,"params":params}))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(&request)
        .and_then(|_| stream.write_all(b"\n"))
        .map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    let mut bytes = [0; 8192];
    while Instant::now() < deadline && result.len() < 1024 * 1024 {
        let count = stream
            .read(&mut bytes)
            .map_err(|e| format!("Herdr {method}: {e}"))?;
        if count == 0 {
            return Err(format!("Herdr {method}: connection closed"));
        }
        result.extend_from_slice(&bytes[..count]);
        if let Some(end) = result.iter().position(|byte| *byte == b'\n') {
            let value: Value = serde_json::from_slice(&result[..end]).map_err(|e| e.to_string())?;
            if value["id"] != "cosh-aw" || value.get("error").is_some() {
                return Err(format!("Herdr {method}: rejected response"));
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| format!("Herdr {method}: result missing"));
        }
    }
    Err(format!(
        "Herdr {method}: response deadline or size exceeded"
    ))
}

fn connect(path: &Path) -> std::io::Result<UnixStream> {
    // sockaddr_un is fully initialized before libc observes it.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err(std::io::Error::other("invalid Herdr socket path"));
    }
    address.sun_family = libc::AF_UNIX as _;
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as _;
    }
    // A successful descriptor transfers immediately to UnixStream.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    stream.set_read_timeout(Some(Duration::from_millis(300)))?;
    stream.set_write_timeout(Some(Duration::from_millis(300)))?;
    // Linux SO_SNDTIMEO bounds connect even when the listen backlog is full.
    if unsafe {
        libc::connect(
            fd,
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&address) as _,
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(stream)
}
