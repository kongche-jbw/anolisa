//! Real bounded subprocess exchange against a synthetic native CLI, not SecCore.

use aw_contracts::{canonical, Registry};
use aw_core::ports::ProviderHost;
use aw_sec_host::{Config, FilePin, Limits, PinState, SecHost};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    path: PathBuf,
    config: Config,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/sec-host-tests");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let path = fs::canonicalize(path).unwrap();
        let script = path.join("native.py");
        fs::write(&script, include_str!("native_fixture.py")).unwrap();
        let program = fs::canonicalize("/usr/bin/python3").unwrap();
        let config = Config {
            provider_id: "test-sec".into(),
            provider_version: "fixture-1".into(),
            program_sha256: canonical::digest(&fs::read(&program).unwrap()),
            program,
            cwd: path.clone(),
            args: vec![script.to_str().unwrap().into()],
            environment: BTreeMap::from([
                ("MODE".into(), mode.into()),
                ("EXPLICIT".into(), "exact-value".into()),
            ]),
            pins: vec![FilePin {
                path: script,
                state: PinState::Sha256(canonical::digest(include_bytes!("native_fixture.py"))),
            }],
            limits: Limits {
                timeout_ms: 2000,
                input_bytes: 65536,
                output_bytes: 65536,
                stderr_bytes: 1024,
            },
        };
        Self { path, config }
    }

    fn called(&self) -> bool {
        self.path.join("called.json").exists()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn invocation(host: &SecHost, text: &str) -> Value {
    let registry = Registry::new().unwrap();
    let fixture: Value =
        canonical::parse(include_bytes!("../../../tests/fixtures/contracts.json")).unwrap();
    let mut invocation = fixture["capability-invocation-v1"].clone();
    let descriptor = host.descriptor("test-sec").unwrap();
    for key in ["provider_id", "provider_version", "manifest_digest"] {
        invocation[key] = descriptor[key].clone();
    }
    for key in ["capability", "input_schema", "output_schema"] {
        invocation[key] = descriptor["capabilities"][0][key].clone();
    }
    invocation["input"] = fixture["security-content-inspect-input-v2"].clone();
    invocation["input"]["artifact"]["content"] = json!(text);
    invocation["input"]["artifact"]["digest"] = json!(canonical::digest(text.as_bytes()));
    invocation["input_digest"] = json!(canonical::document_digest(&invocation["input"]).unwrap());
    invocation["deadline_at_ms"] = json!(now() + 10_000);
    invocation["budget"] = json!({"input_bytes":65536,"output_bytes":65536,"wall_time_ms":2000});
    registry
        .validate("capability-invocation-v1", &invocation)
        .unwrap();
    invocation
}

#[test]
fn exact_stdio_environment_and_receipt_binding_are_preserved() {
    let fixture = Fixture::new("clean");
    let mut host = SecHost::new(fixture.config.clone()).unwrap();
    let input = "中文\n trailing space \t";
    let mut invocation = invocation(&host, input);
    invocation["input"]["constraints"]["include_low_confidence"] = json!(true);
    invocation["input_digest"] = json!(canonical::document_digest(&invocation["input"]).unwrap());
    let result = host.invoke(&invocation).unwrap();
    Registry::new()
        .unwrap()
        .validate_result(&invocation, &result.receipt, result.output.as_ref())
        .unwrap();
    assert_eq!(result.receipt["disposition"], "produced");
    assert_eq!(
        host.descriptor("test-sec").unwrap()["guarantee"],
        "declared"
    );
    for key in [
        "scope",
        "plan_ref",
        "input_digest",
        "invocation_id",
        "manifest_digest",
    ] {
        assert_eq!(result.receipt[key], invocation[key]);
    }
    let called: Value =
        serde_json::from_slice(&fs::read(fixture.path.join("called.json")).unwrap()).unwrap();
    assert_eq!(called["stdin"], input);
    assert_eq!(called["cwd"], fixture.path.to_str().unwrap());
    assert_eq!(
        called["args"],
        json!([
            "scan-pii",
            "--stdin",
            "--format",
            "json",
            "--source",
            "tool_output",
            "--include-low-confidence"
        ])
    );
    assert_eq!(called["environment"]["EXPLICIT"], "exact-value");
    for name in ["HOME", "PATH", "HTTP_PROXY", "HTTPS_PROXY"] {
        assert!(called["environment"].get(name).is_none());
    }
    assert!(!serde_json::to_string(&result.receipt)
        .unwrap()
        .contains(input));
}

#[test]
fn invalid_binding_input_or_deadline_never_launches_a_scan() {
    for (pointer, value) in [
        ("/provider_id", json!("other")),
        ("/provider_version", json!("other")),
        ("/manifest_digest", json!("0".repeat(64))),
        ("/input_digest", json!("0".repeat(64))),
        ("/deadline_at_ms", json!(1)),
        ("/budget/input_bytes", json!(1)),
        ("/capability", json!("security.code.inspect/v2")),
        ("/scope/session_id", json!(null)),
    ] {
        let fixture = Fixture::new("clean");
        let mut host = SecHost::new(fixture.config.clone()).unwrap();
        let mut invocation = invocation(&host, "hello");
        *invocation.pointer_mut(pointer).unwrap() = value;
        let result = host.invoke(&invocation);
        assert!(
            result.is_err() || result.unwrap().receipt["disposition"] == "failed",
            "{pointer}"
        );
        assert!(!fixture.called(), "{pointer}");
    }
}

#[test]
fn native_errors_limits_and_malformed_output_never_leak_diagnostics() {
    for mode in ["fail", "stdout_limit", "stderr_limit", "malformed", "sleep"] {
        let fixture = Fixture::new(mode);
        let mut host = SecHost::new(fixture.config.clone()).unwrap();
        let mut invocation = invocation(&host, "hello");
        if mode == "sleep" {
            invocation["budget"]["wall_time_ms"] = json!(50);
        }
        let result = host.invoke(&invocation).unwrap();
        assert_eq!(result.receipt["disposition"], "failed", "{mode}");
        assert!(result.output.is_none());
        let encoded = serde_json::to_string(&result.receipt).unwrap();
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("hello"));
    }
}

