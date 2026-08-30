use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Metadata captured from the authorized workspace directory object.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceDirectoryMetadata {
    /// Owning user captured from the authorized directory fd.
    pub uid: u32,
    /// Owning group captured from the authorized directory fd.
    pub gid: u32,
    /// Full Unix mode captured from the authorized directory fd.
    pub mode: u32,
}

/// Backend type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendType {
    BtrfsLoop, // btrfs on a loop device (current implementation)
    BtrfsBase, // native btrfs partition / subvolume
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendType::BtrfsLoop => write!(f, "btrfs-loop"),
            BackendType::BtrfsBase => write!(f, "btrfs-base"),
        }
    }
}

/// Cleanup result
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CleanupResult {
    pub removed: Vec<String>, // IDs of snapshots that were cleaned up
    pub kept: usize,          // number of snapshots retained
}

/// GC result (generation cleanup)
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct GcResult {
    pub generations_removed: usize,
}

/// Environment check status
#[derive(Debug, Serialize, Deserialize)]
pub struct EnvironmentStatus {
    pub backend: BackendType,
    pub healthy: bool,
    pub details: Vec<String>, // descriptions of individual check items
}

/// StorageBackend trait — every storage backend must implement this
///
/// The orchestration layer (dispatcher/workspace_mgr/snapshot_mgr) invokes storage
/// operations through this trait. WS ID generation, index.json management, and
/// daemon state registration live in the orchestration layer, not in the trait.
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Return the backend type identifier
    fn backend_type(&self) -> BackendType;

    /// Return the backend data root (parent of workspace subvolumes and snapshots)
    fn data_root(&self) -> &std::path::Path;

    /// Return the snapshot storage root
    fn snapshots_root(&self) -> &std::path::Path;

    /// Return the kernel-backed identity of the current writable workspace generation.
    ///
    /// Backends must override this only when they can bind the identity to the live
    /// storage object. The default fails closed so test and future backends cannot
    /// accidentally substitute a logical or path-derived token.
    async fn live_generation(
        &self,
        _ws_id: &str,
    ) -> anyhow::Result<crate::WorkspaceGenerationTokenV2> {
        anyhow::bail!("storage backend does not support secure live-generation identity")
    }

    /// Legacy path-only workspace initialization hook.
    ///
    /// The daemon uses [`StorageBackend::init_workspace_from_source`] so the
    /// source is bound to the directory authorized from peer credentials. This
    /// default fails closed for backend implementations that only support paths.
    /// - btrfs-loop: rsync + create img + mkfs + losetup + mount + subvol + symlink
    async fn init_workspace(
        &self,
        _original_path: &str,
        _ws_id: &str,
    ) -> anyhow::Result<crate::WorkspaceInfo> {
        anyhow::bail!("storage backend requires fd-bound workspace initialization")
    }

    /// Import an atomically claimed workspace through an fd-backed source path.
    async fn init_workspace_from_source(
        &self,
        _original_path: &str,
        _source_path: &Path,
        _metadata: WorkspaceDirectoryMetadata,
        _ws_id: &str,
    ) -> anyhow::Result<crate::WorkspaceInfo> {
        anyhow::bail!("storage backend does not support fd-bound workspace initialization")
    }

    /// Create a snapshot
    /// - btrfs: btrfs subvolume snapshot -r
    async fn create_snapshot(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<()>;

    /// Roll back to a specific snapshot
    /// - all btrfs backends: create a writable subvolume + atomic symlink swap (ln -s + mv -T)
    async fn rollback(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<PathBuf>;

    /// Delete a snapshot subvolume
    async fn delete_snapshot(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<()>;

    /// Recover the workspace back to a plain directory (undo init)
    /// - btrfs-base: rsync restore + remove symlink + delete subvolume (no umount loop)
    /// - btrfs-loop: rsync restore + remove symlink + delete subvolume + umount + losetup -d + remove img
    async fn recover_workspace(&self, ws_id: &str, original_path: &str) -> anyhow::Result<()>;

    /// Recover into a registered destination pinned by its open parent fd.
    async fn recover_workspace_to_destination(
        &self,
        _ws_id: &str,
        _parent: &std::fs::File,
        _name: &OsStr,
    ) -> anyhow::Result<()> {
        anyhow::bail!("storage backend does not support fd-bound workspace recovery")
    }

    /// Compute diff between two snapshots, or between a snapshot and the live workspace.
    async fn diff(
        &self,
        ws_id: &str,
        from: &str,
        to: Option<&str>,
    ) -> anyhow::Result<Vec<crate::DiffEntry>>;

    /// Clean up old snapshots (retain the most recent `keep` + all pinned ones)
    /// Returns the list of deleted snapshot IDs
    async fn cleanup_snapshots(
        &self,
        ws_id: &str,
        snapshot_ids: &[String],
    ) -> anyhow::Result<Vec<String>>;

    /// Fork an independent workspace from a snapshot (reserved)
    async fn fork(&self, ws_id: &str, snapshot_id: &str, new_ws_id: &str) -> anyhow::Result<()>;

    /// Clean up old generations (reserved)
    async fn gc_generations(&self, ws_id: &str) -> anyhow::Result<GcResult>;

    /// Environment check
    async fn check_environment(&self) -> anyhow::Result<EnvironmentStatus>;

    /// Get filesystem usage (total, used) in bytes
    async fn get_usage(&self) -> anyhow::Result<(u64, u64)>;

    /// Prepare the backend for workspace operations.
    async fn bootstrap(&self, _config: &crate::DaemonConfig) -> anyhow::Result<()> {
        Ok(())
    }

    /// Optional hook: BtrfsLoop reports its on-disk image state for state.json.
    async fn loop_img_state(&self) -> Option<crate::persist::LoopImgState> {
        None
    }
}
