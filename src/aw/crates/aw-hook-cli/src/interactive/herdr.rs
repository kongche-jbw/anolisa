//! Optional bounded Herdr metadata projection, independent of Agent execution.

use super::{evidence, query, storage, Error, Prepared};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// A shell-owned optional viewer worker; dropping it cancels and joins the worker.
pub struct HerdrBridge {
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl HerdrBridge {
    /// Publishes read-only snapshots for at most 24 hours, with expiring metadata.
    ///
    /// # Errors
    /// Rejects malformed pane/socket declarations or a failed worker spawn.
    pub fn start(root: PathBuf, socket: PathBuf, pane: String) -> Result<Self, Error> {
        if !socket.is_absolute() || pane.is_empty() || pane.len() > 256 {
            return Err(Error::Profile("invalid Herdr pane or socket"));
        }
        let (stop, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("aw-herdr".into())
            .spawn(move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(86_400);
                for _ in 0..86_400 {
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    let status = if publish(&root, &socket, &pane).is_ok() {
                        "connected"
                    } else {
                        "unavailable"
                    };
                    if storage::replace(&root.join("viewer.json"), &json!({"status":status}))
                        .is_err()
                    {
                        break;
                    }
                    match receiver.recv_timeout(Duration::from_secs(1)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        _ => {
                            let _ = clear_agent(&root, &socket, &pane);
                            return;
                        }
                    }
                }
                let _ = clear_agent(&root, &socket, &pane);
                let _ = storage::replace(&root.join("viewer.json"), &json!({"status":"expired"}));
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for HerdrBridge {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        // Native tokens expire without another socket request during teardown.
    }
}

fn publish(root: &Path, socket: &Path, pane: &str) -> Result<(), Error> {
    let prepared: Prepared = storage::read(&root.join("prepared.json"))?;
    let process = rpc(socket, "pane.process_info", json!({"pane_id":pane}))?;
    if process["process_info"]["shell_pid"] != prepared.owner_pid {
        return Err(Error::Profile("Herdr pane does not own this cosh process"));
    }
    let mut current = None;
    for (index, entry) in fs::read_dir(root)?.enumerate() {
        if index >= 1028 {
            return Err(Error::Evidence);
        }
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with("run-") {
            continue;
        }
        let view = match query(&entry.path()) {
            Ok(view) => view,
            Err(error) => {
                let _ = clear_agent(root, socket, pane);
                return Err(error);
            }
        };
        if view["runtime_alive"] == true {
            if current.is_some() {
                return Err(Error::Profile("ambiguous foreground runtime"));
            }
            current = Some(view);
        }
    }
    let tokens = match &current {
        Some(view) if view["format"] == 2 => super::event_view::tokens(Some(view)),
        Some(view) => json!({
            "aw":format!("AW {} / attachment {} / gap {}", view["bridge_status"].as_str().unwrap_or("unknown"), view["attachment"], view["observation_gap"]),
            "aw_observation":match view["callbacks_in_runtime"].as_u64() {
                Some(callbacks) => format!("{callbacks} callbacks / {} handlers / {} failed", view["observed"], view["failed"]),
                None => format!("{} observed / {} failed / {} pending", view["observed"], view["failed"], view["pending"]),
            },
            "aw_security":effect_text(view, "checks"),
            "aw_projection":effect_text(view, "projections"),
            "aw_coverage":"OS not attached | adoption unconfirmed"
        }),
        None => super::event_view::tokens(None),
    };
    let seq = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(evidence)?
        .as_millis();
    // The managed session owner aggregates these structured reports. Pane
    // publishers never compete for workspace metadata, even in a single pane.
    storage::replace(
        &root.join("viewer-snapshot.json"),
        &json!({"format":1,"pane_id":pane,"owner_pid":prepared.owner_pid,
            "owner_ticks":prepared.owner_ticks,"observed_at_ms":seq,"view":current}),
    )?;
    if let Some(view) = &current {
        if view["format"] == 2 {
            // The verified owner and live binding identify the nested Qoder PTY.
            // Activity is only the last authenticated callback's presentation hint.
            rpc(
                socket,
                "pane.report_agent",
                json!({
                    "pane_id":pane,"source":"anolisa.aw","agent":"qodercli",
                    "state":view["activity"].as_str().unwrap_or("unknown"),
                    "seq":seq
                }),
            )?;
        }
    } else {
        rpc(
            socket,
            "pane.clear_agent_authority",
            json!({
                "pane_id":pane,"source":"anolisa.aw","seq":seq
            }),
        )?;
    }
    rpc(
        socket,
        "pane.report_metadata",
        json!({
            "pane_id":pane,"source":"anolisa.aw","seq":seq,"ttl_ms":3000,"tokens":tokens
        }),
    )?;

    Ok(())
}

fn clear_agent(root: &Path, socket: &Path, pane: &str) -> Result<(), Error> {
    let prepared: Prepared = storage::read(&root.join("prepared.json"))?;
    let process = rpc(socket, "pane.process_info", json!({"pane_id":pane}))?;
    if process["process_info"]["shell_pid"] != prepared.owner_pid {
        return Err(Error::Profile(
            "Herdr pane owner changed before clearing activity",
        ));
    }
    rpc(
        socket,
        "pane.clear_agent_authority",
        json!({
            "pane_id":pane,"source":"anolisa.aw",
            "seq":SystemTime::now().duration_since(UNIX_EPOCH).map_err(evidence)?.as_millis() + 1
        }),
    )?;
    Ok(())
}

fn effect_text(view: &Value, kind: &str) -> String {
    let provider = if kind == "checks" {
        "tool_guard"
    } else {
        "tool_response"
    };
    if view["format"] != 2 || view[provider] == "not_configured" {
        return "No effect provider configured".into();
    }
    let effect = &view["effects"];
    if effect["status"] != "available" {
        return "Effect evidence unavailable".into();
    }
    let counts = &effect[kind];
    if kind == "checks" {
        format!(
            "Checks {} pass / {} deny / {} failed / {} pending",
            counts["passed"], counts["denied"], counts["failed"], counts["pending"]
        )
    } else {
        format!(
            "Results {} candidates / {} kept / {} failed",
            counts["candidates"], counts["preserved"], counts["failed"]
        )
    }
}

fn rpc(path: &Path, method: &str, params: Value) -> Result<Value, Error> {
    let mut stream = connect(path)?;
    let request =
        serde_json::to_vec(&json!({"id":"aw-interactive","method":method,"params":params}))
            .map_err(evidence)?;
    stream.write_all(&request)?;
    stream.write_all(b"\n")?;
    let deadline = std::time::Instant::now() + Duration::from_millis(300);
    let mut response = Vec::new();
    let mut bytes = [0; 8192];
    loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or(Error::Evidence)?;
        stream.set_read_timeout(Some(remaining))?;
        let count = stream.read(&mut bytes)?;
        if count == 0 || response.len() + count > 1024 * 1024 {
            return Err(Error::Evidence);
        }
        response.extend_from_slice(&bytes[..count]);
        if let Some(end) = response.iter().position(|byte| *byte == b'\n') {
            let value = crate::parse_payload(&response[..end]).map_err(evidence)?;
            if value["id"] != "aw-interactive"
                || !value["error"].is_null()
                || value.get("result").is_none()
            {
                return Err(Error::Evidence);
            }
            return Ok(value["result"].clone());
        }
    }
}

fn connect(path: &Path) -> Result<UnixStream, Error> {
    let bytes = path.as_os_str().as_bytes();
    // sockaddr_un is initialized completely before connect reads its bytes.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err(Error::Evidence);
    }
    address.sun_family = libc::AF_UNIX as _;
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as _;
    }
    // Ownership of a successful socket immediately transfers to UnixStream.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    stream.set_write_timeout(Some(Duration::from_millis(300)))?;
    stream.set_read_timeout(Some(Duration::from_millis(300)))?;
    // Linux applies SO_SNDTIMEO to blocking Unix connect, bounding full backlogs.
    let result = unsafe {
        libc::connect(
            stream.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&address) as _,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_effect_provider_is_not_a_zero_count_check() {
        let view = json!({"format":2,"tool_guard":"not_configured",
            "tool_response":"experimental_bash_result_response","effects":{"status":"unavailable"}});
        assert_eq!(
            effect_text(&view, "checks"),
            "No effect provider configured"
        );
        assert_eq!(
            effect_text(&view, "projections"),
            "Effect evidence unavailable"
        );
    }
}
