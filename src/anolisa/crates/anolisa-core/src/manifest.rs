//! Manifest v2 schema.
//!
//! This module hosts the canonical typed representation of the TOML manifests
//! shipped under `src/anolisa/manifests/`. Two top-level shapes exist:
//!
//! * `CapabilityManifest` — user-facing capability definition.
//! * `ComponentManifest` — concrete component (runtime or osbase substrate).
//!
//! All deserialization is *tolerant*: missing optional fields default and we
//! accept both the new canonical TOML layout (per `templates/*.toml`) and the
//! current bundled fixture layout. Unknown keys are silently ignored so that
//! schema growth in either direction does not break existing artifacts.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Default schema version applied when the TOML omits it.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// CapabilityManifest
// ---------------------------------------------------------------------------

/// Canonical capability manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "CapabilityManifestRaw")]
pub struct CapabilityManifest {
    pub schema_version: u32,
    pub capability: CapabilityMeta,
    pub components: Vec<String>,
    pub default_features: Vec<String>,
    pub env_requirements: EnvRequirements,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityMeta {
    pub name: String,
    pub description: String,
    pub layer: String,
    pub stability: String,
}

#[derive(Deserialize)]
struct CapabilityManifestRaw {
    #[serde(default = "current_schema_version", alias = "manifest_version")]
    schema_version: u32,
    capability: CapabilityMetaRaw,
    #[serde(default)]
    implementation: ImplementationRaw,
    #[serde(default, alias = "env_requirements")]
    requires_env: EnvRequirementsRaw,
}

#[derive(Deserialize)]
struct CapabilityMetaRaw {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "default_capability_layer")]
    layer: String,
    #[serde(default = "default_stability")]
    stability: String,
}

