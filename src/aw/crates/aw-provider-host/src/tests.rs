#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aw_contracts::common::{BoundedName, BoundedOpaque, Digest, IdempotencyKey, TargetRef};
use aw_contracts::ids::{
    ActorId, AgentSessionId, EnvironmentId, ExecutionContextId, ProviderInvocationId, ToolUseId,
    TurnId,
};
use aw_contracts::provider::{
    CapabilityInvocation, ExecutionScope, ProviderDisposition, ProviderInvocationBudget,
    ProviderPayload, ProviderSelection, VersionedSchema,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use super::{
    canonical_json_v1_bytes, ProviderAdmissionOptions, ProviderCatalog, ProviderHostError,
    ProviderManifestSource,
};

const FAKE_PROVIDER: &str = r#"#!/bin/sh
mode="$1"
IFS= read -r payload || true
case "$mode" in
  success)
    printf '%s' '{"disposition":"applied","output":"compressed","before_tokens":1200,"after_tokens":180,"meter_method":"fixture-estimator-v1"}'
    ;;
  bypass)
    printf '%s' '{"disposition":"bypass","output":"unchanged","before_tokens":20,"after_tokens":20,"meter_method":"fixture-estimator-v1"}'
    ;;
  echo_request)
    printf '{"disposition":"applied","output":%s,"before_tokens":1200,"after_tokens":180,"meter_method":"fixture-estimator-v1"}' "$payload"
    ;;
  timeout)
    printf '%s' 'credential-secret' >&2
    /bin/sleep 2
    ;;
  orphan_pipe)
    printf '%s' 'started' > ./orphan-started
    (
      trap '' TERM
      /bin/sleep 1
      printf '%s' 'survived' > ./orphan-survived
    ) &
    ;;
  crash)
    printf '%s' 'credential-secret' >&2
    exit 7
    ;;
  malformed)
    printf '%s' '{not-json'
    ;;
  mismatched_response)
    printf '%s' '{"protocol_version":2,"disposition":"applied","output":"compressed","before_tokens":1200,"after_tokens":180,"meter_method":"fixture-estimator-v1"}'
    ;;
  oversize)
    i=0
    while [ "$i" -lt 2048 ]; do
      printf x
      i=$((i + 1))
    done
    ;;
esac
"#;

struct Fixture {
    directory: tempfile::TempDir,
    manifest: PathBuf,
    state_root: PathBuf,
}

impl Fixture {
    fn new(mode: &str, output_bytes: u64) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-provider.sh");
        fs::write(&executable, FAKE_PROVIDER).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        write_schemas(directory.path());
        let manifest = directory.path().join("provider.toml");
        fs::write(
            &manifest,
            manifest_text(
                "fixture-provider",
                "context.projection.prepare/v1",
                mode,
                output_bytes,
            ),
        )
        .unwrap();
        let state_root = directory.path().join("state");
        fs::create_dir(&state_root).unwrap();
        Self {
            directory,
            manifest,
            state_root,
        }
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog::discover(
            ProviderManifestSource::File(self.manifest.clone()),
            &ProviderAdmissionOptions::default(),
        )
        .unwrap()
    }

    fn replace_schema(&self, resource: &str, schema: serde_json::Value) {
        let bytes = serde_json::to_vec(&schema).unwrap();
        fs::write(self.directory.path().join(resource), &bytes).unwrap();
        let old = format!(
            "resource = \"{resource}\", sha256 = \"44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a\""
        );
        let new = format!(
            "resource = \"{resource}\", sha256 = \"{}\"",
            sha256_digest(&bytes).as_str()
        );
        let manifest = fs::read_to_string(&self.manifest).unwrap();
        assert!(
            manifest.contains(&old),
            "fixture schema resource is present"
        );
        fs::write(&self.manifest, manifest.replacen(&old, &new, 1)).unwrap();
    }

    fn correlate_response(&self, request_pointer: &str, response_pointer: &str) {
        let marker = "[capabilities.codec.response.disposition]\n";
        let correlation = format!(
            "[[capabilities.codec.response.correlations]]\nrequest_pointer = \"{request_pointer}\"\nresponse_pointer = \"{response_pointer}\"\n\n{marker}"
        );
        let manifest = fs::read_to_string(&self.manifest).unwrap();
        assert!(
            manifest.contains(marker),
            "response mapping marker is present"
        );
        fs::write(&self.manifest, manifest.replacen(marker, &correlation, 1)).unwrap();
    }
}

