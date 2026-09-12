//! Real native exchange and shared launch provenance preserve the Host contract.
#![cfg(target_os = "linux")]

use aw_contracts::canonical;
use aw_core::ports::{Cancellation, NeverCancel};
use aw_host_process::{run, Config, FilePin, Limits, PinState};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/process-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn config(&self) -> Config {
        Config {
            provider_id: "fixture-provider".into(),
            provider_version: "0.1.0".into(),
            program: "/bin/cat".into(),
            program_sha256: canonical::digest(&fs::read("/bin/cat").unwrap()),
            cwd: self.0.clone(),
            args: vec![],
            environment: BTreeMap::new(),
            pins: vec![],
            limits: Limits {
                timeout_ms: 2000,
                input_bytes: 1024,
                output_bytes: 1024,
                stderr_bytes: 1024,
            },
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn shared_runner_exchanges_exact_bytes_and_preserves_manifest_shape() {
    let directory = Directory::new();
    let config = directory.config();
    config.validate().unwrap();
    config.check_pins().unwrap();
    let original = json!({"format":1,"config":serde_json::to_value(&config).unwrap(),
        "native_cli_version":"0.12.0","protocol_profile":"agent-sec.scan-pii/v1",
        "operation":"security.content.inspect/v2"});
    assert_eq!(
        config
            .manifest(
                "0.12.0",
                "agent-sec.scan-pii/v1",
                "security.content.inspect/v2"
            )
            .unwrap(),
        original
    );
    let stdin = "original 字节\nwithout added newline".as_bytes();
    let output = run(&config, &[], stdin, 2000, &NeverCancel).unwrap();
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, stdin);
    config.check_pins().unwrap();
}

#[test]
fn selected_pin_drift_retains_existing_failure_code() {
    let directory = Directory::new();
    let mut config = directory.config();
    let pin = directory.0.join("selected-config");
    fs::write(&pin, b"original").unwrap();
    config.pins.push(FilePin {
        path: pin.clone(),
        state: PinState::Sha256(canonical::digest(b"original")),
    });
    config.validate().unwrap();
    config.check_pins().unwrap();
    fs::write(pin, b"changed").unwrap();
    assert_eq!(
        config.check_pins().unwrap_err().code(),
        "pinned_file_changed"
    );
}

struct Cancelled;
impl Cancellation for Cancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

#[test]
fn cancellation_is_observed_before_spawn() {
    let directory = Directory::new();
    let mut config = directory.config();
    config.program = directory.0.join("unavailable-program");
    let result = run(&config, &[], b"input", 2000, &Cancelled);
    assert_eq!(result.err().unwrap().code(), "provider_cancelled");
}
