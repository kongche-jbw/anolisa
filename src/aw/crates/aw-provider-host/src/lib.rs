//! Headless Provider discovery, admission, capability projection, and invocation.
//!
//! This module deliberately does not create Tasks or Agent Runtime sessions. A
//! Provider invocation is one scoped system capability call and remains usable
//! without depending on any Agent Environment implementation.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use aw_contracts::provider::{
    CapabilityInvocation, ProviderCapabilityDescriptor, ProviderDescriptor, ProviderDisposition,
    ProviderInvocationResult, VersionedSchema,
};
use serde_json::Value;
use thiserror::Error;

mod driver;
mod graph;
mod manifest;
mod process_group;

pub use graph::{
    ProviderDataDeclaration, ProviderGuaranteeState, ProviderNetworkAccess,
    ProviderPermissionDeclaration, RuntimeCapabilityEntry, RuntimeCapabilityGraph,
};

/// Encodes one value as Agent Workload canonical JSON v1.
///
/// Re-exported from [`aw_contracts::canonical::canonical_json_v1_bytes`]. The
/// encoding is a contract shared by every AW boundary that digests payloads,
/// so the implementation lives in `aw-contracts` and downstream crates may
/// import it from either module.
pub use aw_contracts::canonical::canonical_json_v1_bytes;

/// Maximum manifest size accepted during discovery.
pub const MAX_PROVIDER_MANIFEST_BYTES: usize = 1024 * 1024;
/// Maximum invocation document accepted by the headless CLI before admission.
pub const MAX_PROVIDER_INVOCATION_BYTES: usize = 65 * 1024 * 1024;

/// One explicit source of Provider manifests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderManifestSource {
    /// Load exactly one manifest file.
    File(PathBuf),
    /// Load `provider.toml` from each direct Provider package directory.
    ///
    /// The package layout is `<root>/<provider-id>/provider.toml`; the package
    /// directory and declared Provider identities must match.
    Directory(PathBuf),
}

/// Trusted host inputs used while resolving executable identities.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderAdmissionOptions {
    /// Explicit executable roots searched for a bare installed command name.
    pub executable_roots: Vec<PathBuf>,
}

/// Bounded stderr facts retained without exposing Provider diagnostic content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStderrDiagnostic {
    /// SHA-256 of all captured stderr bytes.
    pub sha256: String,
    /// Total stderr bytes observed before the process settled.
    pub bytes: u64,
    /// Whether retained diagnostic bytes exceeded the fixed in-memory bound.
    pub truncated: bool,
}

