//! DistributionIndex: typed view over the artifact registry.
//!
//! ANOLISA component manifests declare *what* a component is; the
//! `DistributionIndex` declares *where* concrete pre-built artifacts live
//! (URL, checksum, signature, backend, os/arch/libc/pkg_base selectors).
//!
//! This module is a pure metadata layer:
//!   * NO network IO,
//!   * NO file download,
//!   * NO signature verification.
//!
//! It only loads TOML and resolves a query to a single matching entry.

use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level DistributionIndex document.
///
/// This is the in-memory shape used by the resolver. The on-disk TOML uses
/// `[[entries]]` array-of-tables so each entry is self-describing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionIndex {
    pub schema_version: u32,
    #[serde(default)]
    pub entries: Vec<DistributionEntry>,
}

/// One concrete artifact binding for a (component, version, channel, target).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DistributionEntry {
    pub component: String,
    pub version: String,
    /// Release channel: "stable" | "beta" | "experimental".
    pub channel: String,
    pub artifact_type: ArtifactType,
    /// Backend hint for the install runner: "rpm" | "deb" | "tar" | "oci" | "file" | ...
    pub backend: String,
    pub url: String,
    /// OS selector: "linux" | "darwin" | ...
    pub os: String,
    /// CPU arch selector: "x86_64" | "aarch64" | "any".
    pub arch: String,
    /// libc selector: "glibc" | "musl" | None (any).
    #[serde(default)]
    pub libc: Option<String>,
    /// OS base selector: "anolis23" | "anolis8" | None (any).
    #[serde(default)]
    pub pkg_base: Option<String>,
    /// Allowed install modes: e.g. ["system", "user"].
    #[serde(default)]
    pub install_modes: Vec<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    /// Sibling components this artifact depends on (by component name).
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// Supported on-the-wire artifact types.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Rpm,
    Deb,
    Tar,
    Oci,
    File,
    Binary,
}

/// Resolver query. Borrowed so callers can build it without allocating.
#[derive(Debug, Clone)]
pub struct ResolveQuery<'a> {
    pub component: &'a str,
    /// None => pick highest version in the channel.
    pub version: Option<&'a str>,
    /// None => "stable".
    pub channel: Option<&'a str>,
    pub install_mode: &'a str,
    pub os: &'a str,
    pub arch: &'a str,
    pub libc: Option<&'a str>,
    pub pkg_base: Option<&'a str>,
}

/// Resolver errors. These are vocabulary errors — IO and parse errors live in
/// `DistributionError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    #[error("no distribution entry matches the query")]
    NotFound,
    #[error("multiple distribution entries match the query ({} candidates)", .0.len())]
    Ambiguous(Vec<DistributionEntry>),
    #[error("install mode is not supported by any candidate entry")]
    UnsupportedMode,
    #[error("matching entry has no sha256 but checksum was requested")]
    ChecksumMissing,
}

/// IO / parse errors when loading an index.
#[derive(Debug, thiserror::Error)]
pub enum DistributionError {
    #[error("cannot read distribution index '{0}': {1}")]
    Io(String, std::io::Error),
    #[error("cannot parse distribution index '{0}': {1}")]
    Parse(String, String),
}

