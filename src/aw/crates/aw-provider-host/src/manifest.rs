//! Strict Provider manifest decoding and executable admission.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use aw_contracts::common::{BoundedName, Digest};
use aw_contracts::provider::{
    ProviderApiVersion, ProviderAuthority, ProviderCapabilityDescriptor, ProviderDescriptor,
    ProviderDisposition, ProviderDriver, ProviderLifecycle, ProviderMeasurementKind,
    ProviderScopeKind, SchemaReference, VersionedSchema,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::{
    AdmittedCapability, AdmittedProvider, DispositionMapping, JsonMapCodec, MeterMapping,
    MissingValueAction, OutputFieldMapping, OutputValueSource, ProviderAdmissionOptions,
    ProviderDataDeclaration, ProviderHostError, ProviderLimits, ProviderManifestSource,
    ProviderNetworkAccess, ProviderPermissionDeclaration, RequestFieldMapping, RequestValueSource,
    ResponseCorrelation, ScopeField, MAX_PROVIDER_MANIFEST_BYTES,
};

const JSON_MAP_CODEC: &str = "json-map/v1";
const DEFAULT_INPUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_WALL_TIME_MS: u64 = 5 * 60 * 1000;
const MAX_SCHEMA_RESOURCE_BYTES: u64 = 1024 * 1024;
const MAX_PROVIDER_CAPABILITIES: usize = 64;
const MAX_JSON_MAP_FIELDS: usize = 128;
const MAX_RESPONSE_METERS: usize = 64;
const PROVIDER_STATE_PLACEHOLDER: &str = "{provider_state_dir}";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderManifest {
    api_version: ProviderApiVersion,
    provider_id: String,
    provider_version: String,
    driver: ProviderDriver,
    lifecycle: ProviderLifecycle,
    executable: ManifestExecutable,
    limits: ManifestLimits,
    permissions: ManifestPermissions,
    data: ManifestData,
    capabilities: Vec<ManifestCapability>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestExecutable {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestLimits {
    wall_time_ms: u64,
    #[serde(default = "default_input_bytes")]
    input_bytes: u64,
    output_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPermissions {
    network: ProviderNetworkAccess,
    inherit_environment: bool,
    filesystem_read: Vec<String>,
    filesystem_write: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestData {
    reads: Vec<String>,
    writes: Vec<String>,
    sensitivity: String,
    retention: String,
    telemetry: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestCapability {
    capability: String,
    input_contract: ManifestCanonicalContract,
    output_contract: ManifestCanonicalContract,
    native_input: ManifestNativeContract,
    native_output: ManifestNativeContract,
    authority: ProviderAuthority,
    scopes: Vec<ProviderScopeKind>,
    codec: ManifestCodec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestCodec {
    kind: String,
    request: ManifestRequestMapping,
    response: ManifestResponseMapping,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestCanonicalContract {
    schema: String,
    resource: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestNativeContract {
    resource: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestRequestMapping {
    fields: Vec<ManifestRequestField>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestRequestField {
    target: String,
    source: ManifestRequestSource,
    on_missing: ManifestMissingValueAction,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ManifestRequestSource {
    Const { value: Value },
    Input { pointer: String },
    Scope { field: ManifestScopeField },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManifestMissingValueAction {
    Reject,
    Omit,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManifestScopeField {
    TargetKind,
    TargetAuthority,
    TargetIdentifier,
    EnvironmentId,
    ExecutionContextId,
    ActorId,
    AgentSessionId,
    WorkId,
    AttemptId,
    TurnId,
    ToolUseId,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestResponseMapping {
    disposition: ManifestDispositionMapping,
    #[serde(default)]
    correlations: Vec<ManifestResponseCorrelation>,
    #[serde(default)]
    output_fields: Vec<ManifestOutputField>,
    #[serde(default)]
    meters: Vec<ManifestMeter>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestResponseCorrelation {
    request_pointer: String,
    response_pointer: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestDispositionMapping {
    source: String,
    values: BTreeMap<String, ProviderDisposition>,
    on_unknown: ManifestUnknownDispositionAction,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManifestUnknownDispositionAction {
    Fail,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestOutputField {
    target: String,
    source: ManifestOutputSource,
    when_disposition: Vec<ProviderDisposition>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ManifestOutputSource {
    Const { value: Value },
    Input { pointer: String },
    Response { pointer: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestMeter {
    meter_id: String,
    unit: String,
    measurement_kind: ProviderMeasurementKind,
    #[serde(default)]
    method: Option<String>,
    value_pointer: String,
}

pub(super) fn discover(
    source: ProviderManifestSource,
    options: &ProviderAdmissionOptions,
) -> Result<Vec<AdmittedProvider>, ProviderHostError> {
    let roots = admitted_executable_roots(&options.executable_roots)?;
    let locations = match source {
        ProviderManifestSource::File(path) => vec![ManifestLocation {
            path: validate_manifest_file(&path)?,
            expected_provider_id: None,
        }],
        ProviderManifestSource::Directory(path) => manifest_directory_entries(&path)?,
    };
    let mut providers = locations
        .iter()
        .map(|location| {
            load_manifest(
                &location.path,
                &roots,
                location.expected_provider_id.as_deref(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    providers.sort_by(|left, right| {
        left.descriptor
            .provider_id
            .cmp(&right.descriptor.provider_id)
            .then_with(|| left.manifest_path.cmp(&right.manifest_path))
    });
    Ok(providers)
}

struct ManifestLocation {
    path: PathBuf,
    expected_provider_id: Option<String>,
}

fn default_input_bytes() -> u64 {
    DEFAULT_INPUT_BYTES
}

fn validate_manifest_file(path: &Path) -> Result<PathBuf, ProviderHostError> {
    require_absolute(path, "Provider manifest")?;
    let metadata = fs::symlink_metadata(path).map_err(|source| ProviderHostError::Io {
        operation: "inspect Provider manifest",
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ProviderHostError::SymlinkNotAllowed {
            label: "Provider manifest",
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(ProviderHostError::WrongFileType {
            label: "Provider manifest",
            expected: "regular file",
            path: path.to_path_buf(),
        });
    }
    canonicalize(path, "canonicalize Provider manifest")
}

fn manifest_directory_entries(path: &Path) -> Result<Vec<ManifestLocation>, ProviderHostError> {
    require_absolute(path, "Provider manifest directory")?;
    let metadata = fs::symlink_metadata(path).map_err(|source| ProviderHostError::Io {
        operation: "inspect Provider manifest directory",
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ProviderHostError::SymlinkNotAllowed {
            label: "Provider manifest directory",
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_dir() {
        return Err(ProviderHostError::WrongFileType {
            label: "Provider manifest directory",
            expected: "directory",
            path: path.to_path_buf(),
        });
    }
    let root = canonicalize(path, "canonicalize Provider manifest directory")?;
    let entries = fs::read_dir(&root).map_err(|source| ProviderHostError::Io {
        operation: "read Provider manifest directory",
        path: root.clone(),
        source,
    })?;
    let mut manifests = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ProviderHostError::Io {
            operation: "read Provider manifest directory entry",
            path: root.clone(),
            source,
        })?;
        let package = entry.path();
        let metadata = fs::symlink_metadata(&package).map_err(|source| ProviderHostError::Io {
            operation: "inspect Provider package directory",
            path: package.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ProviderHostError::SymlinkNotAllowed {
                label: "Provider package directory",
                path: package,
            });
        }
        if !metadata.is_dir() {
            continue;
        }
        let package_name = package
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| ProviderHostError::InvalidManifest {
                path: package.clone(),
                reason: "Provider package directory name is not UTF-8".to_owned(),
            })?;
        validate_provider_id(&package, package_name)?;
        let package = canonicalize(&package, "canonicalize Provider package directory")?;
        if !package.starts_with(&root) {
            return Err(ProviderHostError::InvalidManifest {
                path: package,
                reason: "Provider package directory escaped its discovery root".to_owned(),
            });
        }
        let manifest = package.join("provider.toml");
        let metadata = fs::symlink_metadata(&manifest).map_err(|source| ProviderHostError::Io {
            operation: "inspect package Provider manifest",
            path: manifest.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ProviderHostError::SymlinkNotAllowed {
                label: "package Provider manifest",
                path: manifest,
            });
        }
        if !metadata.is_file() {
            return Err(ProviderHostError::WrongFileType {
                label: "package Provider manifest",
                expected: "regular file",
                path: manifest,
            });
        }
        let canonical = canonicalize(&manifest, "canonicalize package Provider manifest")?;
        if !canonical.starts_with(&package) || !canonical.starts_with(&root) {
            return Err(ProviderHostError::InvalidManifest {
                path: canonical,
                reason: "package Provider manifest escaped its discovery root".to_owned(),
            });
        }
        manifests.push(ManifestLocation {
            path: canonical,
            expected_provider_id: Some(package_name.to_owned()),
        });
    }
    manifests.sort_by(|left, right| left.path.cmp(&right.path));
    if manifests.is_empty() {
        return Err(ProviderHostError::EmptyManifestDirectory(root));
    }
    Ok(manifests)
}

fn load_manifest(
    path: &Path,
    executable_roots: &[PathBuf],
    expected_provider_id: Option<&str>,
) -> Result<AdmittedProvider, ProviderHostError> {
    let metadata = fs::metadata(path).map_err(|source| ProviderHostError::Io {
        operation: "inspect Provider manifest",
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_PROVIDER_MANIFEST_BYTES as u64 {
        return invalid(path, "manifest exceeds the 1 MiB admission limit");
    }
    let bytes = fs::read(path).map_err(|source| ProviderHostError::Io {
        operation: "read Provider manifest",
        path: path.to_path_buf(),
        source,
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|_| ProviderHostError::InvalidManifest {
        path: path.to_path_buf(),
        reason: "manifest is not valid UTF-8".to_owned(),
    })?;
    let manifest: ProviderManifest =
        toml::from_str(text).map_err(|source| ProviderHostError::ManifestParse {
            path: path.to_path_buf(),
            source,
        })?;
    validate_manifest(
        path,
        manifest,
        &bytes,
        executable_roots,
        expected_provider_id,
    )
}

fn validate_manifest(
    path: &Path,
    manifest: ProviderManifest,
    bytes: &[u8],
    executable_roots: &[PathBuf],
    expected_provider_id: Option<&str>,
) -> Result<AdmittedProvider, ProviderHostError> {
    if manifest.api_version != ProviderApiVersion::V1 {
        return invalid(path, "unsupported Provider API version");
    }
    if manifest.driver != ProviderDriver::ExecJsonV1 {
        return invalid(path, "only exec-json/v1 is admitted by this Host");
    }
    if manifest.lifecycle != ProviderLifecycle::OneShot {
        return invalid(path, "exec-json/v1 requires the one_shot lifecycle");
    }
    validate_provider_id(path, &manifest.provider_id)?;
    if expected_provider_id.is_some_and(|expected| expected != manifest.provider_id) {
        return invalid(
            path,
            "Provider package directory name must match manifest provider_id",
        );
    }
    let provider_id = bounded_name(path, "provider_id", manifest.provider_id)?;
    let provider_version = bounded_name(path, "provider_version", manifest.provider_version)?;
    let working_directory = path
        .parent()
        .ok_or_else(|| ProviderHostError::InvalidManifest {
            path: path.to_path_buf(),
            reason: "manifest has no parent directory".to_owned(),
        })?;
    let executable = resolve_executable(
        path,
        working_directory,
        &manifest.executable.command,
        executable_roots,
    )?;
    validate_args(path, &manifest.executable.args)?;
    validate_environment(path, &manifest.executable.environment)?;
    let limits = validate_limits(path, manifest.limits)?;
    let permissions = validate_permissions(path, manifest.permissions)?;
    let data = validate_data(path, manifest.data)?;
    let requires_state_directory = manifest
        .executable
        .environment
        .values()
        .any(|value| value == PROVIDER_STATE_PLACEHOLDER)
        || permissions
            .filesystem_read
            .iter()
            .chain(permissions.filesystem_write.iter())
            .any(|value| value == PROVIDER_STATE_PLACEHOLDER);
    if manifest.capabilities.is_empty() {
        return invalid(path, "manifest must advertise at least one Capability");
    }
    if manifest.capabilities.len() > MAX_PROVIDER_CAPABILITIES {
        return invalid(
            path,
            format!("manifest capabilities exceed the {MAX_PROVIDER_CAPABILITIES}-item limit"),
        );
    }

    let mut seen = BTreeSet::new();
    let mut admitted_capabilities = Vec::with_capacity(manifest.capabilities.len());
    let mut descriptors = Vec::with_capacity(manifest.capabilities.len());
    for capability in manifest.capabilities {
        let admitted = validate_capability(path, working_directory, capability)?;
        let identity = super::schema_key(&admitted.descriptor.capability);
        if !seen.insert(identity.clone()) {
            return Err(ProviderHostError::DuplicateCapability {
                id: identity.0,
                version: identity.1,
            });
        }
        descriptors.push(admitted.descriptor.clone());
        admitted_capabilities.push(admitted);
    }
    admitted_capabilities.sort_by(|left, right| {
        super::schema_key(&left.descriptor.capability)
            .cmp(&super::schema_key(&right.descriptor.capability))
    });
    descriptors.sort_by(|left, right| {
        super::schema_key(&left.capability).cmp(&super::schema_key(&right.capability))
    });

    let manifest_digest = sha256_digest(bytes);
    let descriptor = ProviderDescriptor {
        api_version: ProviderApiVersion::V1,
        provider_id,
        provider_version,
        manifest_digest,
        driver: ProviderDriver::ExecJsonV1,
        lifecycle: ProviderLifecycle::OneShot,
        capabilities: descriptors,
    };
    Ok(AdmittedProvider {
        descriptor,
        manifest_path: path.to_path_buf(),
        working_directory: working_directory.to_path_buf(),
        executable,
        args: manifest.executable.args,
        environment: manifest.executable.environment,
        limits,
        requires_state_directory,
        permissions,
        data,
        capabilities: admitted_capabilities,
    })
}

fn validate_capability(
    path: &Path,
    manifest_directory: &Path,
    capability: ManifestCapability,
) -> Result<AdmittedCapability, ProviderHostError> {
    if capability.authority == ProviderAuthority::Enforce {
        return invalid(
            path,
            "exec-json/v1 does not yet admit Enforce Capabilities; effect idempotency and reconciliation are not implemented",
        );
    }
    let capability_id = parse_schema(path, "capability", &capability.capability)?;
    let (input_contract, canonical_input_schema) = validate_canonical_contract(
        path,
        manifest_directory,
        "input_contract",
        capability.input_contract,
    )?;
    let (output_contract, canonical_output_schema) = validate_canonical_contract(
        path,
        manifest_directory,
        "output_contract",
        capability.output_contract,
    )?;
    let native_input_schema = validate_native_contract(
        path,
        manifest_directory,
        "native_input",
        capability.native_input,
    )?;
    let native_output_schema = validate_native_contract(
        path,
        manifest_directory,
        "native_output",
        capability.native_output,
    )?;
    if capability.scopes.is_empty() {
        return invalid(path, "Capability scopes must not be empty");
    }
    if capability
        .scopes
        .iter()
        .any(|scope| matches!(scope, ProviderScopeKind::Host | ProviderScopeKind::User))
    {
        return invalid(
            path,
            "exec-json/v1 currently requires an ExecutionContext or a more specific invocation scope",
        );
    }
    let has_duplicate_scope = capability
        .scopes
        .iter()
        .enumerate()
        .any(|(index, scope)| capability.scopes[..index].contains(scope));
    if has_duplicate_scope {
        return invalid(path, "Capability scopes contain a duplicate value");
    }
    if capability.codec.kind != JSON_MAP_CODEC {
        return invalid(path, "unsupported codec; expected json-map/v1");
    }

    let request_fields = validate_request_fields(path, capability.codec.request.fields)?;
    validate_json_pointer(
        path,
        "response disposition source",
        &capability.codec.response.disposition.source,
    )?;
    if capability.codec.response.disposition.values.is_empty() {
        return invalid(path, "response disposition mapping must not be empty");
    }
    if capability
        .codec
        .response
        .disposition
        .values
        .values()
        .any(|value| {
            matches!(
                value,
                ProviderDisposition::EffectApplied | ProviderDisposition::Uncertain
            )
        })
    {
        return invalid(
            path,
            "exec-json/v1 does not yet admit effect_applied or uncertain dispositions",
        );
    }
    for native in capability.codec.response.disposition.values.keys() {
        if native.is_empty() || native.len() > 128 || native.contains('\0') {
            return invalid(
                path,
                "native disposition keys must be non-empty and bounded",
            );
        }
    }
    let _ = capability.codec.response.disposition.on_unknown;
    let response_correlations =
        validate_response_correlations(path, capability.codec.response.correlations)?;
    let output_fields = validate_output_fields(path, capability.codec.response.output_fields)?;
    if capability
        .codec
        .response
        .disposition
        .values
        .values()
        .any(|value| *value == ProviderDisposition::Produced)
        && output_fields.is_empty()
    {
        return invalid(
            path,
            "a produced disposition requires at least one canonical output field",
        );
    }
    if capability.codec.response.meters.len() > MAX_RESPONSE_METERS {
        return invalid(
            path,
            format!("response meters exceed the {MAX_RESPONSE_METERS}-item limit"),
        );
    }
    let meters = capability
        .codec
        .response
        .meters
        .into_iter()
        .map(|meter| {
            validate_json_pointer(path, "meter value pointer", &meter.value_pointer)?;
            Ok(MeterMapping {
                meter_id: bounded_name(path, "meter_id", meter.meter_id)?,
                unit: bounded_name(path, "meter unit", meter.unit)?,
                measurement_kind: meter.measurement_kind,
                method: meter
                    .method
                    .map(|method| bounded_name(path, "meter method", method))
                    .transpose()?,
                value_pointer: meter.value_pointer,
            })
        })
        .collect::<Result<Vec<_>, ProviderHostError>>()?;
    let mut meter_ids = BTreeSet::new();
    if meters
        .iter()
        .any(|meter| !meter_ids.insert(meter.meter_id.clone()))
    {
        return invalid(path, "response meter_id values must be unique");
    }
    let descriptor = ProviderCapabilityDescriptor {
        capability: capability_id,
        authority: capability.authority,
        input_contract,
        output_contract,
        scopes: capability.scopes,
    };
    Ok(AdmittedCapability {
        descriptor,
        canonical_input_schema,
        canonical_output_schema,
        native_input_schema,
        native_output_schema,
        codec: JsonMapCodec {
            request_fields,
            disposition: DispositionMapping {
                source: capability.codec.response.disposition.source,
                values: capability.codec.response.disposition.values,
            },
            response_correlations,
            output_fields,
            meters,
        },
    })
}

fn validate_response_correlations(
    path: &Path,
    correlations: Vec<ManifestResponseCorrelation>,
) -> Result<Vec<ResponseCorrelation>, ProviderHostError> {
    if correlations.len() > 16 {
        return invalid(path, "response correlations exceed the 16-item limit");
    }
    correlations
        .into_iter()
        .map(|correlation| {
            validate_json_pointer(
                path,
                "response correlation request pointer",
                &correlation.request_pointer,
            )?;
            validate_json_pointer(
                path,
                "response correlation response pointer",
                &correlation.response_pointer,
            )?;
            if correlation.request_pointer.is_empty() || correlation.response_pointer.is_empty() {
                return invalid(
                    path,
                    "response correlations must address fields below the document root",
                );
            }
            Ok(ResponseCorrelation {
                request_pointer: correlation.request_pointer,
                response_pointer: correlation.response_pointer,
            })
        })
        .collect()
}

fn validate_canonical_contract(
    manifest_path: &Path,
    manifest_directory: &Path,
    field: &'static str,
    contract: ManifestCanonicalContract,
) -> Result<(SchemaReference, jsonschema::Validator), ProviderHostError> {
    let schema = parse_schema(manifest_path, field, &contract.schema)?;
    let (digest, validator) = validate_schema_resource(
        manifest_path,
        manifest_directory,
        field,
        &contract.resource,
        &contract.sha256,
    )?;
    Ok((SchemaReference { schema, digest }, validator))
}

fn validate_native_contract(
    manifest_path: &Path,
    manifest_directory: &Path,
    field: &'static str,
    contract: ManifestNativeContract,
) -> Result<jsonschema::Validator, ProviderHostError> {
    validate_schema_resource(
        manifest_path,
        manifest_directory,
        field,
        &contract.resource,
        &contract.sha256,
    )
    .map(|(_, validator)| validator)
}

fn validate_schema_resource(
    manifest_path: &Path,
    manifest_directory: &Path,
    field: &'static str,
    resource: &str,
    expected_sha256: &str,
) -> Result<(Digest, jsonschema::Validator), ProviderHostError> {
    let relative = Path::new(resource);
    if resource.is_empty()
        || resource.contains('\0')
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return invalid(
            manifest_path,
            format!("{field}.resource must be a strict relative path"),
        );
    }
    let candidate = manifest_directory.join(relative);
    let metadata = fs::symlink_metadata(&candidate).map_err(|source| ProviderHostError::Io {
        operation: "inspect Provider schema resource",
        path: candidate.clone(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ProviderHostError::SymlinkNotAllowed {
            label: "Provider schema resource",
            path: candidate,
        });
    }
    if !metadata.is_file() {
        return Err(ProviderHostError::WrongFileType {
            label: "Provider schema resource",
            expected: "regular file",
            path: candidate,
        });
    }
    if metadata.len() > MAX_SCHEMA_RESOURCE_BYTES {
        return invalid(
            manifest_path,
            format!("{field}.resource exceeds the 1 MiB admission limit"),
        );
    }
    let canonical = canonicalize(&candidate, "canonicalize Provider schema resource")?;
    if !canonical.starts_with(manifest_directory) || canonical != candidate {
        return invalid(
            manifest_path,
            format!("{field}.resource resolves through or outside a package path"),
        );
    }
    let bytes = fs::read(&canonical).map_err(|source| ProviderHostError::Io {
        operation: "read Provider schema resource",
        path: canonical,
        source,
    })?;
    let schema = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
        ProviderHostError::InvalidManifest {
            path: manifest_path.to_path_buf(),
            reason: format!("{field}.resource is not valid JSON: {error}"),
        }
    })?;
    let validator = jsonschema::draft202012::options()
        .offline()
        .build(&schema)
        .map_err(|_| ProviderHostError::InvalidManifest {
            path: manifest_path.to_path_buf(),
            reason: format!(
                "{field}.resource is not a self-contained JSON Schema Draft 2020-12 document"
            ),
        })?;
    let expected = Digest::parse(expected_sha256.to_owned()).map_err(|error| {
        ProviderHostError::InvalidManifest {
            path: manifest_path.to_path_buf(),
            reason: format!("{field}.sha256 is invalid: {error}"),
        }
    })?;
    let actual = sha256_digest(&bytes);
    if actual != expected {
        return invalid(
            manifest_path,
            format!("{field}.sha256 does not match the exact resource bytes"),
        );
    }
    Ok((actual, validator))
}

fn validate_request_fields(
    path: &Path,
    fields: Vec<ManifestRequestField>,
) -> Result<Vec<RequestFieldMapping>, ProviderHostError> {
    if fields.len() > MAX_JSON_MAP_FIELDS {
        return invalid(
            path,
            format!("request fields exceed the {MAX_JSON_MAP_FIELDS}-item limit"),
        );
    }
    let mut targets = Vec::with_capacity(fields.len());
    let mut mappings = Vec::with_capacity(fields.len());
    for field in fields {
        validate_mapping_target(path, "request target", &field.target, &targets)?;
        if matches!(&field.source, ManifestRequestSource::Const { .. })
            && matches!(field.on_missing, ManifestMissingValueAction::Omit)
        {
            return invalid(path, "constant request fields cannot be omitted as missing");
        }
        let source = match field.source {
            ManifestRequestSource::Const { value } => RequestValueSource::Constant(value),
            ManifestRequestSource::Input { pointer } => {
                validate_json_pointer(path, "request input source", &pointer)?;
                RequestValueSource::Input(pointer)
            }
            ManifestRequestSource::Scope { field } => {
                RequestValueSource::Scope(map_scope_field(field))
            }
        };
        let on_missing = match field.on_missing {
            ManifestMissingValueAction::Reject => MissingValueAction::Reject,
            ManifestMissingValueAction::Omit => MissingValueAction::Omit,
        };
        targets.push(field.target.clone());
        mappings.push(RequestFieldMapping {
            target: field.target,
            source,
            on_missing,
        });
    }
    Ok(mappings)
}

fn validate_output_fields(
    path: &Path,
    fields: Vec<ManifestOutputField>,
) -> Result<Vec<OutputFieldMapping>, ProviderHostError> {
    if fields.len() > MAX_JSON_MAP_FIELDS {
        return invalid(
            path,
            format!("response output fields exceed the {MAX_JSON_MAP_FIELDS}-item limit"),
        );
    }
    let mut targets = Vec::with_capacity(fields.len());
    let mut mappings = Vec::with_capacity(fields.len());
    for field in fields {
        validate_mapping_target(path, "response output target", &field.target, &targets)?;
        if field.when_disposition.is_empty() {
            return invalid(path, "response output field has no disposition condition");
        }
        let has_duplicate_disposition = field
            .when_disposition
            .iter()
            .enumerate()
            .any(|(index, value)| field.when_disposition[..index].contains(value));
        if has_duplicate_disposition {
            return invalid(
                path,
                "response output field repeats a disposition condition",
            );
        }
        if field
            .when_disposition
            .iter()
            .any(|value| *value != ProviderDisposition::Produced)
        {
            return invalid(
                path,
                "json-map/v1 canonical output fields are only valid for produced results",
            );
        }
        let source = match field.source {
            ManifestOutputSource::Const { value } => OutputValueSource::Constant(value),
            ManifestOutputSource::Input { pointer } => {
                validate_json_pointer(path, "response input source", &pointer)?;
                OutputValueSource::Input(pointer)
            }
            ManifestOutputSource::Response { pointer } => {
                validate_json_pointer(path, "response native source", &pointer)?;
                OutputValueSource::Response(pointer)
            }
        };
        targets.push(field.target.clone());
        mappings.push(OutputFieldMapping {
            target: field.target,
            source,
            when_disposition: field.when_disposition,
        });
    }
    Ok(mappings)
}

fn validate_mapping_target(
    path: &Path,
    field: &'static str,
    target: &str,
    existing: &[String],
) -> Result<(), ProviderHostError> {
    validate_json_pointer(path, field, target)?;
    if target.is_empty() {
        return invalid(path, format!("{field} must address a field below the root"));
    }
    let candidate = pointer_segments(target);
    if existing.iter().any(|value| {
        let other = pointer_segments(value);
        candidate.starts_with(&other) || other.starts_with(&candidate)
    }) {
        return invalid(path, format!("{field} overlaps another mapped target"));
    }
    Ok(())
}

fn pointer_segments(pointer: &str) -> Vec<String> {
    pointer
        .split('/')
        .skip(1)
        .map(|segment| segment.replace("~1", "/").replace("~0", "~"))
        .collect()
}

fn map_scope_field(field: ManifestScopeField) -> ScopeField {
    match field {
        ManifestScopeField::TargetKind => ScopeField::TargetKind,
        ManifestScopeField::TargetAuthority => ScopeField::TargetAuthority,
        ManifestScopeField::TargetIdentifier => ScopeField::TargetIdentifier,
        ManifestScopeField::EnvironmentId => ScopeField::EnvironmentId,
        ManifestScopeField::ExecutionContextId => ScopeField::ExecutionContextId,
        ManifestScopeField::ActorId => ScopeField::ActorId,
        ManifestScopeField::AgentSessionId => ScopeField::AgentSessionId,
        ManifestScopeField::WorkId => ScopeField::WorkId,
        ManifestScopeField::AttemptId => ScopeField::AttemptId,
        ManifestScopeField::TurnId => ScopeField::TurnId,
        ManifestScopeField::ToolUseId => ScopeField::ToolUseId,
    }
}

fn validate_permissions(
    path: &Path,
    permissions: ManifestPermissions,
) -> Result<ProviderPermissionDeclaration, ProviderHostError> {
    if permissions.inherit_environment {
        return invalid(
            path,
            "exec-json/v1 does not admit inherited process environments",
        );
    }
    for declaration in permissions
        .filesystem_read
        .iter()
        .chain(permissions.filesystem_write.iter())
    {
        if declaration != PROVIDER_STATE_PLACEHOLDER {
            return invalid(
                path,
                "filesystem declarations currently admit only {provider_state_dir}",
            );
        }
    }
    Ok(ProviderPermissionDeclaration {
        network: permissions.network,
        inherit_environment: permissions.inherit_environment,
        filesystem_read: permissions.filesystem_read,
        filesystem_write: permissions.filesystem_write,
    })
}

fn validate_data(
    path: &Path,
    data: ManifestData,
) -> Result<ProviderDataDeclaration, ProviderHostError> {
    let retention_supported = data.retention == "provider_managed"
        || (data.retention == "none" && data.writes.is_empty());
    if data.sensitivity != "inherits_input" || !retention_supported || data.telemetry != "disabled"
    {
        return invalid(
            path,
            "unsupported data declaration or non-empty writes with retention=none",
        );
    }
    let reads = data
        .reads
        .into_iter()
        .map(|value| declaration_name(path, "data.reads", value))
        .collect::<Result<Vec<_>, _>>()?;
    let writes = data
        .writes
        .into_iter()
        .map(|value| declaration_name(path, "data.writes", value))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ProviderDataDeclaration {
        reads,
        writes,
        sensitivity: bounded_name(path, "data.sensitivity", data.sensitivity)?,
        retention: bounded_name(path, "data.retention", data.retention)?,
        telemetry: bounded_name(path, "data.telemetry", data.telemetry)?,
    })
}

fn declaration_name(
    path: &Path,
    field: &'static str,
    value: String,
) -> Result<BoundedName, ProviderHostError> {
    let valid = !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if !valid {
        return invalid(path, format!("{field} contains an invalid data category"));
    }
    bounded_name(path, field, value)
}

fn validate_limits(
    path: &Path,
    limits: ManifestLimits,
) -> Result<ProviderLimits, ProviderHostError> {
    if limits.wall_time_ms == 0 || limits.wall_time_ms > MAX_WALL_TIME_MS {
        return invalid(path, "wall_time_ms must be between 1 and 300000");
    }
    if limits.input_bytes == 0 || limits.input_bytes > MAX_INPUT_BYTES {
        return invalid(path, "input_bytes must be between 1 and 67108864");
    }
    if limits.output_bytes == 0 || limits.output_bytes > MAX_OUTPUT_BYTES {
        return invalid(path, "output_bytes must be between 1 and 67108864");
    }
    Ok(ProviderLimits {
        wall_time_ms: limits.wall_time_ms,
        input_bytes: usize::try_from(limits.input_bytes).map_err(|_| {
            ProviderHostError::InvalidManifest {
                path: path.to_path_buf(),
                reason: "input_bytes cannot be represented on this host".to_owned(),
            }
        })?,
        output_bytes: usize::try_from(limits.output_bytes).map_err(|_| {
            ProviderHostError::InvalidManifest {
                path: path.to_path_buf(),
                reason: "output_bytes cannot be represented on this host".to_owned(),
            }
        })?,
    })
}

fn parse_schema(
    path: &Path,
    field: &'static str,
    value: &str,
) -> Result<VersionedSchema, ProviderHostError> {
    let (id, version) =
        value
            .rsplit_once("/v")
            .ok_or_else(|| ProviderHostError::InvalidManifest {
                path: path.to_path_buf(),
                reason: format!("{field} must end in /v1"),
            })?;
    if version != "1" {
        return invalid(path, format!("unsupported {field} revision v{version}"));
    }
    validate_schema_id(path, field, id)?;
    Ok(VersionedSchema {
        id: bounded_name(path, field, id.to_owned())?,
        version: 1,
    })
}

fn validate_schema_id(
    path: &Path,
    field: &'static str,
    value: &str,
) -> Result<(), ProviderHostError> {
    let valid = !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .as_bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        invalid(
            path,
            format!("{field} contains an invalid schema identifier"),
        )
    }
}

fn validate_provider_id(path: &Path, value: &str) -> Result<(), ProviderHostError> {
    let valid = !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .as_bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        invalid(path, "provider_id must be a lowercase path-safe identifier")
    }
}

fn validate_args(path: &Path, args: &[String]) -> Result<(), ProviderHostError> {
    if args.len() > 64 {
        return invalid(path, "executable args exceed the 64-item limit");
    }
    if args
        .iter()
        .any(|arg| arg.len() > 4096 || arg.contains('\0'))
    {
        return invalid(path, "executable args contain an oversized or NUL value");
    }
    Ok(())
}

fn validate_environment(
    path: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<(), ProviderHostError> {
    for (name, value) in environment {
        let valid_name = name.bytes().enumerate().all(|(index, byte)| match index {
            0 => byte.is_ascii_uppercase() || byte == b'_',
            _ => byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_',
        });
        if !valid_name || name.len() > 128 {
            return invalid(path, "environment contains an invalid variable name");
        }
        if name == "AW_PROVIDER_STATE_DIR"
            || name == "PATH"
            || name == "LD_PRELOAD"
            || name == "LD_LIBRARY_PATH"
            || name.starts_with("DYLD_")
        {
            return invalid(path, format!("environment variable `{name}` is reserved"));
        }
        if value.len() > 4096 || value.contains('\0') {
            return invalid(
                path,
                format!("environment variable `{name}` is not bounded"),
            );
        }
        if (value.contains('{') || value.contains('}')) && value != PROVIDER_STATE_PLACEHOLDER {
            return invalid(
                path,
                format!("environment variable `{name}` uses an unknown placeholder"),
            );
        }
    }
    Ok(())
}

fn validate_json_pointer(
    path: &Path,
    field: &'static str,
    pointer: &str,
) -> Result<(), ProviderHostError> {
    if pointer.is_empty() {
        return Ok(());
    }
    if !pointer.starts_with('/') {
        return invalid(path, format!("{field} must be an RFC 6901 JSON pointer"));
    }
    let bytes = pointer.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'~' {
            if bytes
                .get(index + 1)
                .is_none_or(|next| !matches!(next, b'0' | b'1'))
            {
                return invalid(path, format!("{field} contains an invalid RFC 6901 escape"));
            }
            index += 1;
        }
        index += 1;
    }
    Ok(())
}

fn admitted_executable_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>, ProviderHostError> {
    let mut admitted = Vec::with_capacity(roots.len());
    for root in roots {
        require_absolute(root, "executable root")?;
        let canonical = canonicalize(root, "canonicalize executable root")?;
        if !canonical.is_dir() {
            return Err(ProviderHostError::WrongFileType {
                label: "executable root",
                expected: "directory",
                path: canonical,
            });
        }
        admitted.push(canonical);
    }
    admitted.sort();
    admitted.dedup();
    Ok(admitted)
}

fn resolve_executable(
    manifest_path: &Path,
    manifest_directory: &Path,
    command: &str,
    executable_roots: &[PathBuf],
) -> Result<PathBuf, ProviderHostError> {
    if command.is_empty() || command.contains('\0') {
        return invalid(
            manifest_path,
            "executable command must be non-empty and NUL-free",
        );
    }
    let command_path = Path::new(command);
    if command_path.is_absolute() {
        return invalid(
            manifest_path,
            "absolute executable commands are not admitted; use an explicit executable root",
        );
    }
    if command.contains('/') {
        if command_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return invalid(manifest_path, "relative executable path attempts to escape");
        }
        let resolved = canonicalize(
            &manifest_directory.join(command_path),
            "canonicalize relative Provider executable",
        )?;
        if !resolved.starts_with(manifest_directory) {
            return invalid(
                manifest_path,
                "relative executable resolves outside the manifest directory",
            );
        }
        validate_executable_file(manifest_path, &resolved)?;
        return Ok(resolved);
    }

    validate_bare_command(manifest_path, command)?;
    let mut matches = Vec::new();
    for root in executable_roots {
        let candidate = root.join(command);
        let Ok(resolved) = fs::canonicalize(&candidate) else {
            continue;
        };
        if !resolved.starts_with(root) {
            return invalid(
                manifest_path,
                "installed executable resolves outside its admitted root",
            );
        }
        validate_executable_file(manifest_path, &resolved)?;
        matches.push(resolved);
    }
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [resolved] => Ok(resolved.clone()),
        [] => invalid(
            manifest_path,
            format!("installed executable `{command}` was not found in an explicit root"),
        ),
        _ => invalid(
            manifest_path,
            format!("installed executable `{command}` is ambiguous across explicit roots"),
        ),
    }
}

fn validate_bare_command(path: &Path, command: &str) -> Result<(), ProviderHostError> {
    if command
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        Ok(())
    } else {
        invalid(
            path,
            "installed executable name contains an invalid character",
        )
    }
}

fn validate_executable_file(
    manifest_path: &Path,
    executable: &Path,
) -> Result<(), ProviderHostError> {
    let metadata = fs::metadata(executable).map_err(|source| ProviderHostError::Io {
        operation: "inspect Provider executable",
        path: executable.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(ProviderHostError::WrongFileType {
            label: "Provider executable",
            expected: "regular file",
            path: executable.to_path_buf(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return invalid(
                manifest_path,
                "Provider executable has no execute permission",
            );
        }
    }
    Ok(())
}

fn require_absolute(path: &Path, label: &'static str) -> Result<(), ProviderHostError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(ProviderHostError::PathNotAbsolute {
            label,
            path: path.to_path_buf(),
        })
    }
}

fn canonicalize(path: &Path, operation: &'static str) -> Result<PathBuf, ProviderHostError> {
    fs::canonicalize(path).map_err(|source| ProviderHostError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

fn bounded_name(
    path: &Path,
    field: &'static str,
    value: String,
) -> Result<BoundedName, ProviderHostError> {
    BoundedName::new(value).map_err(|error| ProviderHostError::InvalidManifest {
        path: path.to_path_buf(),
        reason: format!("{field} is invalid: {error}"),
    })
}

fn sha256_digest(bytes: &[u8]) -> Digest {
    let value = format!("{:x}", Sha256::digest(bytes));
    Digest::parse(value)
        .unwrap_or_else(|_| unreachable!("SHA-256 formatting always produces a canonical digest"))
}

fn invalid<T>(path: &Path, reason: impl Into<String>) -> Result<T, ProviderHostError> {
    Err(ProviderHostError::InvalidManifest {
        path: path.to_path_buf(),
        reason: reason.into(),
    })
}

pub(super) fn expand_environment_value(value: &str, state_directory: &Path) -> String {
    if value == PROVIDER_STATE_PLACEHOLDER {
        state_directory.to_string_lossy().into_owned()
    } else {
        value.to_owned()
    }
}
