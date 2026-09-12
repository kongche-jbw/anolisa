//! Real bounded subprocess exchange against a synthetic native CLI, not Tokenless.

use aw_contracts::{canonical, Registry};
use aw_core::ports::ProviderHost;
use aw_tokenless_host::{Config, FilePin, Limits, PinState, TokenlessHost};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

pub struct Fixture {
    pub path: PathBuf,
    pub config: Config,
}

impl Fixture {
    pub fn new(mode: &str) -> Self {
        let parent =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tokenless-host-tests");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let path = fs::canonicalize(path).unwrap();
        let script = path.join("native.py");
        fs::write(&script, include_str!("../native_fixture.py")).unwrap();
        let program = fs::canonicalize("/usr/bin/python3").unwrap();
        let config = Config {
            provider_id: "test-tokenless".into(),
            provider_version: "fixture-1".into(),
            program_sha256: canonical::digest(&fs::read(&program).unwrap()),
            program,
            cwd: path.clone(),
            args: vec![script.to_str().unwrap().into()],
            environment: BTreeMap::from([
                ("MODE".into(), mode.into()),
                ("EXPLICIT".into(), "exact-value".into()),
                ("TOKENLESS_STATS_ENABLED".into(), "0".into()),
                ("TOKENLESS_SLS_ENABLED".into(), "0".into()),
                ("TOKENLESS_COMPRESSION_ENABLED".into(), "1".into()),
            ]),
            pins: vec![FilePin {
                path: script,
                state: PinState::Sha256(canonical::digest(include_bytes!("../native_fixture.py"))),
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

    pub fn called(&self) -> bool {
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

pub fn invocation(host: &TokenlessHost, text: &str) -> Value {
    let registry = Registry::new().unwrap();
    let fixture: Value =
        canonical::parse(include_bytes!("../../../../tests/fixtures/contracts.json")).unwrap();
    let mut invocation = fixture["capability-invocation-v1"].clone();
    let descriptor = host.descriptor("test-tokenless").unwrap();
    for key in ["provider_id", "provider_version", "manifest_digest"] {
        invocation[key] = descriptor[key].clone();
    }
    for key in ["capability", "input_schema", "output_schema"] {
        invocation[key] = descriptor["capabilities"][0][key].clone();
    }
    invocation["input"] = fixture["context-projection-prepare-input-v2"].clone();
    invocation["input"]["artifact"]["content"] = json!(text);
    invocation["input"]["artifact"]["tool_name"] = json!("Bash");
    invocation["input"]["artifact"]["origin"] = json!("command_output");
    invocation["input"]["constraints"]["accepted_reversibility"] = json!(["unrecoverable"]);
    invocation["input"]["artifact"]["digest"] = json!(canonical::digest(text.as_bytes()));
    invocation["input_digest"] = json!(canonical::document_digest(&invocation["input"]).unwrap());
    invocation["deadline_at_ms"] = json!(now() + 10_000);
    invocation["budget"] = json!({"input_bytes":65536,"output_bytes":65536,"wall_time_ms":2000});
    registry
        .validate("capability-invocation-v1", &invocation)
        .unwrap();
    invocation
}