impl DistributionIndex {
    /// Load a `DistributionIndex` from a TOML file on disk.
    pub fn load(path: &Path) -> Result<Self, DistributionError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| DistributionError::Io(path.display().to_string(), e))?;
        Self::from_str(&content)
            .map_err(|e| DistributionError::Parse(path.display().to_string(), e))
    }

    /// Parse from a TOML string. Returned error is the raw `toml` message.
    pub fn from_str(s: &str) -> Result<Self, String> {
        toml::from_str(s).map_err(|e| e.to_string())
    }

    /// Serialize to TOML. Useful for tests and tooling.
    pub fn to_toml_string(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| e.to_string())
    }

    /// Resolve a query to a single matching entry.
    ///
    /// Filter rules (in order):
    ///   1. `component` exact match.
    ///   2. `channel` exact match (default "stable").
    ///   3. `install_mode` must appear in the entry's `install_modes`.
    ///   4. `os` exact match.
    ///   5. `arch` exact match OR entry arch == "any".
    ///   6. `libc` and `pkg_base`: if entry has Some, query must match.
    ///      If entry has None, accepted for any query value.
    ///   7. `version`: if Some, exact match. If None, pick the highest by
    ///      semver if all candidate versions parse, else lexicographic.
    pub fn resolve(&self, q: &ResolveQuery<'_>) -> Result<DistributionEntry, ResolveError> {
        let want_channel = q.channel.unwrap_or("stable");

        // 1-6: filter without considering version.
        let mut candidates: Vec<&DistributionEntry> = self
            .entries
            .iter()
            .filter(|e| e.component == q.component)
            .filter(|e| e.channel == want_channel)
            .filter(|e| e.os == q.os)
            .filter(|e| e.arch == q.arch || e.arch == "any")
            .filter(|e| matches_optional(e.libc.as_deref(), q.libc))
            .filter(|e| matches_optional(e.pkg_base.as_deref(), q.pkg_base))
            .collect();

        if candidates.is_empty() {
            return Err(ResolveError::NotFound);
        }

        // 7a: install_mode filter — track separately so we can distinguish
        // "would have matched but the install mode is wrong" from a generic
        // NotFound.
        let before_mode = candidates.len();
        candidates.retain(|e| e.install_modes.iter().any(|m| m.as_str() == q.install_mode));
        if candidates.is_empty() {
            return if before_mode > 0 {
                Err(ResolveError::UnsupportedMode)
            } else {
                Err(ResolveError::NotFound)
            };
        }

        // 7b: version selection.
        let picked: DistributionEntry = match q.version {
            Some(v) => {
                let filtered: Vec<&DistributionEntry> = candidates
                    .iter()
                    .copied()
                    .filter(|e| e.version == v)
                    .collect();
                match filtered.len() {
                    0 => return Err(ResolveError::NotFound),
                    1 => filtered[0].clone(),
                    _ => {
                        return Err(ResolveError::Ambiguous(
                            filtered.into_iter().cloned().collect(),
                        ));
                    }
                }
            }
            None => pick_highest(&candidates),
        };

        Ok(picked)
    }
}

/// Optional selector match: entry None => wildcard accept; entry Some =>
/// query must be Some and equal.
fn matches_optional(entry_val: Option<&str>, query_val: Option<&str>) -> bool {
    match entry_val {
        None => true,
        Some(ev) => query_val.is_some_and(|qv| qv == ev),
    }
}

