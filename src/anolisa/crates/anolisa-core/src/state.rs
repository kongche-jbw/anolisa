//! Installed state tracking (`installed.toml`).
//!
//! `InstalledState` is the on-disk record of every ANOLISA-managed object
//! (capability / component / adapter / osbase) plus the backups and
//! operations that produced them. Persistence is TOML and save is atomic
//! (`tmp` + `rename`) so a crash mid-write cannot leave a truncated state
//! file.
//!
//! See `templates/installed-state.toml` and launch spec §8.1 for the
//! field-level contract.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

/// Current `installed.toml` schema version. Bump on incompatible changes.
pub const STATE_SCHEMA_VERSION: u32 = 1;

/// Default for `bool` fields that should serialise to `true` when absent.
fn default_true() -> bool {
    true
}

/// Install mode reported in `installed.toml`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallMode {
    User,
    System,
}

impl Default for InstallMode {
    fn default() -> Self {
        Self::User
    }
}

/// Discriminator for objects tracked in installed state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Capability,
    Component,
    Adapter,
    Osbase,
}

/// Lifecycle status for an installed object.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectStatus {
    Installed,
    Partial,
    Disabled,
    Failed,
    Adopted,
}

/// Subscription scope attached to an object.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionScope {
    #[default]
    None,
    Registered,
    Entitled,
    Reporting,
}

/// File ownership: ANOLISA-owned vs. external (third-party).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileOwner {
    Anolisa,
    External,
}

/// File installed and owned by ANOLISA.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnedFile {
    pub path: PathBuf,
    pub owner: FileOwner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// External (non-ANOLISA) file that an operation modified. Linked back to
/// the originating [`BackupRecord`] by `backup_id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalModifiedFile {
    pub path: PathBuf,
    pub owner: FileOwner,
    pub backup_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256_after: Option<String>,
}

/// Service unit installed or managed by an object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceRef {
    pub name: String,
    pub manager: String,
    #[serde(default)]
    pub restartable: bool,
    #[serde(default)]
    pub enabled: bool,
}

/// Last-known health probe result for an object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthEntry {
    pub name: String,
    pub status: String,
    pub checked_at: String,
}

/// A single installed object (capability, component, adapter, or osbase).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledObject {
    pub kind: ObjectKind,
    pub name: String,
    pub version: String,
    pub status: ObjectStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution_source: Option<String>,
    pub installed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_operation_id: Option<String>,
    #[serde(default = "default_true")]
    pub managed: bool,
    #[serde(default)]
    pub adopted: bool,
    #[serde(default)]
    pub subscription_scope: SubscriptionScope,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_features: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<OwnedFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_modified_files: Vec<ExternalModifiedFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ServiceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub health: Vec<HealthEntry>,
}

/// Backup metadata recorded when an operation touched an external file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupRecord {
    pub id: String,
    pub operation_id: String,
    pub original_path: PathBuf,
    pub backup_path: PathBuf,
    pub restore_strategy: String,
}

/// Operation record for an `installed.toml` audit trail entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationRecord {
    pub id: String,
    pub command: String,
    pub status: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

/// On-disk record of installed objects, backups, and operation history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledState {
    pub schema_version: u32,
    pub updated_at: String,
    pub install_mode: InstallMode,
    pub prefix: PathBuf,
    pub anolisa_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<InstalledObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backups: Vec<BackupRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<OperationRecord>,
}

