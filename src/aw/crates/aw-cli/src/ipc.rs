//! Length-framed private Unix socket messages with a total I/O deadline.
use crate::Error;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::Path,
    thread,
    time::{Duration, Instant},
};
const MAX_FRAME: usize = 40 * 1024 * 1024;
fn transfer(
    stream: &mut UnixStream,
    buffer: &mut [u8],
    deadline: Instant,
    write: bool,
) -> Result<(), Error> {
    let mut offset = 0;
    while offset < buffer.len() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("socket deadline exceeded")?;
        let result = if write {
            stream.set_write_timeout(Some(remaining))?;
            stream.write(&buffer[offset..])
        } else {
            stream.set_read_timeout(Some(remaining))?;
            stream.read(&mut buffer[offset..])
        };
        match result {
            Ok(0) => return Err("socket closed before complete message".into()),
            Ok(n) => offset += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub(crate) fn read<T: DeserializeOwned>(
    stream: &mut UnixStream,
    deadline: Instant,
) -> Result<T, Error> {
    let mut size = [0; 4];
    transfer(stream, &mut size, deadline, false)?;
    let size = u32::from_be_bytes(size) as usize;
    if size > MAX_FRAME {
        return Err("socket frame too large".into());
    }
    let mut bytes = vec![0; size];
    transfer(stream, &mut bytes, deadline, false)?;
    Ok(serde_json::from_slice(&bytes)?)
}
pub(crate) fn write<T: Serialize>(
    stream: &mut UnixStream,
    value: &T,
    deadline: Instant,
) -> Result<(), Error> {
    let mut bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME {
        return Err("socket frame too large".into());
    }
    transfer(
        stream,
        &mut (bytes.len() as u32).to_be_bytes(),
        deadline,
        true,
    )?;
    transfer(stream, &mut bytes, deadline, true)
}
pub(crate) fn call(
    socket: &Path,
    request: &crate::model::Request,
) -> Result<crate::model::Response, Error> {
    let seconds = if request.op == "invoke" { 310 } else { 1 };
    call_until(
        socket,
        request,
        Instant::now() + Duration::from_secs(seconds),
    )
}

/// Uses the caller's total deadline, including a full Unix listener backlog.
pub(crate) fn call_until(
    socket: &Path,
    request: &crate::model::Request,
    deadline: Instant,
) -> Result<crate::model::Response, Error> {
    let mut stream = connect_until(socket, deadline)?;
    write(&mut stream, request, deadline)?;
    read(&mut stream, deadline)
}

fn connect_until(path: &Path, deadline: Instant) -> Result<UnixStream, Error> {
    let bytes = path.as_os_str().as_bytes();
    // A zeroed sockaddr_un has no invalid Rust values; sun_path is filled below.
    let mut address = unsafe { std::mem::zeroed::<libc::sockaddr_un>() };
    if bytes.is_empty() || bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err("invalid Unix socket path".into());
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
        *target = *byte as libc::c_char;
    }
    let length =
        (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
    // The descriptor is immediately placed under UnixStream ownership.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    stream.set_nonblocking(true)?;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("socket deadline exceeded")?;
        // address points to an initialized sockaddr_un of the supplied length.
        let result = unsafe {
            libc::connect(
                stream.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                length,
            )
        };
        if result == 0 {
            stream.set_nonblocking(false)?;
            return Ok(stream);
        }
        let error = std::io::Error::last_os_error();
        if !matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
        ) {
            return Err(error.into());
        }
        // Linux AF_UNIX reports EAGAIN for a full backlog; retrying this
        // unconnected socket keeps connection setup within the same deadline.
        thread::sleep(Duration::from_millis(5).min(remaining));
    }
}
