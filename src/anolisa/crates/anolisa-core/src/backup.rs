//! Backup planning skeleton.
//!
//! At P1-A this module only **plans** where backups would be stored —
//! actual filesystem copies, checksums, and restore are wired in later
//! milestones together with `Transaction` (launch spec §8.2 / §8.3).
//!
//! The plan maps every input `src` path to a stable location under
//! `<backup_root>/<id>/<flattened-src>`. The flattening converts the
//! source path to a relative form (`/` separators replaced with `__`)
//! so we can dump everything into a single per-backup directory without
//! permission surprises.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A planned (or completed) backup set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupSet {
    /// Stable backup id (timestamp- or uuid-based).
    pub id: String,
    /// ISO8601 UTC timestamp when the plan was created.
    pub created_at: String,
    /// One entry per source path captured by this set.
    pub entries: Vec<BackupEntry>,
}

/// A single backup mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    /// Original on-disk path the operator wants to protect.
    pub src: PathBuf,
    /// Where the copy would live under the backup root.
    pub stored_at: PathBuf,
    /// Recorded checksum once the copy completes (planning phase: None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

impl BackupSet {
    /// Build a plan that maps every `paths` entry to a target under
    /// `<backup_root>/<id>/`.
    pub fn plan(id: String, paths: Vec<PathBuf>, backup_root: &Path) -> Self {
        let root = backup_root.join(&id);
        let entries = paths
            .into_iter()
            .map(|src| {
                let stored_at = root.join(flatten_src(&src));
                BackupEntry {
                    src,
                    stored_at,
                    sha256: None,
                }
            })
            .collect();
        Self {
            id,
            created_at: chrono::Utc::now().to_rfc3339(),
            entries,
        }
    }
}

/// Convert `/etc/anolisa/foo.toml` to `etc__anolisa__foo.toml` so it can
/// live alongside other captured files in the backup root.
fn flatten_src(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let trimmed = raw.trim_start_matches('/');
    let mut out = trimmed.replace('/', "__");
    if out.is_empty() {
        out = "_root".to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_emits_one_entry_per_path() {
        let backup_root = PathBuf::from("/var/lib/anolisa/backups");
        let plan = BackupSet::plan(
            "op-20260601-001".to_string(),
            vec![
                PathBuf::from("/etc/openclaw/config.json"),
                PathBuf::from("/etc/anolisa/features.toml"),
            ],
            &backup_root,
        );
        assert_eq!(plan.id, "op-20260601-001");
        assert_eq!(plan.entries.len(), 2);
        assert!(plan.entries.iter().all(|e| e.sha256.is_none()));
    }

    #[test]
    fn plan_uses_id_namespaced_stored_at() {
        let backup_root = PathBuf::from("/var/lib/anolisa/backups");
        let plan = BackupSet::plan(
            "op-1".to_string(),
            vec![PathBuf::from("/etc/openclaw/config.json")],
            &backup_root,
        );
        let entry = &plan.entries[0];
        assert_eq!(
            entry.stored_at,
            PathBuf::from("/var/lib/anolisa/backups/op-1/etc__openclaw__config.json")
        );
        assert_eq!(entry.src, PathBuf::from("/etc/openclaw/config.json"));
    }

    #[test]
    fn plan_handles_relative_and_root_paths() {
        let backup_root = PathBuf::from("/tmp/backups");
        let plan = BackupSet::plan(
            "op-2".to_string(),
            vec![PathBuf::from("relative/file"), PathBuf::from("/")],
            &backup_root,
        );
        assert_eq!(
            plan.entries[0].stored_at,
            PathBuf::from("/tmp/backups/op-2/relative__file")
        );
        assert_eq!(
            plan.entries[1].stored_at,
            PathBuf::from("/tmp/backups/op-2/_root")
        );
    }
}
