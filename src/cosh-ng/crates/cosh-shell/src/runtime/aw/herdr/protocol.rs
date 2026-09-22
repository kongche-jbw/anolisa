//! Bounded native Herdr JSON RPC over the private instance socket.

use nix::libc;
use serde_json::{json, Value};
use std::{
    io::{ErrorKind, Read, Write},
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
        // Closing a pane can briefly occupy the server while its PTY owner
        // drains. Poll this response until its deadline; never replay a request.
        stream
            .set_read_timeout(Some(
                deadline
                    .saturating_duration_since(Instant::now())
                    .max(Duration::from_millis(1))
                    .min(Duration::from_millis(300)),
            ))
            .map_err(|e| e.to_string())?;
        let count = match stream.read(&mut bytes) {
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(error) => return Err(format!("Herdr {method}: {error}")),
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        os::unix::net::UnixListener,
        thread,
    };

    fn server(delay: Duration) -> (tempfile::TempDir, std::thread::JoinHandle<()>) {
        let directory = tempfile::Builder::new()
            .prefix("hr-")
            .tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
            .unwrap();
        let listener = UnixListener::bind(directory.path().join("s")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("bounded accept: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&request).unwrap()["method"],
                "pane.list"
            );
            thread::sleep(delay);
            if delay < Duration::from_secs(2) {
                stream
                    .write_all(b"{\"id\":\"cosh-aw\",\"result\":")
                    .unwrap();
                thread::sleep(delay);
                stream.write_all(b"{\"panes\":[]}}\n").unwrap();
            }
        });
        (directory, worker)
    }

    #[test]
    fn rpc_waits_across_poll_timeouts_for_one_response() {
        let (directory, worker) = server(Duration::from_millis(450));
        let value = rpc(&directory.path().join("s"), "pane.list", json!({})).unwrap();
        assert_eq!(value, json!({"panes":[]}));
        worker.join().unwrap();
    }

    #[test]
    fn rpc_response_deadline_stays_bounded() {
        let (directory, worker) = server(Duration::from_millis(2200));
        let started = Instant::now();
        let error = rpc(&directory.path().join("s"), "pane.list", json!({})).unwrap_err();
        assert!(error.contains("response deadline"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
        worker.join().unwrap();
    }
}
