//! Qoder replacement composition, with explicit native peers and durable records.

mod common;
use aw_core::ports::Cancellation;
use aw_hook_cli::{project, project_with_cancellation};
use common::{projection_config as fixture, Directory};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[test]
fn projection_is_recorded_after_required_inspection_before_return() {
    let dir = Directory::new();
    let (config, native, candidate) = fixture(&dir, "ok");
    let result = project(serde_json::from_value(config).unwrap(), native.clone()).unwrap();
    assert!(result.projected);
    assert_eq!(
        result.response,
        json!({"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":candidate}})
    );
    let record: Value = serde_json::from_slice(&fs::read(&result.record_path).unwrap()).unwrap();
    assert_eq!(record["calls"].as_array().unwrap().len(), 2);
    assert_eq!(
        record["calls"][0]["invocation"]["capability"],
        "security.content.inspect/v2"
    );
    assert_eq!(
        record["calls"][1]["invocation"]["capability"],
        "context.projection.prepare/v2"
    );
    assert_eq!(
        record["calls"][1]["output"]["candidate"]["reversibility"],
        "unrecoverable"
    );
    assert_eq!(record["execution"]["decision"], "proceed");
    assert!(
        !result.record_path.with_extension("returned.json").exists(),
        "library preparation is not transport completion"
    );
    assert_eq!(
        fs::read_to_string(dir.0.join("received")).unwrap(),
        native["tool_response"]["stdout"]
    );
}

#[test]
fn sensitive_inspection_adds_observation_without_denying_projection() {
    let dir = Directory::new();
    let (config, native, candidate) = fixture(&dir, "sensitive");
    let result = project(serde_json::from_value(config).unwrap(), native).unwrap();
    assert!(result.projected);
    assert_eq!(
        result.response["hookSpecificOutput"]["updatedToolOutput"],
        candidate
    );
    assert!(result.response["systemMessage"]
        .as_str()
        .unwrap()
        .contains("observational"));
    assert!(result.response.get("decision").is_none());
}

#[test]
fn required_failure_stops_projection_and_optional_failure_preserves_source() {
    for mode in ["security", "tokenless", "no_savings"] {
        let dir = Directory::new();
        let (config, native, _) = fixture(&dir, mode);
        let result = project(serde_json::from_value(config).unwrap(), native).unwrap();
        assert!(!result.projected, "{mode}");
        assert!(result.response.get("hookSpecificOutput").is_none());
        assert_eq!(
            dir.0.join("tokenless_received").exists(),
            mode != "security"
        );
        let record: Value = serde_json::from_slice(&fs::read(result.record_path).unwrap()).unwrap();
        assert_eq!(record["execution"]["decision"], "preserve");
        if mode == "security" {
            assert!(result.response["systemMessage"]
                .as_str()
                .unwrap()
                .contains("unavailable"));
        }
    }
}

#[test]
fn missing_opt_in_or_history_binding_prevents_all_provider_probes() {
    for (pointer, value) in [
        ("/accepted_reversibility", json!(["lossless"])),
        ("/retention", json!("none")),
        ("/history_profile", json!("unknown")),
        ("/max_observation_delay_ms", json!(0)),
        ("/tokenless/provider_id", json!("sec-test")),
    ] {
        let dir = Directory::new();
        let (mut config, native, _) = fixture(&dir, "ok");
        *config.pointer_mut(pointer).unwrap() = value;
        assert!(
            project(serde_json::from_value(config).unwrap(), native).is_err(),
            "{pointer}"
        );
        assert!(!dir.0.join("security_probe").exists(), "{pointer}");
        assert!(!dir.0.join("tokenless_probe").exists(), "{pointer}");
    }
    let dir = Directory::new();
    let (config, mut native, _) = fixture(&dir, "ok");
    native["tool_response"]["exitCode"] = json!(1);
    assert!(project(serde_json::from_value(config).unwrap(), native).is_err());
    assert!(!dir.0.join("security_probe").exists());
}