/// Pick the entry with the highest version. Uses semver when every candidate
/// version parses; otherwise falls back to lexicographic comparison. With
/// multiple candidates sharing the top version, the first wins (we cannot
/// raise Ambiguous here because the caller asked for `latest`).
fn pick_highest(candidates: &[&DistributionEntry]) -> DistributionEntry {
    debug_assert!(!candidates.is_empty(), "pick_highest needs >=1 candidate");

    let parsed: Option<Vec<Version>> = candidates
        .iter()
        .map(|e| Version::parse(&e.version).ok())
        .collect();

    if let Some(versions) = parsed {
        let (best_idx, _) = versions
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.cmp(b))
            .unwrap_or((0, &versions[0]));
        return candidates[best_idx].clone();
    }

    let best = candidates
        .iter()
        .max_by(|a, b| a.version.cmp(&b.version))
        .copied()
        .unwrap_or(candidates[0]);
    best.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn sample_entry() -> DistributionEntry {
        DistributionEntry {
            component: "agentsight".into(),
            version: "0.1.0".into(),
            channel: "stable".into(),
            artifact_type: ArtifactType::Rpm,
            backend: "rpm".into(),
            url: "https://example.invalid/agentsight-0.1.0.rpm".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            libc: Some("glibc".into()),
            pkg_base: Some("anolis23".into()),
            install_modes: vec!["system".into()],
            sha256: Some("0".repeat(64)),
            signature: None,
            dependencies: vec!["kernel-headers".into()],
        }
    }

    fn linux_x86_query<'a>(component: &'a str, mode: &'a str) -> ResolveQuery<'a> {
        ResolveQuery {
            component,
            version: None,
            channel: None,
            install_mode: mode,
            os: "linux",
            arch: "x86_64",
            libc: Some("glibc"),
            pkg_base: Some("anolis23"),
        }
    }

    #[test]
    fn toml_roundtrip_preserves_entries() {
        let index = DistributionIndex {
            schema_version: 1,
            entries: vec![sample_entry()],
        };

        let serialized = index.to_toml_string().expect("serialize");
        let parsed: DistributionIndex =
            DistributionIndex::from_str(&serialized).expect("deserialize");

        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0], index.entries[0]);
    }

    #[test]
    fn resolve_fixture_agentsight_matches() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../manifests/distribution-index/index.toml");
        let index = DistributionIndex::load(&fixture).expect("load fixture");

        let q = linux_x86_query("agentsight", "system");
        let entry = index.resolve(&q).expect("resolve");

        assert_eq!(entry.component, "agentsight");
        assert_eq!(entry.os, "linux");
        assert_eq!(entry.arch, "x86_64");
        assert!(entry.install_modes.contains(&"system".to_string()));
        assert!(entry.url.contains("agentsight"));
    }

    #[test]
    fn resolve_wrong_arch_returns_not_found() {
        let index = DistributionIndex {
            schema_version: 1,
            entries: vec![sample_entry()],
        };
        let mut q = linux_x86_query("agentsight", "system");
        q.arch = "aarch64";

        assert_eq!(index.resolve(&q), Err(ResolveError::NotFound));
    }

    #[test]
    fn resolve_without_version_picks_highest_semver() {
        let mut newer = sample_entry();
        newer.version = "0.2.0".into();
        newer.url = "https://example.invalid/agentsight-0.2.0.rpm".into();

        let index = DistributionIndex {
            schema_version: 1,
            entries: vec![sample_entry(), newer.clone()],
        };

        let q = linux_x86_query("agentsight", "system");
        let entry = index.resolve(&q).expect("resolve");
        assert_eq!(entry.version, "0.2.0");
        assert_eq!(entry.url, newer.url);
    }

    #[test]
    fn resolve_ambiguous_when_two_entries_share_version_query() {
        // Two entries with the same component/channel/os/arch/version but
        // differing libc=None (wildcard) — both match a query with libc=Some.
        let a = sample_entry();
        let mut b = sample_entry();
        b.libc = None;
        b.url = "https://example.invalid/agentsight-0.1.0.alt.rpm".into();

        let index = DistributionIndex {
            schema_version: 1,
            entries: vec![a, b],
        };

        let mut q = linux_x86_query("agentsight", "system");
        q.version = Some("0.1.0");

        match index.resolve(&q) {
            Err(ResolveError::Ambiguous(list)) => assert_eq!(list.len(), 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn resolve_unsupported_mode_distinguishes_from_not_found() {
        let index = DistributionIndex {
            schema_version: 1,
            entries: vec![sample_entry()],
        };
        let q = linux_x86_query("agentsight", "user");
        assert_eq!(index.resolve(&q), Err(ResolveError::UnsupportedMode));
    }

    #[test]
    fn load_from_tempfile_roundtrips() {
        let index = DistributionIndex {
            schema_version: 1,
            entries: vec![sample_entry()],
        };
        let toml_str = index.to_toml_string().expect("serialize");

        let mut tmp = NamedTempFile::new().expect("tempfile");
        tmp.write_all(toml_str.as_bytes()).expect("write");
        let loaded = DistributionIndex::load(tmp.path()).expect("load");

        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0], index.entries[0]);
    }
}