#[test]
fn program_selected_files_and_explicit_absence_are_pinned_before_launch() {
    for kind in ["program", "present", "absent"] {
        let mut fixture = Fixture::new("clean");
        let pinned = fixture.path.join("rules.yaml");
        if kind == "program" {
            let program = fixture.path.join("python");
            fs::copy(&fixture.config.program, &program).unwrap();
            fixture.config.program = program;
        } else {
            let state = if kind == "present" {
                fs::write(&pinned, "original").unwrap();
                PinState::Sha256(canonical::digest(b"original"))
            } else {
                PinState::Absent
            };
            fixture.config.pins.push(FilePin {
                path: pinned.clone(),
                state,
            });
        }
        let mut host = SecHost::new(fixture.config.clone()).unwrap();
        if kind == "program" {
            fs::write(&fixture.config.program, "changed").unwrap();
        } else {
            fs::write(&pinned, "changed").unwrap();
        }
        let result = host.invoke(&invocation(&host, "hello")).unwrap();
        assert_eq!(result.receipt["disposition"], "failed");
        assert!(!fixture.called());
    }
}

#[test]
fn changed_rules_after_execution_cannot_produce_a_successful_receipt() {
    let mut fixture = Fixture::new("mutate");
    let rules = fixture.path.join("rules.yaml");
    fs::write(&rules, "original").unwrap();
    fixture.config.pins.push(FilePin {
        path: rules,
        state: PinState::Sha256(canonical::digest(b"original")),
    });
    let mut host = SecHost::new(fixture.config.clone()).unwrap();
    let result = host.invoke(&invocation(&host, "hello")).unwrap();
    assert!(fixture.called());
    assert_eq!(result.receipt["disposition"], "failed");
    assert_eq!(result.receipt["error_code"], "pinned_file_changed");
}

