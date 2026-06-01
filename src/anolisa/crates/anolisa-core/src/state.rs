//! Installed state tracking (`installed.toml`).
//!
//! `InstalledState` is the on-disk record of which capabilities and
//! components ANOLISA has enabled or installed. Persistence is TOML and
//! save is atomic (tmp + rename) so a crash mid-write cannot leave a
//! truncated state file.
//!
//! See launch spec §8.1 for the field-level requirements.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Current `installed.toml` schema version. Bump on incompatible changes.
pub const STATE_SCHEMA_VERSION: u32 = 1;

/// On-disk record of installed capabilities and components.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledState {
    pub schema_version: u32,
    #[serde(default)]
    pub install_mode: String,
    #[serde(default)]
    pub prefix: PathBuf,
    #[serde(default)]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub capabilities: BTreeMap<String, CapabilityRecord>,
    #[serde(default)]
    pub components: BTreeMap<String, ComponentRecord>,
}

impl Default for InstalledState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            install_mode: String::new(),
            prefix: PathBuf::new(),
            updated_at: Utc::now(),
            capabilities: BTreeMap::new(),
            components: BTreeMap::new(),
        }
    }
}

/// Per-capability record: what was enabled and when.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRecord {
    pub name: String,
    #[serde(default)]
    pub enabled_features: Vec<String>,
    #[serde(default)]
    pub components: Vec<String>,
    pub enabled_at: DateTime<Utc>,
}

/// Per-component record: provenance and installed files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentRecord {
    pub name: String,
    pub version: String,
    pub install_mode: String,
    #[serde(default)]
    pub install_files: Vec<PathBuf>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub installed_at: DateTime<Utc>,
}

/// Errors raised while loading or persisting [`InstalledState`].
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("io error while accessing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse installed state at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to serialize installed state: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl InstalledState {
    /// Load state from `path`. Returns a fresh default if the file does
    /// not exist (first-run case).
    pub fn load(path: &Path) -> Result<Self, StateError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path).map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&content).map_err(|source| StateError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Atomically write state to `path` (`tmp` + `rename`). Refreshes
    /// `updated_at` to the current UTC time before serialising.
    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        let mut snapshot = self.clone();
        snapshot.updated_at = Utc::now();

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| StateError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
        }

        let content = toml::to_string_pretty(&snapshot)?;

        let tmp = tmp_path_for(path);
        fs::write(&tmp, content.as_bytes()).map_err(|source| StateError::Io {
            path: tmp.clone(),
            source,
        })?;
        fs::rename(&tmp, path).map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    /// Insert or replace a capability record, keyed by `name`.
    pub fn upsert_capability(&mut self, record: CapabilityRecord) {
        self.capabilities.insert(record.name.clone(), record);
    }

    /// Remove a capability record by name; returns `true` if present.
    pub fn remove_capability(&mut self, name: &str) -> bool {
        self.capabilities.remove(name).is_some()
    }

    /// Insert or replace a component record, keyed by `name`.
    pub fn upsert_component(&mut self, record: ComponentRecord) {
        self.components.insert(record.name.clone(), record);
    }

    /// Remove a component record by name; returns `true` if present.
    pub fn remove_component(&mut self, name: &str) -> bool {
        self.components.remove(name).is_some()
    }

    /// True iff a component is recorded.
    pub fn is_installed(&self, component: &str) -> bool {
        self.components.contains_key(component)
    }
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "installed.toml".to_string());
    tmp.set_file_name(format!(".{file_name}.tmp"));
    tmp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_component(name: &str) -> ComponentRecord {
        ComponentRecord {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            install_mode: "user".to_string(),
            install_files: vec![PathBuf::from("/tmp/anolisa/bin/foo")],
            services: vec!["foo.service".to_string()],
            source_url: Some("https://example.invalid/foo.tar.gz".to_string()),
            sha256: Some("deadbeef".to_string()),
            installed_at: Utc::now(),
        }
    }

    fn sample_capability(name: &str) -> CapabilityRecord {
        CapabilityRecord {
            name: name.to_string(),
            enabled_features: vec!["alpha".to_string()],
            components: vec!["foo".to_string()],
            enabled_at: Utc::now(),
        }
    }

    #[test]
    fn load_missing_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installed.toml");
        let state = InstalledState::load(&path).unwrap();
        assert_eq!(state.schema_version, STATE_SCHEMA_VERSION);
        assert!(state.components.is_empty());
        assert!(state.capabilities.is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("installed.toml");

        let mut state = InstalledState::default();
        state.install_mode = "user".to_string();
        state.prefix = PathBuf::from("/home/dev/.local");
        state.upsert_component(sample_component("foo"));
        state.upsert_capability(sample_capability("agent-observability"));

        state.save(&path).unwrap();
        assert!(path.exists());

        let loaded = InstalledState::load(&path).unwrap();
        assert_eq!(loaded.install_mode, "user");
        assert_eq!(loaded.prefix, PathBuf::from("/home/dev/.local"));
        assert!(loaded.components.contains_key("foo"));
        assert_eq!(loaded.components["foo"].version, "0.1.0");
        assert!(loaded.capabilities.contains_key("agent-observability"));
    }

    #[test]
    fn upsert_replaces_existing_record() {
        let mut state = InstalledState::default();
        let mut first = sample_component("foo");
        first.version = "0.1.0".to_string();
        state.upsert_component(first);

        let mut second = sample_component("foo");
        second.version = "0.2.0".to_string();
        state.upsert_component(second);

        assert_eq!(state.components.len(), 1);
        assert_eq!(state.components["foo"].version, "0.2.0");
    }

    #[test]
    fn remove_component_reports_presence() {
        let mut state = InstalledState::default();
        state.upsert_component(sample_component("foo"));
        assert!(state.remove_component("foo"));
        assert!(!state.remove_component("foo"));
        assert!(!state.is_installed("foo"));
    }

    #[test]
    fn remove_capability_reports_presence() {
        let mut state = InstalledState::default();
        state.upsert_capability(sample_capability("agent-observability"));
        assert!(state.remove_capability("agent-observability"));
        assert!(!state.remove_capability("agent-observability"));
    }

    #[test]
    fn save_uses_atomic_tmp_then_rename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installed.toml");

        let state = InstalledState::default();
        state.save(&path).unwrap();

        // Ensure no leftover tmp file once save completes successfully.
        let tmp = tmp_path_for(&path);
        assert!(!tmp.exists(), "tmp file should be renamed away on success");
        assert!(path.exists());
    }
}
