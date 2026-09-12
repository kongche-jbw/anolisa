//! Real hook/Core/Host/Journal composition using an explicit native protocol peer.

mod common;
use common::{payload, settings, Directory};

use aw_adapters::Host;
use aw_contracts::canonical;
use aw_core::journal::FileJournal;
use aw_hook_cli::{inspect, parse_payload, read_settings, Settings, MAX_INPUT_BYTES};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn native_payload_parser_preserves_non_aw_values_and_rejects_ambiguity() {
    let value = json!({"未知":1.25,"large":18446744073709551615_u64,"array":[null,true,"\n"]});
    assert_eq!(
        parse_payload(&serde_json::to_vec(&value).unwrap()).unwrap(),
        value
    );
    for bytes in [
        br#"{"session_id":"a","session_id":"b"}"#.as_slice(),
        br#"{"nested":{"id":1,"id":2}}"#,
        b"{} {}",
        b"NaN",
    ] {
        assert!(parse_payload(bytes).is_err());
    }
    assert!(parse_payload(&vec![b' '; MAX_INPUT_BYTES + 1]).is_err());
}

#[test]
fn both_hooks_bind_real_transport_and_durable_journal_without_replacement() {
    for host in [Host::Codex, Host::Qoder] {
        let dir = Directory::new();
        let config = settings(&dir, false);
        let native = payload(host);
        let result = inspect(
            host,
            serde_json::from_value(config.clone()).unwrap(),
            native.clone(),
        )
        .unwrap();
        assert!(result.inspected);
        assert_eq!(result.response, json!({}));
        assert_eq!(result.execution.native_payload(), &native);
        let call = &result.execution.execution().calls()[0];
        let expected = native["tool_response"]
            .as_str()
            .unwrap()
            .as_bytes()
            .to_vec();
        assert_eq!(fs::read(dir.0.join("received")).unwrap(), expected);
        assert_eq!(call.result().receipt["disposition"], "produced");
        assert_eq!(
            call.result().output.as_ref().unwrap()["inspection"]["coverage"]["input_digest"],
            canonical::digest(&expected)
        );
        let journal = FileJournal::new(dir.0.join("journal")).unwrap();
        assert!(!journal
            .read_verified(
                &result.event_key,
                result.execution.execution().journal_ack()
            )
            .unwrap()
            .is_empty());
        assert!(inspect(host, serde_json::from_value(config).unwrap(), native).is_err());
        assert_eq!(
            fs::read(dir.0.join("received")).unwrap(),
            expected,
            "duplicate must not rescan"
        );
    }
}

#[test]
fn failed_inspection_records_failure_without_leaking_native_diagnostics() {
    let dir = Directory::new();
    let config = settings(&dir, true);
    let result = inspect(
        Host::Codex,
        serde_json::from_value(config).unwrap(),
        payload(Host::Codex),
    )
    .unwrap();
    assert!(!result.inspected);
    assert!(result.response["systemMessage"]
        .as_str()
        .unwrap()
        .contains("unavailable"));
    let receipt = &result.execution.execution().calls()[0].result().receipt;
    assert_eq!(receipt["disposition"], "failed");
    assert!(!receipt.to_string().contains("private scanner"));
    assert!(result.execution.execution().calls()[0]
        .result()
        .output
        .is_none());
}

#[test]
fn missing_or_conflicting_native_and_owner_identity_never_scan() {
    let dir = Directory::new();
    let config = settings(&dir, false);
    for pointer in ["/session_id", "/tool_use_id", "/turn_id"] {
        let mut native = payload(Host::Codex);
        *native.pointer_mut(pointer).unwrap() = Value::Null;
        assert!(inspect(
            Host::Codex,
            serde_json::from_value(config.clone()).unwrap(),
            native
        )
        .is_err());
    }
    for (pointer, value) in [
        ("/agent_start_ticks", json!(0)),
        ("/scope/session_id", json!("wrong")),
        ("/runtime/process_ref", json!("wrong")),
        ("/qoder_single_turn_id", Value::Null),
    ] {
        let mut invalid = config.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(inspect(
            Host::Qoder,
            serde_json::from_value(invalid).unwrap(),
            payload(Host::Qoder)
        )
        .is_err());
    }
    assert!(!dir.0.join("received").exists());
}

#[test]
fn cli_uses_private_settings_and_emits_only_native_response() {
    let dir = Directory::new();
    let config = settings(&dir, false);
    let path = dir.0.join("settings.json");
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let _: Settings = read_settings(&path).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_aw-hook-cli"))
        .arg("codex")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&payload(Host::Codex)).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({})
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_settings(&path).is_err());
}

struct RunningHook {
    child: Child,
    marker: PathBuf,
}

impl Drop for RunningHook {
    fn drop(&mut self) {
        // Only the fixture writes this newly created directory's group marker.
        if let Ok(group) = fs::read_to_string(&self.marker) {
            if let Ok(group) = group.parse::<i32>() {
                if group > 1 {
                    unsafe { libc::kill(-group, libc::SIGKILL) };
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn termination_cancels_version_probe_and_scan_and_reclaims_the_group() {
    for (during_probe, signal) in [(true, libc::SIGINT), (false, libc::SIGTERM)] {
        let dir = Directory::new();
        let mut config = settings(&dir, false);
        config["provider"]["args"][1] = json!(format!(
            "import os,sys,pathlib,time\nif sys.argv[-1]=='--version' and not {during_probe}:\n print('agent-sec-cli 0.12.0');sys.exit(0)\nchild=os.fork()\nif child==0:\n time.sleep(60);os._exit(0)\npathlib.Path('group').write_text(str(os.getpid()))\npathlib.Path('descendant').write_text(str(child))\ntime.sleep(60)\n",
            during_probe = if during_probe { "True" } else { "False" }
        ));
        let path = dir.0.join("settings.json");
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut running = RunningHook {
            child: Command::new(env!("CARGO_BIN_EXE_aw-hook-cli"))
                .arg("codex")
                .arg(&path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
            marker: dir.0.join("group"),
        };
        running
            .child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&payload(Host::Codex)).unwrap())
            .unwrap();
        let descendant = dir.0.join("descendant");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !descendant.exists() {
            assert!(Instant::now() < deadline, "native child did not start");
            assert!(
                running.child.try_wait().unwrap().is_none(),
                "hook exited before test signal"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let group: i32 = fs::read_to_string(&running.marker)
            .unwrap()
            .parse()
            .unwrap();
        let descendant: u32 = fs::read_to_string(descendant).unwrap().parse().unwrap();
        assert_eq!(unsafe { libc::kill(running.child.id() as i32, signal) }, 0);
        let deadline = Instant::now() + Duration::from_secs(3);
        let status = loop {
            if let Some(status) = running.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "hook failed to finish cancellation"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            status.code(),
            Some(1),
            "signal must reach normal failure cleanup"
        );
        for pid in [group as u32, descendant] {
            if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
                let state = stat
                    .rsplit_once(')')
                    .unwrap()
                    .1
                    .split_whitespace()
                    .next()
                    .unwrap();
                assert!(
                    matches!(state, "Z" | "X"),
                    "native task remains executable: {stat}"
                );
            }
        }
        // Successful cleanup already reaped the leader; never kill a reused PGID.
        fs::remove_file(&running.marker).unwrap();
    }
}