impl Default for InstalledState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            updated_at: now_iso8601(),
            install_mode: InstallMode::User,
            prefix: PathBuf::new(),
            anolisa_version: env!("CARGO_PKG_VERSION").to_string(),
            objects: Vec::new(),
            backups: Vec::new(),
            operations: Vec::new(),
        }
    }
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
        snapshot.updated_at = now_iso8601();

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

    /// Insert or replace an object, deduped by `(kind, name)`.
    pub fn upsert_object(&mut self, obj: InstalledObject) {
        if let Some(slot) = self
            .objects
            .iter_mut()
            .find(|o| o.kind == obj.kind && o.name == obj.name)
        {
            *slot = obj;
        } else {
            self.objects.push(obj);
        }
    }

    /// Remove an object by `(kind, name)`, returning the removed value.
    pub fn remove_object(&mut self, kind: ObjectKind, name: &str) -> Option<InstalledObject> {
        let idx = self
            .objects
            .iter()
            .position(|o| o.kind == kind && o.name == name)?;
        Some(self.objects.remove(idx))
    }

    /// Find an object by `(kind, name)`.
    pub fn find_object(&self, kind: ObjectKind, name: &str) -> Option<&InstalledObject> {
        self.objects
            .iter()
            .find(|o| o.kind == kind && o.name == name)
    }

    /// Mutable variant of [`Self::find_object`].
    pub fn find_object_mut(
        &mut self,
        kind: ObjectKind,
        name: &str,
    ) -> Option<&mut InstalledObject> {
        self.objects
            .iter_mut()
            .find(|o| o.kind == kind && o.name == name)
    }

    /// Append a backup record.
    pub fn append_backup(&mut self, b: BackupRecord) {
        self.backups.push(b);
    }

    /// Append an operation record.
    pub fn append_operation(&mut self, op: OperationRecord) {
        self.operations.push(op);
    }
}

fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
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

    fn sample_object(kind: ObjectKind, name: &str, version: &str) -> InstalledObject {
        InstalledObject {
            kind,
            name: name.to_string(),
            version: version.to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: Some("sha256:abc".to_string()),
            distribution_source: Some("builtin".to_string()),
            installed_at: now_iso8601(),
            last_operation_id: Some("op-1".to_string()),
            managed: true,
            adopted: false,
            subscription_scope: SubscriptionScope::None,
            enabled_features: vec!["alpha".to_string()],
            component_refs: vec!["agentsight".to_string()],
            files: vec![OwnedFile {
                path: PathBuf::from("/tmp/anolisa/bin/foo"),
                owner: FileOwner::Anolisa,
                sha256: Some("deadbeef".to_string()),
            }],
            external_modified_files: Vec::new(),
            services: vec![ServiceRef {
                name: "foo.service".to_string(),
                manager: "systemd".to_string(),
                restartable: true,
                enabled: true,
            }],
            health: vec![HealthEntry {
                name: "binary".to_string(),
                status: "ok".to_string(),
                checked_at: now_iso8601(),
            }],
        }
    }

    fn sample_backup(id: &str, op: &str) -> BackupRecord {
        BackupRecord {
            id: id.to_string(),
            operation_id: op.to_string(),
            original_path: PathBuf::from("/etc/openclaw/config.toml"),
            backup_path: PathBuf::from("/var/lib/anolisa/backups/op-1/openclaw/config.toml"),
            restore_strategy: "replace-file".to_string(),
        }
    }

    fn sample_operation(id: &str) -> OperationRecord {
        OperationRecord {
            id: id.to_string(),
            command: "enable agent-observability".to_string(),
            status: "ok".to_string(),
            started_at: now_iso8601(),
            finished_at: Some(now_iso8601()),
        }
    }

    #[test]
    fn default_state_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");

        let state = InstalledState::default();
        state.save(&path).expect("save default");

        let loaded = InstalledState::load(&path).expect("load default");
        assert_eq!(loaded.schema_version, STATE_SCHEMA_VERSION);
        assert_eq!(loaded.install_mode, InstallMode::User);
        assert_eq!(loaded.anolisa_version, env!("CARGO_PKG_VERSION"));
        assert!(loaded.objects.is_empty());
        assert!(loaded.backups.is_empty());
        assert!(loaded.operations.is_empty());
    }

    #[test]
    fn parse_template_round_trip() {
        let template_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("templates")
            .join("installed-state.toml");
        let content = fs::read_to_string(&template_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", template_path.display()));
        let state: InstalledState =
            toml::from_str(&content).expect("template parses into InstalledState");

        assert_eq!(state.schema_version, 1);
        assert_eq!(state.install_mode, InstallMode::User);
        assert!(!state.objects.is_empty(), "expected at least one object");
        assert!(!state.backups.is_empty(), "expected at least one backup");
        assert!(
            !state.operations.is_empty(),
            "expected at least one operation"
        );

        let cap = state
            .objects
            .iter()
            .find(|o| o.kind == ObjectKind::Capability)
            .expect("template has capability object");
        assert_eq!(cap.name, "agent-observability");
        assert!(!cap.external_modified_files.is_empty());
        assert_eq!(
            cap.external_modified_files[0].backup_id,
            state.backups[0].id
        );
    }

    #[test]
    fn upsert_then_find_object() {
        let mut state = InstalledState::default();
        let first = sample_object(ObjectKind::Capability, "agent-observability", "0.1.0");
        state.upsert_object(first);

        let found = state
            .find_object(ObjectKind::Capability, "agent-observability")
            .expect("present after upsert");
        assert_eq!(found.version, "0.1.0");

        let second = sample_object(ObjectKind::Capability, "agent-observability", "0.2.0");
        state.upsert_object(second);
        assert_eq!(state.objects.len(), 1, "upsert dedupes by (kind, name)");
        assert_eq!(
            state
                .find_object(ObjectKind::Capability, "agent-observability")
                .expect("present")
                .version,
            "0.2.0"
        );
    }

    #[test]
    fn remove_object_returns_removed() {
        let mut state = InstalledState::default();
        state.upsert_object(sample_object(ObjectKind::Component, "agentsight", "0.1.0"));

        let removed = state.remove_object(ObjectKind::Component, "agentsight");
        assert!(removed.is_some());
        assert_eq!(removed.expect("just checked").name, "agentsight");

        assert!(
            state
                .remove_object(ObjectKind::Component, "agentsight")
                .is_none()
        );
    }

    #[test]
    fn append_backup_and_operation() {
        let mut state = InstalledState::default();
        assert_eq!(state.backups.len(), 0);
        assert_eq!(state.operations.len(), 0);

        state.append_backup(sample_backup("backup-op-1", "op-1"));
        state.append_operation(sample_operation("op-1"));
        state.append_operation(sample_operation("op-2"));

        assert_eq!(state.backups.len(), 1);
        assert_eq!(state.operations.len(), 2);
    }

    #[test]
    fn external_modified_files_links_backup_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");

        let mut state = InstalledState::default();
        let mut obj = sample_object(ObjectKind::Adapter, "openclaw", "0.1.0");
        obj.external_modified_files.push(ExternalModifiedFile {
            path: PathBuf::from("/etc/openclaw/config.toml"),
            owner: FileOwner::External,
            backup_id: "backup-op-1".to_string(),
            sha256_before: Some("before".to_string()),
            sha256_after: Some("after".to_string()),
        });
        state.upsert_object(obj);
        state.append_backup(sample_backup("backup-op-1", "op-1"));
        state.append_operation(sample_operation("op-1"));

        state.save(&path).expect("save");
        let loaded = InstalledState::load(&path).expect("load");

        let adapter = loaded
            .find_object(ObjectKind::Adapter, "openclaw")
            .expect("adapter present");
        assert_eq!(adapter.external_modified_files.len(), 1);
        assert_eq!(
            adapter.external_modified_files[0].backup_id,
            loaded.backups[0].id
        );
    }

    #[test]
    fn serialize_skips_optional_none() {
        let mut state = InstalledState::default();
        let mut obj = sample_object(ObjectKind::Component, "agentsight", "0.1.0");
        obj.manifest_digest = None;
        obj.distribution_source = None;
        obj.last_operation_id = None;
        state.upsert_object(obj);

        let rendered = toml::to_string_pretty(&state).expect("serialize");
        assert!(
            !rendered.contains("manifest_digest"),
            "None manifest_digest must be skipped, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("distribution_source"),
            "None distribution_source must be skipped"
        );
        assert!(
            !rendered.contains("last_operation_id"),
            "None last_operation_id must be skipped"
        );
    }
}