fn manifest_text(provider_id: &str, capability: &str, mode: &str, output_bytes: u64) -> String {
    format!(
        r#"api_version = "providers.agentic-os.sh/v1"
provider_id = "{provider_id}"
provider_version = "1.2.3"
driver = "exec-json/v1"
lifecycle = "one_shot"

[executable]
command = "./fake-provider.sh"
args = ["{mode}"]

[executable.environment]
FEATURE_ENABLED = "false"
PROVIDER_DATA = "{{provider_state_dir}}"

[limits]
wall_time_ms = 1500
input_bytes = 1048576
output_bytes = {output_bytes}

[permissions]
network = "none"
inherit_environment = false
filesystem_read = []
filesystem_write = ["{{provider_state_dir}}"]

[data]
reads = ["model_visible_context"]
writes = ["local_provider_state"]
sensitivity = "inherits_input"
retention = "provider_managed"
telemetry = "disabled"

[[capabilities]]
capability = "{capability}"
input_contract = {{ schema = "context.projection.prepare.input/v1", resource = "canonical-input.schema.json", sha256 = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a" }}
output_contract = {{ schema = "context.projection.prepare.output/v1", resource = "canonical-output.schema.json", sha256 = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a" }}
native_input = {{ resource = "native-input.schema.json", sha256 = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a" }}
native_output = {{ resource = "native-output.schema.json", sha256 = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a" }}
authority = "advise"
scopes = ["tool_call"]

[capabilities.codec]
kind = "json-map/v1"

[capabilities.codec.request]

[[capabilities.codec.request.fields]]
target = "/protocol_version"
source = {{ kind = "const", value = 1 }}
on_missing = "reject"

[[capabilities.codec.request.fields]]
target = "/content"
source = {{ kind = "input", pointer = "/content" }}
on_missing = "reject"

[[capabilities.codec.request.fields]]
target = "/agent_session_id"
source = {{ kind = "scope", field = "agent_session_id" }}
on_missing = "reject"

[capabilities.codec.response.disposition]
source = "/disposition"
on_unknown = "fail"

[capabilities.codec.response.disposition.values]
applied = "produced"
bypass = "bypassed"
failed = "failed"

[[capabilities.codec.response.output_fields]]
target = "/content"
source = {{ kind = "response", pointer = "/output" }}
when_disposition = ["produced"]

[[capabilities.codec.response.meters]]
meter_id = "input_tokens"
unit = "tokens"
measurement_kind = "estimate"
method_pointer = "/meter_method"
value_pointer = "/before_tokens"

[[capabilities.codec.response.meters]]
meter_id = "output_tokens"
unit = "tokens"
measurement_kind = "estimate"
method_pointer = "/meter_method"
value_pointer = "/after_tokens"
"#
    )
}

fn write_schemas(directory: &std::path::Path) {
    for resource in [
        "canonical-input.schema.json",
        "canonical-output.schema.json",
        "native-input.schema.json",
        "native-output.schema.json",
    ] {
        fs::write(directory.join(resource), "{}").unwrap();
    }
}

fn schema(id: &str) -> VersionedSchema {
    VersionedSchema {
        id: BoundedName::new(id).unwrap(),
        version: 1,
    }
}

fn invocation(catalog: &ProviderCatalog) -> CapabilityInvocation {
    let provider = catalog.descriptors()[0];
    invocation_for_provider(catalog, provider.provider_id.as_str())
}

fn invocation_for_provider(catalog: &ProviderCatalog, provider_id: &str) -> CapabilityInvocation {
    let provider = catalog
        .descriptors()
        .into_iter()
        .find(|provider| provider.provider_id.as_str() == provider_id)
        .unwrap();
    let body = json!({
        "protocol_version": 1,
        "content": "large tool output",
        "agent_id": "fixture",
        "seam": "post_tool"
    });
    let encoded = canonical_json_v1_bytes(&body).unwrap();
    CapabilityInvocation {
        invocation_id: ProviderInvocationId::new(),
        provider: ProviderSelection {
            provider_id: provider.provider_id.clone(),
            provider_version: provider.provider_version.clone(),
            manifest_digest: provider.manifest_digest.clone(),
        },
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
        idempotency_key: IdempotencyKey::new("fixture-invocation").unwrap(),
        policy_revision: 1,
        deadline_at_ms: now_ms() + 5_000,
        budget: ProviderInvocationBudget {
            wall_time_ms: 3_000,
            output_bytes: 1_048_576,
        },
        input: ProviderPayload {
            schema: schema("context.projection.prepare.input"),
            digest: sha256_digest(&encoded),
            body,
        },
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

fn sha256_digest(bytes: &[u8]) -> Digest {
    Digest::parse(format!("{:x}", Sha256::digest(bytes))).unwrap()
}

#[test]
fn canonical_json_digest_ignores_object_insertion_order() {
    let left: serde_json::Value =
        serde_json::from_str(r#"{"z":1,"nested":{"b":2,"a":1},"items":[{"d":4,"c":3}]}"#).unwrap();
    let right: serde_json::Value =
        serde_json::from_str(r#"{"items":[{"c":3,"d":4}],"nested":{"a":1,"b":2},"z":1}"#).unwrap();
    let left = canonical_json_v1_bytes(&left).unwrap();
    let right = canonical_json_v1_bytes(&right).unwrap();
    assert_eq!(left, right);
    assert_eq!(sha256_digest(&left), sha256_digest(&right));
}

#[test]
fn graph_and_success_receipt_preserve_admitted_identity() {
    let fixture = Fixture::new("success", 4096);
    let catalog = fixture.catalog();
    let graph = catalog.capability_graph();
    assert_eq!(graph.capabilities.len(), 1);
    assert_eq!(
        graph.capabilities[0].provider_id.as_str(),
        "fixture-provider"
    );
    assert_eq!(
        graph.capabilities[0].capability.id.as_str(),
        "context.projection.prepare"
    );
    assert_eq!(
        graph.capabilities[0].guarantee,
        super::ProviderGuaranteeState::DeclaredNotEnforced
    );
    assert!(graph.capabilities[0]
        .permissions
        .filesystem_write
        .contains(&"{provider_state_dir}".to_owned()));

    let result = catalog
        .invoke(&invocation(&catalog), Some(&fixture.state_root))
        .unwrap();
    assert_eq!(result.receipt.disposition, ProviderDisposition::Produced);
    assert_eq!(result.receipt.provider_version.as_str(), "1.2.3");
    assert_eq!(result.receipt.meters.len(), 2);
    assert_eq!(result.receipt.meters[0].value, 1200);
    assert!(result.outcome.output.is_some());
    assert!(result.receipt.output_schema.is_some());
    assert!(result.receipt.output_digest.is_some());
    assert!(result.receipt.output_bytes.is_some());
    let receipt = serde_json::to_string(&result.receipt).unwrap();
    assert!(!receipt.contains("large tool output"));
    assert!(!receipt.contains("compressed"));
    assert!(fixture.state_root.join("fixture-provider").is_dir());
}

#[test]
fn bypass_is_not_reported_as_applied() {
    let fixture = Fixture::new("bypass", 4096);
    let catalog = fixture.catalog();
    let result = catalog
        .invoke(&invocation(&catalog), Some(&fixture.state_root))
        .unwrap();
    assert_eq!(
        result.receipt.disposition,
        ProviderDisposition::Bypassed,
        "{:#?}",
        result.receipt
    );
    assert!(result.outcome.output.is_none());
}

#[test]
fn json_map_builds_native_input_without_forwarding_the_canonical_body() {
    let fixture = Fixture::new("echo_request", 4096);
    let catalog = fixture.catalog();
    let result = catalog
        .invoke(&invocation(&catalog), Some(&fixture.state_root))
        .unwrap();
    let body = &result.outcome.output.as_ref().unwrap().body;
    let native = body.pointer("/content").unwrap();
    assert_eq!(native.pointer("/protocol_version"), Some(&json!(1)));
    assert_eq!(
        native.pointer("/content"),
        Some(&json!("large tool output"))
    );
    assert!(native.pointer("/agent_session_id").is_some());
    assert!(native.pointer("/agent_id").is_none());
    assert!(native.pointer("/seam").is_none());
}

#[test]
fn accepted_timeout_and_nonzero_exit_return_content_free_failed_receipts() {
    let timeout = Fixture::new("timeout", 4096);
    let catalog = timeout.catalog();
    let result = catalog
        .invoke(&invocation(&catalog), Some(&timeout.state_root))
        .unwrap();
    assert_failed_receipt(&result);
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("credential-secret"));

    let crash = Fixture::new("crash", 4096);
    let catalog = crash.catalog();
    let result = catalog
        .invoke(&invocation(&catalog), Some(&crash.state_root))
        .unwrap();
    assert_failed_receipt(&result);
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("credential-secret"));
}

#[test]
fn wall_time_covers_pipe_drain_and_kills_the_process_group() {
    let fixture = Fixture::new("orphan_pipe", 4096);
    let manifest = fs::read_to_string(&fixture.manifest)
        .unwrap()
        .replace("wall_time_ms = 1500", "wall_time_ms = 500");
    fs::write(&fixture.manifest, manifest).unwrap();
    let catalog = fixture.catalog();

    let started = Instant::now();
    let result = catalog
        .invoke(&invocation(&catalog), Some(&fixture.state_root))
        .unwrap();
    let elapsed = started.elapsed();

    assert_failed_receipt(&result);
    assert_eq!(
        result.receipt.error.as_ref().unwrap().code.as_str(),
        "provider_timeout"
    );
    assert!(
        elapsed < Duration::from_millis(900),
        "pipe drain exceeded the invocation bound: {elapsed:?}"
    );
    assert!(fixture.directory.path().join("orphan-started").is_file());
    thread::sleep(Duration::from_millis(1_100));
    assert!(
        !fixture.directory.path().join("orphan-survived").exists(),
        "a Provider descendant survived process-group cleanup"
    );
}

#[test]
fn accepted_malformed_and_oversized_stdout_return_content_free_failed_receipts() {
    let malformed = Fixture::new("malformed", 4096);
    let catalog = malformed.catalog();
    let result = catalog
        .invoke(&invocation(&catalog), Some(&malformed.state_root))
        .unwrap();
    assert_failed_receipt(&result);
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("{not-json"));

    let oversize = Fixture::new("oversize", 128);
    let catalog = oversize.catalog();
    let result = catalog
        .invoke(&invocation(&catalog), Some(&oversize.state_root))
        .unwrap();
    assert_failed_receipt(&result);
}

#[test]
fn canonical_output_cannot_exceed_the_invocation_budget() {
    let fixture = Fixture::new("success", 4096);
    let manifest = fs::read_to_string(&fixture.manifest).unwrap().replace(
        "source = { kind = \"response\", pointer = \"/output\" }",
        "source = { kind = \"input\", pointer = \"/content\" }",
    );
    fs::write(&fixture.manifest, manifest).unwrap();
    let catalog = fixture.catalog();
    let mut invocation = invocation(&catalog);
    invocation.input.body["content"] = json!("x".repeat(512));
    invocation.input.digest =
        sha256_digest(&canonical_json_v1_bytes(&invocation.input.body).unwrap());
    invocation.budget.output_bytes = 256;

    let result = catalog
        .invoke(&invocation, Some(&fixture.state_root))
        .unwrap();
    assert_failed_receipt(&result);
}

#[test]
fn enforce_authority_is_rejected_until_effect_reconciliation_exists() {
    let fixture = Fixture::new("timeout", 4096);
    let manifest = fs::read_to_string(&fixture.manifest)
        .unwrap()
        .replace("authority = \"advise\"", "authority = \"enforce\"");
    fs::write(&fixture.manifest, manifest).unwrap();

    assert!(matches!(
        ProviderCatalog::discover(
            ProviderManifestSource::File(fixture.manifest.clone()),
            &ProviderAdmissionOptions::default(),
        ),
        Err(ProviderHostError::InvalidManifest { .. })
    ));
}

#[test]
fn uncertain_disposition_is_rejected_without_effect_reconciliation() {
    let fixture = Fixture::new("success", 4096);
    let manifest = fs::read_to_string(&fixture.manifest)
        .unwrap()
        .replace("failed = \"failed\"", "failed = \"uncertain\"");
    fs::write(&fixture.manifest, manifest).unwrap();

    assert!(matches!(
        ProviderCatalog::discover(
            ProviderManifestSource::File(fixture.manifest.clone()),
            &ProviderAdmissionOptions::default(),
        ),
        Err(ProviderHostError::InvalidManifest { .. })
    ));
}

#[test]
fn host_and_user_scopes_are_rejected_until_they_can_be_invoked() {
    for scope in ["host", "user"] {
        let fixture = Fixture::new("success", 4096);
        let manifest = fs::read_to_string(&fixture.manifest).unwrap().replace(
            "scopes = [\"tool_call\"]",
            &format!("scopes = [\"{scope}\"]"),
        );
        fs::write(&fixture.manifest, manifest).unwrap();
        assert!(matches!(
            ProviderCatalog::discover(
                ProviderManifestSource::File(fixture.manifest.clone()),
                &ProviderAdmissionOptions::default(),
            ),
            Err(ProviderHostError::InvalidManifest { .. })
        ));
    }
}

fn assert_failed_receipt(result: &aw_contracts::provider::ProviderInvocationResult) {
    assert_eq!(result.receipt.disposition, ProviderDisposition::Failed);
    assert!(result.outcome.output.is_none());
    assert!(result.receipt.error.is_some());
    assert!(result.receipt.output_schema.is_none());
    assert!(result.receipt.output_digest.is_none());
    assert!(result.receipt.output_bytes.is_none());
}

#[test]
fn missing_unknown_and_escaping_manifest_inputs_are_rejected() {
    let fixture = Fixture::new("success", 4096);
    let missing = fs::read_to_string(&fixture.manifest)
        .unwrap()
        .replace("./fake-provider.sh", "./missing-provider");
    fs::write(&fixture.manifest, missing).unwrap();
    assert!(ProviderCatalog::discover(
        ProviderManifestSource::File(fixture.manifest.clone()),
        &ProviderAdmissionOptions::default(),
    )
    .is_err());

    let fixture = Fixture::new("success", 4096);
    let unknown = format!(
        "{}\nunknown_field = true\n",
        fs::read_to_string(&fixture.manifest).unwrap()
    );
    fs::write(&fixture.manifest, unknown).unwrap();
    assert!(matches!(
        ProviderCatalog::discover(
            ProviderManifestSource::File(fixture.manifest.clone()),
            &ProviderAdmissionOptions::default(),
        ),
        Err(ProviderHostError::ManifestParse { .. })
    ));

    let fixture = Fixture::new("success", 4096);
    let outside = fixture.directory.path().join("outside.sh");
    fs::write(&outside, FAKE_PROVIDER).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
    let package = fixture.directory.path().join("package");
    fs::create_dir(&package).unwrap();
    write_schemas(&package);
    let manifest = package.join("provider.toml");
    fs::write(
        &manifest,
        manifest_text(
            "fixture-provider",
            "context.projection.prepare/v1",
            "success",
            4096,
        )
        .replace("./fake-provider.sh", "../outside.sh"),
    )
    .unwrap();
    assert!(matches!(
        ProviderCatalog::discover(
            ProviderManifestSource::File(manifest),
            &ProviderAdmissionOptions::default(),
        ),
        Err(ProviderHostError::InvalidManifest { .. })
    ));
}

#[test]
fn schema_resources_are_content_addressed_valid_json() {
    let fixture = Fixture::new("success", 4096);
    let manifest = fs::read_to_string(&fixture.manifest).unwrap().replace(
        "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    fs::write(&fixture.manifest, manifest).unwrap();
    assert!(matches!(
        ProviderCatalog::discover(
            ProviderManifestSource::File(fixture.manifest.clone()),
            &ProviderAdmissionOptions::default(),
        ),
        Err(ProviderHostError::InvalidManifest { .. })
    ));

    let fixture = Fixture::new("success", 4096);
    fs::write(
        fixture.directory.path().join("canonical-input.schema.json"),
        "{not-json",
    )
    .unwrap();
    assert!(matches!(
        ProviderCatalog::discover(
            ProviderManifestSource::File(fixture.manifest.clone()),
            &ProviderAdmissionOptions::default(),
        ),
        Err(ProviderHostError::InvalidManifest { .. })
    ));
}

#[test]
fn schema_resources_must_be_self_contained_draft_2020_12_documents() {
    let fixture = Fixture::new("success", 4096);
    fixture.replace_schema(
        "canonical-input.schema.json",
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$ref": "https://example.invalid/external.schema.json"
        }),
    );

    assert!(matches!(
        ProviderCatalog::discover(
            ProviderManifestSource::File(fixture.manifest.clone()),
            &ProviderAdmissionOptions::default(),
        ),
        Err(ProviderHostError::InvalidManifest { .. })
    ));
}

#[test]
fn invocation_validates_canonical_and_mapped_native_inputs_before_spawn() {
    for resource in ["canonical-input.schema.json", "native-input.schema.json"] {
        let fixture = Fixture::new("success", 4096);
        fixture.replace_schema(
            resource,
            json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "required": ["schema_test_missing"]
            }),
        );
        let catalog = fixture.catalog();

        assert!(matches!(
            catalog.invoke(&invocation(&catalog), Some(&fixture.state_root)),
            Err(ProviderHostError::InvocationRejected(_))
        ));
        assert!(
            !fixture.state_root.join("fixture-provider").exists(),
            "invalid {resource} input must not reach Provider execution"
        );
    }
}

#[test]
fn invalid_native_and_canonical_outputs_settle_as_failed_receipts() {
    for resource in ["native-output.schema.json", "canonical-output.schema.json"] {
        let fixture = Fixture::new("success", 4096);
        fixture.replace_schema(
            resource,
            json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "required": ["schema_test_missing"]
            }),
        );
        let catalog = fixture.catalog();
        let result = catalog
            .invoke(&invocation(&catalog), Some(&fixture.state_root))
            .unwrap();

        assert_failed_receipt(&result);
        assert_eq!(
            result.receipt.error.as_ref().unwrap().code.as_str(),
            "provider_invalid_response"
        );
    }
}

#[test]
fn native_response_must_correlate_with_the_mapped_request() {
    let fixture = Fixture::new("mismatched_response", 4096);
    fixture.correlate_response("/protocol_version", "/protocol_version");
    let catalog = fixture.catalog();
    let result = catalog
        .invoke(&invocation(&catalog), Some(&fixture.state_root))
        .unwrap();

    assert_failed_receipt(&result);
    assert_eq!(
        result.receipt.error.as_ref().unwrap().code.as_str(),
        "provider_invalid_response"
    );
}

#[test]
fn directory_identity_is_enforced_and_capability_implementations_coexist() {
    let directory = tempfile::tempdir().unwrap();
    let same_provider = directory.path().join("same-provider");
    let other_provider = directory.path().join("other-provider");
    fs::create_dir(&same_provider).unwrap();
    fs::create_dir(&other_provider).unwrap();
    for package in [&same_provider, &other_provider] {
        let executable = package.join("fake-provider.sh");
        fs::write(&executable, FAKE_PROVIDER).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        write_schemas(package);
    }
    fs::write(
        same_provider.join("provider.toml"),
        manifest_text(
            "same-provider",
            "context.projection.prepare/v1",
            "success",
            4096,
        ),
    )
    .unwrap();
    fs::write(
        other_provider.join("provider.toml"),
        manifest_text(
            "same-provider",
            "context.projection.other/v1",
            "success",
            4096,
        ),
    )
    .unwrap();
    assert!(matches!(
        ProviderCatalog::discover(
            ProviderManifestSource::Directory(directory.path().to_path_buf()),
            &ProviderAdmissionOptions::default(),
        ),
        Err(ProviderHostError::InvalidManifest { .. })
    ));

    fs::write(
        other_provider.join("provider.toml"),
        manifest_text(
            "other-provider",
            "context.projection.prepare/v1",
            "success",
            4096,
        ),
    )
    .unwrap();
    let catalog = ProviderCatalog::discover(
        ProviderManifestSource::Directory(directory.path().to_path_buf()),
        &ProviderAdmissionOptions::default(),
    )
    .unwrap();
    assert_eq!(catalog.descriptors().len(), 2);
    let state_root = directory.path().join("state");
    fs::create_dir(&state_root).unwrap();
    let invocation = invocation_for_provider(&catalog, "other-provider");
    let result = catalog.invoke(&invocation, Some(&state_root)).unwrap();
    assert_eq!(result.receipt.provider_id.as_str(), "other-provider");
}

#[test]
fn directory_discovery_rejects_symlinked_provider_packages() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = Fixture::new("success", 4096);
    symlink(
        outside.directory.path(),
        root.path().join("fixture-provider"),
    )
    .unwrap();

    assert!(matches!(
        ProviderCatalog::discover(
            ProviderManifestSource::Directory(root.path().to_path_buf()),
            &ProviderAdmissionOptions::default(),
        ),
        Err(ProviderHostError::SymlinkNotAllowed { .. })
    ));
}

#[test]
fn invocation_must_pin_the_admitted_provider_release() {
    let fixture = Fixture::new("success", 4096);
    let catalog = fixture.catalog();
    let mut invocation = invocation(&catalog);
    invocation.provider.manifest_digest = Digest::parse("0".repeat(64)).unwrap();
    assert!(matches!(
        catalog.invoke(&invocation, Some(&fixture.state_root)),
        Err(ProviderHostError::InvocationRejected(_))
    ));
    assert!(!fixture.state_root.join("fixture-provider").exists());
}

#[test]
fn bare_executable_requires_an_explicit_root() {
    let fixture = Fixture::new("success", 4096);
    let manifest = fs::read_to_string(&fixture.manifest)
        .unwrap()
        .replace("./fake-provider.sh", "fake-provider.sh");
    fs::write(&fixture.manifest, manifest).unwrap();
    assert!(ProviderCatalog::discover(
        ProviderManifestSource::File(fixture.manifest.clone()),
        &ProviderAdmissionOptions::default(),
    )
    .is_err());

    let catalog = ProviderCatalog::discover(
        ProviderManifestSource::File(fixture.manifest.clone()),
        &ProviderAdmissionOptions {
            executable_roots: vec![fixture.directory.path().to_path_buf()],
        },
    )
    .unwrap();
    assert_eq!(catalog.descriptors().len(), 1);
}
