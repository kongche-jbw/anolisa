#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use aw_contracts::common::{BoundedName, BoundedOpaque, Digest, IdempotencyKey, TargetRef};
use aw_contracts::ids::{
    ActorId, AgentSessionId, EnvironmentId, ExecutionContextId, ProviderInvocationId, ToolUseId,
    TurnId,
};
use aw_contracts::provider::{
    CapabilityInvocation, ExecutionScope, ProviderInvocationBudget, ProviderPayload,
    ProviderSelection, VersionedSchema,
};
use aw_provider_host::canonical_json_v1_bytes;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

const MANIFEST: &str = r#"api_version = "providers.agentic-os.sh/v1"
provider_id = "cli-fixture"
provider_version = "1.0.0"
driver = "exec-json/v1"
lifecycle = "one_shot"

[executable]
command = "./fake-provider.sh"
args = []

[limits]
wall_time_ms = 1000
input_bytes = 1048576
output_bytes = 1048576

[permissions]
network = "none"
inherit_environment = false
filesystem_read = []
filesystem_write = []

[data]
reads = ["model_visible_context"]
writes = []
sensitivity = "inherits_input"
retention = "none"
telemetry = "disabled"

[[capabilities]]
capability = "context.projection.prepare/v1"
input_contract = { schema = "context.projection.prepare.input/v1", resource = "schema.json", sha256 = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a" }
output_contract = { schema = "context.projection.prepare.output/v1", resource = "schema.json", sha256 = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a" }
native_input = { resource = "schema.json", sha256 = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a" }
native_output = { resource = "schema.json", sha256 = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a" }
authority = "advise"
scopes = ["tool_call"]

[capabilities.codec]
kind = "json-map/v1"

[capabilities.codec.request]

[[capabilities.codec.request.fields]]
target = "/content"
source = { kind = "input", pointer = "/content" }
on_missing = "reject"

[capabilities.codec.response.disposition]
source = "/disposition"
on_unknown = "fail"

[capabilities.codec.response.disposition.values]
applied = "produced"

[[capabilities.codec.response.output_fields]]
target = "/content"
source = { kind = "response", pointer = "/output" }
when_disposition = ["produced"]
"#;

#[test]
fn headless_provider_cli_lists_and_invokes_without_a_daemon() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("provider.toml");
    let executable = directory.path().join("fake-provider.sh");
    let invocation_file = directory.path().join("invocation.json");
    fs::write(&manifest, MANIFEST).unwrap();
    fs::write(directory.path().join("schema.json"), "{}").unwrap();
    fs::write(
        &executable,
        "#!/bin/sh\nIFS= read -r payload || true\nprintf '%s' '{\"disposition\":\"applied\",\"output\":\"compressed\"}'\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let selection = selection("cli-fixture", "1.0.0", MANIFEST.as_bytes());
    fs::write(
        &invocation_file,
        serde_json::to_vec(&invocation(
            json!({"content": "large tool output"}),
            selection,
        ))
        .unwrap(),
    )
    .unwrap();

    let list = Command::new(env!("CARGO_BIN_EXE_aw-provider-host"))
        .args(["--output", "jsonl", "list", "--manifest"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let listed: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(
        listed.pointer("/capabilities/0/guarantee"),
        Some(&json!("declared_not_enforced"))
    );

    let invoke = Command::new(env!("CARGO_BIN_EXE_aw-provider-host"))
        .args(["--output", "jsonl", "invoke", "--manifest"])
        .arg(&manifest)
        .args(["--invocation-file"])
        .arg(&invocation_file)
        .output()
        .unwrap();
    assert!(
        invoke.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&invoke.stdout),
        String::from_utf8_lossy(&invoke.stderr),
    );
    let invoked: Value = serde_json::from_slice(&invoke.stdout).unwrap();
    assert_eq!(
        invoked.pointer("/result/invocation/receipt/disposition"),
        Some(&json!("produced"))
    );
    assert_eq!(
        invoked.pointer("/result/invocation/outcome/output/body/content"),
        Some(&json!("compressed"))
    );
    let receipt = invoked
        .pointer("/result/invocation/receipt")
        .unwrap()
        .to_string();
    assert!(!receipt.contains("compressed"));
}

#[test]
fn tokenless_manifest_admits_and_maps_with_a_stub_executable() {
    let repository = repository_root();
    let package = repository.join("providers/tokenless");
    let manifest = package.join("provider.toml");
    let manifest_bytes = fs::read(&manifest).unwrap();
    let manifest_document: toml::Value =
        toml::from_str(std::str::from_utf8(&manifest_bytes).expect("Tokenless manifest is UTF-8"))
            .unwrap();
    let provider_version = manifest_document
        .get("provider_version")
        .and_then(toml::Value::as_str)
        .expect("Tokenless manifest declares provider_version");
    let canonical_body: Value = serde_json::from_slice(
        &fs::read(package.join("fixtures/context-projection-prepare.json")).unwrap(),
    )
    .unwrap();

    let directory = tempfile::tempdir().unwrap();
    let executable_root = directory.path().join("bin");
    let executable = executable_root.join("tokenless");
    let invocation_file = directory.path().join("invocation.json");
    fs::create_dir(&executable_root).unwrap();
    fs::write(
        &executable,
        concat!(
            "#!/bin/sh\n",
            "IFS= read -r payload || true\n",
            "printf '%s' '",
            r#"{"protocol_version":1,"output":"compressed by stub","output_media_type":"text/plain","disposition":"applied","reversibility":"lossless","before_tokens":10,"after_tokens":2,"stash_keys":[],"tokenizer_id":"stub-v1","compressor_chain":["stub"]}"#,
            "'\n",
        ),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        &invocation_file,
        serde_json::to_vec(&invocation(
            canonical_body,
            selection("tokenless", provider_version, &manifest_bytes),
        ))
        .unwrap(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_aw-provider-host"))
        .args(["--output", "jsonl", "invoke", "--manifest"])
        .arg(&manifest)
        .args(["--executable-root"])
        .arg(&executable_root)
        .args(["--invocation-file"])
        .arg(&invocation_file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value.pointer("/result/invocation/receipt/disposition"),
        Some(&json!("produced"))
    );
    assert_eq!(
        value.pointer("/result/invocation/outcome/output/body/candidate/content"),
        Some(&json!("compressed by stub"))
    );
    let receipt = value.pointer("/result/invocation/receipt").unwrap();
    let meters = receipt
        .pointer("/meters")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(meters.len(), 2);
    assert!(meters
        .iter()
        .all(|meter| { meter.pointer("/method").and_then(Value::as_str) == Some("heuristic-v1") }));
    assert_eq!(
        receipt.pointer("/input_schema/id").and_then(Value::as_str),
        Some("context.projection.prepare.input")
    );
    assert!(receipt
        .pointer("/input_digest")
        .and_then(Value::as_str)
        .is_some_and(|digest| digest.len() == 64));
}

#[test]
#[ignore = "requires src/tokenless/target/debug/tokenless"]
fn tokenless_manifest_runs_through_the_generic_headless_host() {
    let repository = repository_root();
    let package = repository.join("providers/tokenless");
    let manifest = package.join("provider.toml");
    let executable_root = repository.join("src/tokenless/target/debug");
    let executable = executable_root.join("tokenless");
    assert!(
        executable.is_file(),
        "build Tokenless first: cd {} && cargo build --bin tokenless",
        repository.join("src/tokenless").display()
    );

    let canonical_body: Value = serde_json::from_slice(
        &fs::read(package.join("fixtures/context-projection-prepare.json")).unwrap(),
    )
    .unwrap();
    let manifest_bytes = fs::read(&manifest).unwrap();
    let manifest_document: toml::Value =
        toml::from_str(std::str::from_utf8(&manifest_bytes).expect("Tokenless manifest is UTF-8"))
            .unwrap();
    let provider_version = manifest_document
        .get("provider_version")
        .and_then(toml::Value::as_str)
        .expect("Tokenless manifest declares provider_version");
    let directory = tempfile::tempdir().unwrap();
    let invocation_file = directory.path().join("invocation.json");
    fs::write(
        &invocation_file,
        serde_json::to_vec(&invocation(
            canonical_body,
            selection("tokenless", provider_version, &manifest_bytes),
        ))
        .unwrap(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_aw-provider-host"))
        .args(["--output", "jsonl", "invoke", "--manifest"])
        .arg(&manifest)
        .args(["--executable-root"])
        .arg(&executable_root)
        .args(["--invocation-file"])
        .arg(&invocation_file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value.pointer("/result/invocation/receipt/disposition"),
        Some(&json!("produced"))
    );
    assert!(value
        .pointer("/result/invocation/outcome/output/body/candidate/content")
        .and_then(Value::as_str)
        .is_some_and(|content| !content.is_empty()));
    let receipt = value.pointer("/result/invocation/receipt").unwrap().clone();
    assert!(!receipt
        .to_string()
        .contains("scheduler trace retained only for operator diagnostics"));
    let meters = receipt
        .pointer("/meters")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(meters.len(), 2);
    assert!(meters
        .iter()
        .all(|meter| { meter.pointer("/method").and_then(Value::as_str) == Some("heuristic-v1") }));
    assert!(!directory.path().join("tokenless").exists());
}

fn invocation(body: Value, provider: ProviderSelection) -> CapabilityInvocation {
    let encoded = canonical_json_v1_bytes(&body).unwrap();
    CapabilityInvocation {
        invocation_id: ProviderInvocationId::new(),
        provider,
        capability: schema("context.projection.prepare"),
        scope: ExecutionScope {
            target: TargetRef {
                kind: BoundedName::new("host").unwrap(),
                authority: BoundedName::new("local").unwrap(),
                identifier: BoundedOpaque::new("fixture-host").unwrap(),
            },
            environment_id: EnvironmentId::new(),
            execution_context_id: ExecutionContextId::new(),
            actor_id: ActorId::new(),
            agent_session_id: Some(AgentSessionId::new()),
            work_id: None,
            attempt_id: None,
            turn_id: Some(TurnId::new()),
            tool_use_id: Some(ToolUseId::new()),
        },
        binding_id: None,
        idempotency_key: IdempotencyKey::new("cli-provider-fixture").unwrap(),
        policy_revision: 1,
        deadline_at_ms: now_ms() + 60_000,
        budget: ProviderInvocationBudget {
            wall_time_ms: 5_000,
            output_bytes: 1_048_576,
        },
        input: ProviderPayload {
            schema: schema("context.projection.prepare.input"),
            digest: Digest::parse(format!("{:x}", Sha256::digest(encoded))).unwrap(),
            body,
        },
    }
}

fn selection(provider_id: &str, provider_version: &str, manifest: &[u8]) -> ProviderSelection {
    ProviderSelection {
        provider_id: BoundedName::new(provider_id).unwrap(),
        provider_version: BoundedName::new(provider_version).unwrap(),
        manifest_digest: Digest::parse(format!("{:x}", Sha256::digest(manifest))).unwrap(),
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .unwrap()
        .to_path_buf()
}

fn schema(id: &str) -> VersionedSchema {
    VersionedSchema {
        id: BoundedName::new(id).unwrap(),
        version: 1,
    }
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}