#[test]
fn constructor_rejects_untrusted_pins_and_native_version_mismatch() {
    let fixture = Fixture::new("clean");
    let mut config = fixture.config.clone();
    config.program_sha256 = "0".repeat(64);
    assert!(SecHost::new(config).is_err());
    let fifo = fixture.path.join("untrusted.fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let mut config = fixture.config.clone();
    config.pins.push(FilePin {
        path: fifo,
        state: PinState::Sha256("0".repeat(64)),
    });
    assert!(SecHost::new(config).is_err());
    assert!(!fixture.path.join("version-called").exists());
    let mut config = fixture.config.clone();
    config
        .environment
        .insert("VERSION".into(), "agent-sec-cli 9.0.0".into());
    assert!(SecHost::new(config).is_err());
    let mut config = fixture.config.clone();
    config.cwd = PathBuf::from("relative");
    assert!(SecHost::new(config).is_err());
    let mut config = fixture.config.clone();
    config.pins.push(config.pins[0].clone());
    assert!(SecHost::new(config).is_err());
    let mut config = fixture.config.clone();
    config.pins.push(FilePin {
        path: fixture.path.clone(),
        state: PinState::Sha256("0".repeat(64)),
    });
    assert!(SecHost::new(config).is_err());
}

#[test]
fn projected_output_must_fit_the_independent_invocation_budget() {
    let fixture = Fixture::new("clean");
    let mut host = SecHost::new(fixture.config.clone()).unwrap();
    let mut invocation = invocation(&host, "hello");
    invocation["budget"]["output_bytes"] = json!(1);
    let result = host.invoke(&invocation).unwrap();
    assert!(fixture.called());
    assert_eq!(result.receipt["disposition"], "failed");
    assert_eq!(result.receipt["error_code"], "output_budget_exceeded");
    assert!(result.output.is_none());
}

#[test]
fn every_launch_setting_is_bound_to_the_manifest_and_config_is_strict() {
    let fixture = Fixture::new("clean");
    let original = SecHost::new(fixture.config.clone()).unwrap();
    let digest = &original.descriptor("test-sec").unwrap()["manifest_digest"];
    for variant in 0..4 {
        let mut config = fixture.config.clone();
        match variant {
            0 => {
                config.environment.insert("ANOTHER".into(), "value".into());
            }
            1 => config.limits.timeout_ms += 1,
            2 => config.provider_version = "fixture-2".into(),
            _ => config.pins.push(FilePin {
                path: fixture.path.join("absent.yaml"),
                state: PinState::Absent,
            }),
        }
        let changed = SecHost::new(config).unwrap();
        assert_ne!(
            &changed.descriptor("test-sec").unwrap()["manifest_digest"],
            digest
        );
    }
    let mut wire = serde_json::to_value(&fixture.config).unwrap();
    assert_eq!(
        wire["pins"][0]["state"],
        json!({"sha256":canonical::digest(include_bytes!("native_fixture.py"))})
    );
    wire["unexpected"] = json!(true);
    assert!(serde_json::from_value::<Config>(wire).is_err());
}

#[test]
fn descendants_are_stopped_on_both_pipe_timeout_and_success() {
    for mode in ["child_pipe", "child_closed"] {
        let fixture = Fixture::new(mode);
        let mut host = SecHost::new(fixture.config.clone()).unwrap();
        let invocation = invocation(&host, "hello");
        let result = host.invoke(&invocation).unwrap();
        assert_eq!(
            result.receipt["disposition"],
            if mode == "child_closed" {
                "produced"
            } else {
                "failed"
            }
        );
        let child = fs::read_to_string(fixture.path.join("child.pid")).unwrap();
        let stat = fs::read_to_string(format!("/proc/{child}/stat"));
        if let Ok(stat) = stat {
            let state = stat
                .rsplit_once(')')
                .unwrap()
                .1
                .trim_start()
                .chars()
                .next()
                .unwrap();
            assert_eq!(state, 'Z', "owned descendant must not remain executable");
        }
    }
}

#[test]
fn nonreading_stdin_is_bounded_by_the_same_invocation_deadline() {
    let fixture = Fixture::new("no_read");
    let mut host = SecHost::new(fixture.config.clone()).unwrap();
    let invocation = invocation(&host, &"x".repeat(60_000));
    let result = host.invoke(&invocation).unwrap();
    assert_eq!(result.receipt["disposition"], "failed");
    assert!(fixture.path.join("no_read.started").exists());
    assert!(!fixture.called());
}