/// Failure raised by manifest admission or a headless Provider invocation.
#[derive(Debug, Error)]
pub enum ProviderHostError {
    /// An externally supplied path is relative.
    #[error("{label} path must be absolute: {path}")]
    PathNotAbsolute {
        /// Path role used in the diagnostic.
        label: &'static str,
        /// Rejected path.
        path: PathBuf,
    },
    /// A required path does not identify the expected file type.
    #[error("{label} is not a {expected}: {path}")]
    WrongFileType {
        /// Path role used in the diagnostic.
        label: &'static str,
        /// Expected file type.
        expected: &'static str,
        /// Rejected path.
        path: PathBuf,
    },
    /// Symbolic links are not admitted at a manifest discovery boundary.
    #[error("{label} must not be a symbolic link: {path}")]
    SymlinkNotAllowed {
        /// Path role used in the diagnostic.
        label: &'static str,
        /// Rejected path.
        path: PathBuf,
    },
    /// A filesystem operation failed before Provider execution.
    #[error("failed to {operation} {path}: {source}")]
    Io {
        /// Operation that failed.
        operation: &'static str,
        /// Affected path.
        path: PathBuf,
        /// Underlying operating-system failure.
        #[source]
        source: io::Error,
    },
    /// Manifest bytes are not valid strict TOML.
    #[error("failed to parse Provider manifest {path}: {source}")]
    ManifestParse {
        /// Manifest path.
        path: PathBuf,
        /// Strict TOML decoding failure.
        #[source]
        source: toml::de::Error,
    },
    /// A parsed manifest violates a Provider admission invariant.
    #[error("invalid Provider manifest {path}: {reason}")]
    InvalidManifest {
        /// Manifest path.
        path: PathBuf,
        /// Safe validation reason without raw manifest content.
        reason: String,
    },
    /// No manifest was found in an explicit discovery directory.
    #[error("Provider manifest directory is empty: {0}")]
    EmptyManifestDirectory(PathBuf),
    /// More than one admitted manifest declares the same Provider identity.
    #[error("duplicate Provider identity `{0}`")]
    DuplicateProvider(String),
    /// One Provider manifest declares the same Capability revision twice.
    #[error("duplicate Capability `{id}/v{version}`")]
    DuplicateCapability {
        /// Stable Capability name.
        id: String,
        /// Capability schema revision.
        version: u16,
    },
    /// Invocation selects no admitted Capability implementation.
    #[error("no admitted Provider implements Capability `{id}/v{version}`")]
    CapabilityUnavailable {
        /// Stable Capability name.
        id: String,
        /// Capability schema revision.
        version: u16,
    },
    /// Invocation metadata conflicts with the admitted descriptor or scope.
    #[error("Provider invocation was rejected: {0}")]
    InvocationRejected(String),
    /// Provider could not be spawned or its pipes could not be managed.
    #[error("Provider process failed: {0}")]
    Process(String),
    /// Provider did not settle before the effective invocation deadline.
    #[error("Provider `{provider_id}` timed out after {timeout_ms} ms")]
    Timeout {
        /// Provider identity.
        provider_id: String,
        /// Effective timeout enforced by the Host.
        timeout_ms: u64,
        /// Bounded, content-free stderr diagnostic.
        stderr: ProviderStderrDiagnostic,
    },
    /// Provider exited without a successful native response.
    #[error("Provider `{provider_id}` exited with status {status}")]
    NonZeroExit {
        /// Provider identity.
        provider_id: String,
        /// Numeric exit code or `signal`.
        status: String,
        /// Bounded, content-free stderr diagnostic.
        stderr: ProviderStderrDiagnostic,
    },
    /// Native or canonical mapped output exceeded the admitted output limit.
    #[error("Provider `{provider_id}` output exceeds the {limit}-byte limit")]
    OutputTooLarge {
        /// Provider identity.
        provider_id: String,
        /// Effective output byte limit.
        limit: usize,
        /// SHA-256 of the rejected output representation.
        output_sha256: String,
    },
    /// Native stdout was bounded but did not satisfy the declared response codec.
    #[error("Provider `{provider_id}` returned an invalid mapped JSON response: {reason}")]
    InvalidResponse {
        /// Provider identity.
        provider_id: String,
        /// Safe structural reason without raw stdout.
        reason: String,
        /// SHA-256 of the rejected native stdout.
        output_sha256: String,
    },
    /// Host-constructed terminal facts contradict the public result contract.
    #[error("Provider invocation result violates its contract: {reason}")]
    InvalidInvocationResult {
        /// Content-free invariant failure suitable for operator diagnostics.
        reason: String,
    },
}

/// Deterministically admitted Provider set and Capability index.
#[derive(Debug)]
pub struct ProviderCatalog {
    providers: Vec<AdmittedProvider>,
    capabilities: BTreeMap<(String, String, u16), usize>,
}

#[derive(Debug)]
struct AdmittedProvider {
    descriptor: ProviderDescriptor,
    manifest_path: PathBuf,
    working_directory: PathBuf,
    executable: PathBuf,
    args: Vec<String>,
    environment: BTreeMap<String, String>,
    limits: ProviderLimits,
    requires_state_directory: bool,
    permissions: ProviderPermissionDeclaration,
    data: ProviderDataDeclaration,
    capabilities: Vec<AdmittedCapability>,
}

#[derive(Debug, Clone, Copy)]
struct ProviderLimits {
    wall_time_ms: u64,
    input_bytes: usize,
    output_bytes: usize,
}

#[derive(Debug)]
struct AdmittedCapability {
    descriptor: ProviderCapabilityDescriptor,
    canonical_input_schema: jsonschema::Validator,
    canonical_output_schema: jsonschema::Validator,
    native_input_schema: jsonschema::Validator,
    native_output_schema: jsonschema::Validator,
    codec: JsonMapCodec,
}

#[derive(Debug)]
struct JsonMapCodec {
    request_fields: Vec<RequestFieldMapping>,
    disposition: DispositionMapping,
    response_correlations: Vec<ResponseCorrelation>,
    output_fields: Vec<OutputFieldMapping>,
    meters: Vec<MeterMapping>,
}

#[derive(Debug)]
struct ResponseCorrelation {
    request_pointer: String,
    response_pointer: String,
}

#[derive(Debug)]
struct RequestFieldMapping {
    target: String,
    source: RequestValueSource,
    on_missing: MissingValueAction,
}

#[derive(Debug)]
enum RequestValueSource {
    Constant(Value),
    Input(String),
    Scope(ScopeField),
}