#[derive(Deserialize, Default)]
struct ImplementationRaw {
    #[serde(default)]
    components: Vec<String>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

impl From<CapabilityManifestRaw> for CapabilityManifest {
    fn from(raw: CapabilityManifestRaw) -> Self {
        let ImplementationRaw {
            components,
            features,
        } = raw.implementation;
        let mut default_features: Vec<String> = features.into_values().flatten().collect();
        default_features.sort();
        default_features.dedup();
        Self {
            schema_version: raw.schema_version,
            capability: CapabilityMeta {
                name: raw.capability.name,
                description: raw.capability.description,
                layer: raw.capability.layer,
                stability: raw.capability.stability,
            },
            components,
            default_features,
            env_requirements: raw.requires_env.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// ComponentManifest
// ---------------------------------------------------------------------------

/// Canonical runtime / osbase component manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "ComponentManifestRaw")]
pub struct ComponentManifest {
    pub schema_version: u32,
    pub component: ComponentMeta,
    pub source: SourceSpec,
    pub distribution_selectors: Vec<String>,
    pub build: BuildSpec,
    pub install: InstallSpec,
    pub env_requirements: EnvRequirements,
    pub dependencies: Vec<String>,
    pub features: Vec<FeatureSpec>,
    pub adapters: Vec<String>,
    pub health: HealthSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentMeta {
    pub name: String,
    pub version: String,
    pub layer: String,
    pub domain: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceSpec {
    pub kind: String,
    pub path: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildSpec {
    pub backend: String,
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstallSpec {
    pub modes: Vec<String>,
    pub files: Vec<String>,
    pub services: Vec<String>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureSpec {
    pub name: String,
    pub description: String,
    pub default: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HealthSpec {
    pub kind: String,
    pub command: Option<String>,
    pub probe: Option<String>,
}

#[derive(Deserialize)]
struct ComponentManifestRaw {
    #[serde(default = "current_schema_version", alias = "manifest_version")]
    schema_version: u32,
    component: ComponentMetaRaw,
    #[serde(default)]
    source: Option<SourceRaw>,
    #[serde(default)]
    distribution: Option<DistributionRaw>,
    #[serde(default)]
    build: Option<BuildRaw>,
    #[serde(default)]
    install: Option<InstallRaw>,
    #[serde(default, alias = "env_requirements")]
    environment: EnvRequirementsRaw,
    #[serde(default)]
    dependencies: DependenciesRaw,
    #[serde(default)]
    features: Vec<FeatureRaw>,
    #[serde(default)]
    adapters: Vec<AdapterRaw>,
    #[serde(default, alias = "health")]
    health_checks: Vec<HealthCheckRaw>,
}

#[derive(Deserialize)]
struct ComponentMetaRaw {
    name: String,
    version: String,
    #[serde(default = "default_runtime_layer")]
    layer: String,
    #[serde(default)]
    domain: Option<String>,
}

#[derive(Deserialize, Default)]
struct SourceRaw {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    upstream: Option<String>,
}

#[derive(Deserialize, Default)]
struct DistributionRaw {
    #[serde(default)]
    selectors: Vec<DistributionSelectorRaw>,
}

#[derive(Deserialize, Default)]
struct DistributionSelectorRaw {
    #[serde(default)]
    install_mode: Option<String>,
    #[serde(default)]
    os: Vec<String>,
    #[serde(default)]
    arch: Vec<String>,
    #[serde(default)]
    libc: Option<String>,
    #[serde(default)]
    pkg_base: Option<String>,
}

#[derive(Deserialize, Default)]
struct BuildRaw {
    #[serde(default, alias = "backend")]
    system: Option<String>,
    #[serde(default, alias = "outputs")]
    targets: Vec<String>,
    #[serde(default)]
    outputs_named: Vec<BuildOutputRaw>,
}

#[derive(Deserialize)]
struct BuildOutputRaw {
    name: String,
}

#[derive(Deserialize, Default)]
struct InstallRaw {
    #[serde(default)]
    modes: Vec<String>,
    #[serde(default)]
    files: Vec<InstallFileRaw>,
    #[serde(default)]
    services: Vec<String>,
    #[serde(default)]
    capabilities: Vec<InstallCapabilityRaw>,
}

#[derive(Deserialize, Default)]
struct InstallFileRaw {
    #[serde(default)]
    dest: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Deserialize, Default)]
#[allow(dead_code)]
struct InstallCapabilityRaw {
    #[serde(default)]
    path: Option<String>,
    // Accepted from the TOML (e.g. `caps = ["cap_bpf"]`) but not yet surfaced
    // on `InstallSpec`; kept so unknown-keys are silently tolerated.
    #[serde(default)]
    caps: Vec<String>,
}

#[derive(Deserialize, Default)]
struct DependenciesRaw {
    #[serde(default)]
    build: Vec<String>,
    #[serde(default)]
    runtime: Vec<String>,
    #[serde(default)]
    components: Vec<String>,
}

#[derive(Deserialize)]
struct FeatureRaw {
    name: String,
    #[serde(default, alias = "label")]
    description: String,
    #[serde(default)]
    default: bool,
}

#[derive(Deserialize)]
struct AdapterRaw {
    #[serde(default)]
    framework: Option<String>,
    #[serde(default)]
    plugin_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize, Default)]
struct HealthCheckRaw {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    probe: Option<String>,
}

impl From<ComponentManifestRaw> for ComponentManifest {
    fn from(raw: ComponentManifestRaw) -> Self {
        let component = ComponentMeta {
            name: raw.component.name,
            version: raw.component.version,
            layer: raw.component.layer,
            domain: raw.component.domain.unwrap_or_default(),
        };

        let source = raw.source.map(source_from_raw).unwrap_or_default();

        let distribution_selectors = raw
            .distribution
            .map(|d| {
                d.selectors
                    .into_iter()
                    .map(selector_to_string)
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let build = raw
            .build
            .map(|b| {
                let mut outputs = b.targets;
                outputs.extend(b.outputs_named.into_iter().map(|o| o.name));
                BuildSpec {
                    backend: b.system.unwrap_or_default(),
                    outputs,
                }
            })
            .unwrap_or_default();

        let install = raw
            .install
            .map(|i| {
                let files = i
                    .files
                    .into_iter()
                    .filter_map(|f| f.dest.or(f.source))
                    .collect();
                let capabilities = i.capabilities.into_iter().filter_map(|c| c.path).collect();
                InstallSpec {
                    modes: i.modes,
                    files,
                    services: i.services,
                    capabilities,
                }
            })
            .unwrap_or_default();

        let mut dependencies: Vec<String> = Vec::new();
        dependencies.extend(raw.dependencies.build);
        dependencies.extend(raw.dependencies.runtime);
        dependencies.extend(raw.dependencies.components);
        dependencies.sort();
        dependencies.dedup();

        let features = raw
            .features
            .into_iter()
            .map(|f| FeatureSpec {
                name: f.name,
                description: f.description,
                default: f.default,
            })
            .collect();

        let mut adapters: Vec<String> = raw
            .adapters
            .into_iter()
            .filter_map(|a| a.plugin_id.or(a.framework).or(a.name))
            .collect();
        adapters.sort();
        adapters.dedup();

        let health = raw
            .health_checks
            .into_iter()
            .next()
            .map(|h| HealthSpec {
                kind: h.kind.unwrap_or_default(),
                command: h.command,
                probe: h.probe.or(h.name),
            })
            .unwrap_or_default();

        Self {
            schema_version: raw.schema_version,
            component,
            source,
            distribution_selectors,
            build,
            install,
            env_requirements: raw.environment.into(),
            dependencies,
            features,
            adapters,
            health,
        }
    }
}

fn source_from_raw(raw: SourceRaw) -> SourceSpec {
    let kind = raw
        .kind
        .or(raw.upstream)
        .unwrap_or_else(|| "workspace".to_string());
    SourceSpec {
        kind,
        path: raw.path,
        url: raw.url,
    }
}

fn selector_to_string(s: DistributionSelectorRaw) -> String {
    let os = if s.os.is_empty() {
        "*".to_string()
    } else {
        s.os.join("|")
    };
    let arch = if s.arch.is_empty() {
        "*".to_string()
    } else {
        s.arch.join("|")
    };
    let libc = s.libc.unwrap_or_else(|| "*".to_string());
    let mode = s.install_mode.unwrap_or_else(|| "any".to_string());
    let pkg = s.pkg_base.unwrap_or_else(|| "any".to_string());
    format!("{mode}:{os}/{arch}/{libc}/{pkg}")
}

// ---------------------------------------------------------------------------
// EnvRequirements
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(from = "EnvRequirementsRaw")]
pub struct EnvRequirements {
    pub os: Vec<String>,
    pub arch: Vec<String>,
    pub libc: Vec<String>,
    pub kernel_min: Option<String>,
    pub btf: Option<bool>,
    pub cap_bpf: Option<bool>,
    pub pkg_base: Vec<String>,
}

#[derive(Deserialize, Default)]
struct EnvRequirementsRaw {
    // Capability-style keys.
    #[serde(default)]
    os: Option<StringOrList>,
    #[serde(default)]
    arch: Option<StringOrList>,
    #[serde(default)]
    libc: Option<StringOrList>,
    #[serde(default)]
    kernel: Option<String>,
    #[serde(default)]
    pkg_base: Option<StringOrList>,

    // Component-style keys.
    #[serde(default)]
    requires_os: Option<StringOrList>,
    #[serde(default)]
    requires_arch: Option<StringOrList>,
    #[serde(default)]
    requires_libc: Option<StringOrList>,
    #[serde(default)]
    requires_kernel: Option<String>,
    #[serde(default)]
    requires_pkg_base: Option<StringOrList>,

    // Either prefix accepts the free-form map.
    #[serde(default)]
    requires_env: BTreeMap<String, toml::Value>,

    #[serde(default)]
    btf: Option<bool>,
    #[serde(default)]
    cap_bpf: Option<bool>,
}

impl From<EnvRequirementsRaw> for EnvRequirements {
    fn from(r: EnvRequirementsRaw) -> Self {
        let merge = |a: Option<StringOrList>, b: Option<StringOrList>| -> Vec<String> {
            a.or(b).map(|v| v.into_vec()).unwrap_or_default()
        };
        let btf = r
            .btf
            .or_else(|| lookup_bool(&r.requires_env, "btf_available"))
            .or_else(|| lookup_bool(&r.requires_env, "btf"));
        let cap_bpf = r
            .cap_bpf
            .or_else(|| lookup_cap_bpf(r.requires_env.get("linux_capabilities")))
            .or_else(|| lookup_cap_bpf(r.requires_env.get("capability")));
        Self {
            os: merge(r.os, r.requires_os),
            arch: merge(r.arch, r.requires_arch),
            libc: merge(r.libc, r.requires_libc),
            kernel_min: r.kernel.or(r.requires_kernel),
            btf,
            cap_bpf,
            pkg_base: merge(r.pkg_base, r.requires_pkg_base),
        }
    }
}

fn lookup_bool(map: &BTreeMap<String, toml::Value>, key: &str) -> Option<bool> {
    match map.get(key)? {
        toml::Value::Boolean(b) => Some(*b),
        toml::Value::String(s) => match s.as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn lookup_cap_bpf(value: Option<&toml::Value>) -> Option<bool> {
    let v = value?;
    match v {
        toml::Value::String(s) => Some(s.eq_ignore_ascii_case("CAP_BPF")),
        toml::Value::Array(items) => Some(items.iter().any(|item| match item {
            toml::Value::String(s) => s.eq_ignore_ascii_case("CAP_BPF"),
            _ => false,
        })),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum StringOrList {
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(s) => vec![s],
            Self::Many(v) => v,
        }
    }
}

impl<'de> Deserialize<'de> for StringOrList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Helper {
            One(String),
            Many(Vec<String>),
        }
        Ok(match Helper::deserialize(deserializer)? {
            Helper::One(s) => Self::One(s),
            Helper::Many(v) => Self::Many(v),
        })
    }
}

fn current_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}
fn default_capability_layer() -> String {
    "tier1-capability".to_string()
}
fn default_runtime_layer() -> String {
    "runtime".to_string()
}
fn default_stability() -> String {
    "stable".to_string()
}

// ---------------------------------------------------------------------------
// File-loading entry points
// ---------------------------------------------------------------------------

impl CapabilityManifest {
    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let content = read_to_string(path)?;
        toml::from_str(&content)
            .map_err(|e| ManifestError::Parse(path.display().to_string(), e.to_string()))
    }

    pub fn from_str(s: &str) -> Result<Self, ManifestError> {
        toml::from_str(s).map_err(|e| ManifestError::Parse("<string>".into(), e.to_string()))
    }
}

impl ComponentManifest {
    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let content = read_to_string(path)?;
        toml::from_str(&content)
            .map_err(|e| ManifestError::Parse(path.display().to_string(), e.to_string()))
    }

    pub fn from_str(s: &str) -> Result<Self, ManifestError> {
        toml::from_str(s).map_err(|e| ManifestError::Parse("<string>".into(), e.to_string()))
    }
}

fn read_to_string(path: &Path) -> Result<String, ManifestError> {
    std::fs::read_to_string(path).map_err(|e| ManifestError::Io(path.display().to_string(), e))
}

/// Helper used by [`Catalog`] when scanning layer directories.
pub(crate) fn manifest_paths(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !dir.exists() {
        return files;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("toml") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("cannot read manifest '{0}': {1}")]
    Io(String, std::io::Error),
    #[error("cannot parse manifest '{0}': {1}")]
    Parse(String, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_manifest_parses_existing_fixture() {
        let toml_text = r#"
            [capability]
            name = "agent-observability"
            description = "Agent behavior tracing"

            [implementation]
            components = ["agentsight"]
            features.agentsight = ["token_counting", "ebpf_tracing"]

            [requires_env]
            os = "linux"
            arch = ["x86_64", "aarch64"]
        "#;
        let m = CapabilityManifest::from_str(toml_text).expect("parse");
        assert_eq!(m.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(m.capability.name, "agent-observability");
        assert_eq!(m.capability.layer, "tier1-capability");
        assert_eq!(m.components, vec!["agentsight"]);
        assert_eq!(m.env_requirements.os, vec!["linux"]);
        assert_eq!(m.env_requirements.arch, vec!["x86_64", "aarch64"]);
        assert!(m.default_features.contains(&"token_counting".to_string()));
    }

    #[test]
    fn component_manifest_parses_existing_fixture() {
        let toml_text = r#"
            [component]
            name = "agentsight"
            version = "0.2.0"
            layer = "runtime"
            domain = "observability"

            [build]
            system = "cargo"
            targets = ["agentsight"]

            [install]
            modes = ["system"]
            services = ["agentsight.service"]

            [[install.files]]
            source = "target/release/agentsight"
            dest = "{bindir}/agentsight"

            [environment]
            requires_os = "linux"
            requires_arch = ["x86_64"]
            requires_kernel = ">=5.8"

            [environment.requires_env]
            btf_available = "true"
            capability = "CAP_BPF"

            [dependencies]
            build = ["rust>=1.91"]
            runtime = ["kernel-headers"]

            [[features]]
            name = "token_counting"
            label = "LLM Token metering"
            default = true
        "#;
        let m = ComponentManifest::from_str(toml_text).expect("parse");
        assert_eq!(m.component.name, "agentsight");
        assert_eq!(m.build.backend, "cargo");
        assert_eq!(m.install.modes, vec!["system"]);
        assert_eq!(m.env_requirements.kernel_min.as_deref(), Some(">=5.8"));
        assert_eq!(m.env_requirements.btf, Some(true));
        assert_eq!(m.env_requirements.cap_bpf, Some(true));
        assert_eq!(m.features.len(), 1);
        assert_eq!(m.features[0].description, "LLM Token metering");
        assert!(m.dependencies.iter().any(|d| d == "rust>=1.91"));
    }
}