#[test]
fn failed_record_persistence_never_releases_a_candidate() {
    let dir = Directory::new();
    let (config, native, _) = fixture(&dir, "record_failure");
    assert!(project(serde_json::from_value(config).unwrap(), native).is_err());
    assert!(dir.0.join("tokenless_received").exists());
    assert!(fs::read_dir(dir.0.join("records"))
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn cancellation_during_projection_reclaims_transport_without_returning_candidate() {
    struct Cancel(AtomicBool);
    impl Cancellation for Cancel {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }
    let dir = Directory::new();
    let (config, native, _) = fixture(&dir, "cancel");
    let cancel = Arc::new(Cancel(AtomicBool::new(false)));
    let observer = cancel.clone();
    let marker = dir.0.join("tokenless_received");
    let waiter = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if marker.exists() {
                observer.0.store(true, Ordering::Relaxed);
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    });
    let result = project_with_cancellation(serde_json::from_value(config).unwrap(), native, cancel);
    assert!(
        waiter.join().unwrap(),
        "projection provider was never launched"
    );
    assert!(result.is_err());
}

struct RunningCli(Option<std::process::Child>);
impl Drop for RunningCli {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            if child.try_wait().ok().flatten().is_none() {
                unsafe {
                    libc::kill(child.id() as i32, libc::SIGTERM);
                }
                let deadline = Instant::now() + Duration::from_secs(3);
                while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

#[test]
fn cli_marks_only_completed_stdout_delivery() {
    for discard_stdout in [false, true] {
        let dir = Directory::new();
        let (config, native, candidate) = fixture(&dir, "ok");
        let settings = dir.0.join("settings.json");
        fs::write(&settings, serde_json::to_vec(&config).unwrap()).unwrap();
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();
        let mut running = RunningCli(Some(
            Command::new(env!("CARGO_BIN_EXE_aw-hook-cli"))
                .arg("qoder-project")
                .arg(settings)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ));
        let child = running.0.as_mut().unwrap();
        if discard_stdout {
            drop(child.stdout.take());
        }
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&native).unwrap())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while child.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "projection CLI deadline exceeded"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = running.0.take().unwrap().wait_with_output().unwrap();
        assert_eq!(
            output.status.success(),
            !discard_stdout,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let records: Vec<_> = fs::read_dir(dir.0.join("records"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        let markers = records
            .iter()
            .filter(|path| path.to_string_lossy().ends_with(".returned.json"))
            .count();
        assert_eq!(markers, usize::from(!discard_stdout));
        if !discard_stdout {
            let response: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                response["hookSpecificOutput"]["updatedToolOutput"],
                candidate
            );
        }
    }
}

#[test]
fn cancellation_after_journal_start_records_a_call_free_execution() {
    struct CancelAtDispatch(std::path::PathBuf);
    impl Cancellation for CancelAtDispatch {
        fn is_cancelled(&self) -> bool {
            fs::read_dir(&self.0)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .any(|entry| {
                    fs::read_to_string(entry.path())
                        .is_ok_and(|text| text.contains("invocation_started"))
                })
        }
    }
    let dir = Directory::new();
    let (config, native, _) = fixture(&dir, "ok");
    let cancellation = Arc::new(CancelAtDispatch(dir.0.join("journal")));
    assert!(project_with_cancellation(
        serde_json::from_value(config).unwrap(),
        native,
        cancellation
    )
    .is_err());
    assert!(!dir.0.join("received").exists());
    let records: Vec<_> = fs::read_dir(dir.0.join("records"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(records.len(), 1);
    let view = aw_hook_cli::query(&records[0]).unwrap();
    assert_eq!(view["execution_decision"], "cancelled");
    assert_eq!(view["prepared"], false);
}

#[test]
fn nonconsuming_stdout_is_bounded_and_cancellable_without_a_returned_marker() {
    use std::os::fd::{AsRawFd, FromRawFd};
    for terminate in [false, true] {
        let dir = Directory::new();
        let (config, native, candidate) = fixture(&dir, "large");
        assert!(candidate.len() > 4096);
        let settings = dir.0.join("settings.json");
        fs::write(&settings, serde_json::to_vec(&config).unwrap()).unwrap();
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();
        let mut fds = [-1; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        // pipe2 succeeded; these two File owners close only this test's descriptors.
        let reader = unsafe { fs::File::from_raw_fd(fds[0]) };
        let writer = unsafe { fs::File::from_raw_fd(fds[1]) };
        assert_eq!(
            unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_SETPIPE_SZ, 4096) },
            4096
        );
        let original_flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
        let mut running = RunningCli(Some(
            Command::new(env!("CARGO_BIN_EXE_aw-hook-cli"))
                .arg("qoder-project")
                .arg(settings)
                .stdin(Stdio::piped())
                .stdout(Stdio::from(writer.try_clone().unwrap()))
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ));
        let child = running.0.as_mut().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&native).unwrap())
            .unwrap();
        let start_deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let mut available: libc::c_int = 0;
            assert_eq!(
                unsafe { libc::ioctl(reader.as_raw_fd(), libc::FIONREAD, &mut available) },
                0
            );
            if available > 0 {
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "CLI exited before candidate transmission"
            );
            assert!(
                Instant::now() < start_deadline,
                "candidate transmission never started"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let blocked_at = Instant::now();
        if terminate {
            assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
        }
        let bound = if terminate {
            Duration::from_secs(3)
        } else {
            Duration::from_secs(7)
        };
        while child.try_wait().unwrap().is_none() {
            assert!(
                blocked_at.elapsed() < bound,
                "stdout backpressure bypassed deadline or cancellation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        if !terminate {
            assert!(blocked_at.elapsed() >= Duration::from_secs(4));
        }
        let output = running.0.take().unwrap().wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) },
            original_flags,
            "stdout flags must be restored on the inherited open-file description"
        );
        assert!(fs::read_dir(dir.0.join("records"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .path()
                .to_string_lossy()
                .ends_with(".returned.json")));
    }
}