#[derive(Debug, Clone, Copy)]
enum ScopeField {
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

#[derive(Debug, Clone, Copy)]
enum MissingValueAction {
    Reject,
    Omit,
}

#[derive(Debug)]
struct DispositionMapping {
    source: String,
    values: BTreeMap<String, ProviderDisposition>,
}

#[derive(Debug)]
struct OutputFieldMapping {
    target: String,
    source: OutputValueSource,
    when_disposition: Vec<ProviderDisposition>,
}

#[derive(Debug)]
enum OutputValueSource {
    Constant(Value),
    Input(String),
    Response(String),
}

#[derive(Debug)]
struct MeterMapping {
    meter_id: aw_contracts::common::BoundedName,
    unit: aw_contracts::common::BoundedName,
    measurement_kind: aw_contracts::provider::ProviderMeasurementKind,
    value_pointer: String,
    method: Option<aw_contracts::common::BoundedName>,
}

impl ProviderCatalog {
    /// Discovers and admits a complete manifest set from an explicit source.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed manifests, unsafe executable resolution,
    /// duplicate Provider or Capability identities, or unsupported contracts.
    pub fn discover(
        source: ProviderManifestSource,
        options: &ProviderAdmissionOptions,
    ) -> Result<Self, ProviderHostError> {
        let providers = manifest::discover(source, options)?;
        let mut provider_ids = BTreeSet::new();
        let mut capabilities = BTreeMap::new();

        for (provider_index, provider) in providers.iter().enumerate() {
            let provider_id = provider.descriptor.provider_id.as_str().to_owned();
            if !provider_ids.insert(provider_id.clone()) {
                return Err(ProviderHostError::DuplicateProvider(provider_id));
            }
            for capability in &provider.capabilities {
                let identity = &capability.descriptor.capability;
                let key = (
                    provider_id.clone(),
                    identity.id.as_str().to_owned(),
                    identity.version,
                );
                capabilities.insert(key, provider_index);
            }
        }

        Ok(Self {
            providers,
            capabilities,
        })
    }

    /// Projects the admitted set into a deterministic Runtime Capability Graph.
    #[must_use]
    pub fn capability_graph(&self) -> RuntimeCapabilityGraph {
        graph::project(&self.providers)
    }

    /// Invokes one admitted Capability without creating a Task or Agent Runtime.
    ///
    /// `state_root` is required only when the admitted manifest references
    /// `{provider_state_dir}`. The Host creates or reuses one isolated child
    /// after accepting the invocation; stateless Providers receive no state.
    ///
    /// # Errors
    ///
    /// Returns an error when admission metadata does not match, the state path
    /// escapes its root, process execution fails, or the response mapping is not
    /// satisfied exactly.
    pub fn invoke(
        &self,
        invocation: &CapabilityInvocation,
        state_root: Option<&Path>,
    ) -> Result<ProviderInvocationResult, ProviderHostError> {
        let key = (
            invocation.provider.provider_id.as_str().to_owned(),
            invocation.capability.id.as_str().to_owned(),
            invocation.capability.version,
        );
        let provider_index = self.capabilities.get(&key).copied().ok_or_else(|| {
            ProviderHostError::CapabilityUnavailable {
                id: key.1.clone(),
                version: key.2,
            }
        })?;
        let provider = self.providers.get(provider_index).ok_or_else(|| {
            ProviderHostError::InvocationRejected(
                "Capability index refers to an absent Provider".to_owned(),
            )
        })?;
        let capability = provider
            .capabilities
            .iter()
            .find(|candidate| candidate.descriptor.capability == invocation.capability)
            .ok_or_else(|| {
                ProviderHostError::InvocationRejected(
                    "Capability index and Provider descriptor disagree".to_owned(),
                )
            })?;
        if provider.descriptor.provider_version != invocation.provider.provider_version
            || provider.descriptor.manifest_digest != invocation.provider.manifest_digest
        {
            return Err(ProviderHostError::InvocationRejected(format!(
                "Provider selection does not match admitted `{}` release and manifest",
                invocation.provider.provider_id.as_str()
            )));
        }
        let result = driver::invoke(provider, capability, invocation, state_root)?;
        result
            .validate_for_invocation(invocation)
            .map_err(|error| ProviderHostError::InvalidInvocationResult {
                reason: error.to_string(),
            })?;
        Ok(result)
    }

    /// Returns admitted descriptors in deterministic Provider identity order.
    #[must_use]
    pub fn descriptors(&self) -> Vec<&ProviderDescriptor> {
        self.providers
            .iter()
            .map(|provider| &provider.descriptor)
            .collect()
    }
}

fn schema_key(schema: &VersionedSchema) -> (String, u16) {
    (schema.id.as_str().to_owned(), schema.version)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
