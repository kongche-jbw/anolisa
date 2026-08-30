use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use async_trait::async_trait;
use nix::unistd::{chown, Gid, Uid};
use tokio::process::Command;
use tracing::{error, info, warn};

use ws_ckpt_common::backend::*;
use ws_ckpt_common::{
    DaemonConfig, DiffEntry, WorkspaceGenerationTokenV2, WorkspaceInfo, SNAPSHOTS_DIR,
};

use super::{btrfs_common, btrfs_identity, rollback_recovery};
use btrfs_common::backup_path_for;

/// Deployment scenario for BtrfsBase backend.
#[derive(Debug, Clone, Copy)]
pub enum BtrfsBaseScenario {
    /// Scenario A: workspace already on btrfs partition, cp --reflink COW
    InPlace,
    /// Scenario B: workspace on non-btrfs disk, needs rsync migration to btrfs data disk
    CrossDisk,
}

pub struct BtrfsBaseBackend {
    /// Data root on the btrfs partition (e.g. <btrfs_mount>/ws-ckpt-data)
    data_root: PathBuf,
    /// Snapshot storage directory: {data_root}/snapshots
    snapshots_dir: PathBuf,
    /// Deployment scenario
    scenario: BtrfsBaseScenario,
}

impl BtrfsBaseBackend {
    pub fn new(btrfs_mount: PathBuf, scenario: BtrfsBaseScenario) -> Self {
        Self::from_data_root(btrfs_mount.join("ws-ckpt-data"), scenario)
    }

    /// Build a backend whose data root is exactly the configured path.
    pub fn from_data_root(data_root: PathBuf, scenario: BtrfsBaseScenario) -> Self {
        let snapshots_dir = data_root.join(SNAPSHOTS_DIR);
        Self {
            data_root,
            snapshots_dir,
            scenario,
        }
    }

    async fn recover_interrupted_rollbacks(&self) -> anyhow::Result<()> {
        rollback_recovery::recover_interrupted_rollbacks(&self.data_root)
            .await
            .context("failed to recover interrupted BtrfsBase rollback")
    }

    async fn import_claimed_storage(
        &self,
        source_path: &Path,
        metadata: WorkspaceDirectoryMetadata,
        ws_id: &str,
    ) -> anyhow::Result<()> {
        let subvol_path = self.data_root.join(ws_id);
        let snap_dir = self.snapshots_dir.join(ws_id);
        tokio::fs::create_dir_all(&self.data_root).await?;
        btrfs_common::create_subvolume(&subvol_path).await?;
        if let Err(error) = async {
            tokio::fs::create_dir_all(&snap_dir).await?;
            let status = match self.scenario {
                BtrfsBaseScenario::InPlace => {
                    Command::new("cp")
                        .args(["-a", "--reflink=always"])
                        .arg(source_path.join("."))
                        .arg(&subvol_path)
                        .status()
                        .await?
                }
                BtrfsBaseScenario::CrossDisk => {
                    Command::new("rsync")
                        .arg("-a")
                        .arg(format!("{}/", source_path.display()))
                        .arg(&subvol_path)
                        .status()
                        .await?
                }
            };
            if !status.success() {
                bail!(
                    "workspace import failed with exit code: {:?}",
                    status.code()
                );
            }
            restore_root_metadata(&subvol_path, metadata.uid, metadata.gid, metadata.mode).await
        }
        .await
        {
            let _ = tokio::fs::remove_dir_all(&snap_dir).await;
            let _ = btrfs_common::delete_subvolume(&subvol_path).await;
            return Err(error);
        }
        Ok(())
    }
}

async fn restore_root_metadata(path: &Path, uid: u32, gid: u32, mode: u32) -> anyhow::Result<()> {
    let current = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to inspect replacement root {}", path.display()))?;
    if current.uid() != uid || current.gid() != gid {
        chown(path, Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid)))
            .with_context(|| format!("failed to restore ownership on {}", path.display()))?;
    }
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .await
        .with_context(|| format!("failed to restore permissions on {}", path.display()))?;
    Ok(())
}

#[async_trait]
impl StorageBackend for BtrfsBaseBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::BtrfsBase
    }

    fn data_root(&self) -> &Path {
        &self.data_root
    }

    fn snapshots_root(&self) -> &Path {
        &self.snapshots_dir
    }

    async fn live_generation(&self, ws_id: &str) -> anyhow::Result<WorkspaceGenerationTokenV2> {
        btrfs_identity::live_generation(&self.data_root, ws_id)
    }

    async fn init_workspace_from_source(
        &self,
        original_path: &str,
        source_path: &Path,
        metadata: WorkspaceDirectoryMetadata,
        ws_id: &str,
    ) -> anyhow::Result<WorkspaceInfo> {
        self.import_claimed_storage(source_path, metadata, ws_id)
            .await?;
        Ok(WorkspaceInfo {
            ws_id: ws_id.to_string(),
            path: original_path.to_string(),
            snapshot_count: 0,
        })
    }

    async fn create_snapshot(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<()> {
        let ws_subvol = self.data_root.join(ws_id);
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);
        btrfs_common::create_snapshot(&ws_subvol, &snap_path, true).await
    }

    async fn rollback(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<PathBuf> {
        let ws_path = self.data_root.join(ws_id);
        let tmp_path = self.data_root.join(format!("{}.rollback-tmp", ws_id));
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);

        // Verify ws_path is a real subvolume, not a symlink
        let metadata = tokio::fs::symlink_metadata(&ws_path)
            .await
            .context("Failed to read workspace metadata")?;
        if metadata.file_type().is_symlink() {
            bail!("workspace path {:?} is a symlink, expected btrfs subvolume; aborting rollback to prevent symlink chain corruption", ws_path);
        }

        // Warmup snapshot metadata cache
        btrfs_common::warmup_snapshot_metadata(&snap_path).await;

        // Move current workspace aside
        tokio::fs::rename(&ws_path, &tmp_path).await?;

        // Create writable snapshot from target
        match btrfs_common::create_snapshot(&snap_path, &ws_path, false).await {
            Ok(()) => {}
            Err(e) => {
                // Rollback protection: restore original workspace
                error!("rollback snapshot failed, restoring original: {}", e);
                tokio::fs::rename(&tmp_path, &ws_path).await?;
                return Err(e);
            }
        }

        // Clean up old subvolume (non-fatal)
        if let Err(e) = btrfs_common::delete_subvolume(&tmp_path).await {
            warn!("failed to delete old subvolume (non-fatal): {}", e);
        }

        Ok(ws_path)
    }

    async fn delete_snapshot(&self, ws_id: &str, snapshot_id: &str) -> anyhow::Result<()> {
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);
        btrfs_common::delete_subvolume(&snap_path).await
    }

    async fn recover_workspace(&self, ws_id: &str, original_path: &str) -> anyhow::Result<()> {
        let subvol_path = self.data_root.join(ws_id);
        let snap_base = self.snapshots_dir.join(ws_id);

        // Copy and publish through open directory fds. The workspace owner can
        // mutate its parent concurrently, so path-based rsync/chown is unsafe.
        btrfs_common::restore_workspace_from_subvolume(&subvol_path, Path::new(original_path))
            .await?;

        // 3. Delete all snapshot subvolumes by scanning the filesystem directory
        if let Ok(mut entries) = tokio::fs::read_dir(&snap_base).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    if let Err(e) = btrfs_common::delete_subvolume(&path).await {
                        warn!("failed to delete snapshot subvolume {:?}: {:#}", path, e);
                    }
                }
            }
        }

        // 4. Delete workspace subvolume
        if let Err(e) = btrfs_common::delete_subvolume(&subvol_path).await {
            warn!("failed to delete workspace subvolume {}: {:#}", ws_id, e);
        }

        // 5. Remove snapshots/{ws_id} directory
        if let Err(e) = tokio::fs::remove_dir_all(&snap_base).await {
            warn!("failed to remove snapshots dir {:?}: {}", snap_base, e);
        }

        // 6. Clean orphan `.pre-init-bak` if it still exists (prior interrupted
        //    init). Safe to remove at this point — subvol is gone, original_path
        //    has been restored as a normal directory in steps above.
        let backup_path = backup_path_for(original_path);
        if tokio::fs::symlink_metadata(&backup_path).await.is_ok() {
            if let Err(e) = tokio::fs::remove_dir_all(&backup_path).await {
                warn!("failed to clean orphan backup {:?}: {:#}", backup_path, e);
            } else {
                info!("cleaned orphan backup {:?} during recover", backup_path);
            }
        }

        // NOTE: BtrfsBase does NOT need umount, losetup -d, or img deletion
        // (that's the key difference from BtrfsLoop)

        Ok(())
    }

    async fn recover_workspace_to_destination(
        &self,
        ws_id: &str,
        parent: &std::fs::File,
        name: &std::ffi::OsStr,
    ) -> anyhow::Result<()> {
        let subvol_path = self.data_root.join(ws_id);
        let snap_base = self.snapshots_dir.join(ws_id);
        btrfs_common::restore_workspace_from_subvolume_anchored(&subvol_path, parent, name).await?;
        if let Ok(mut entries) = tokio::fs::read_dir(&snap_base).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    if let Err(error) = btrfs_common::delete_subvolume(&path).await {
                        warn!("failed to delete snapshot subvolume {:?}: {error:#}", path);
                    }
                }
            }
        }
        if let Err(error) = btrfs_common::delete_subvolume(&subvol_path).await {
            warn!("failed to delete workspace subvolume {ws_id}: {error:#}");
        }
        if let Err(error) = tokio::fs::remove_dir_all(&snap_base).await {
            warn!("failed to remove snapshots dir {:?}: {error}", snap_base);
        }
        Ok(())
    }

    async fn diff(
        &self,
        ws_id: &str,
        from: &str,
        to: Option<&str>,
    ) -> anyhow::Result<Vec<DiffEntry>> {
        let snap_base = self.snapshots_dir.join(ws_id);
        let snap_from = snap_base.join(from);
        match to {
            Some(id) => btrfs_common::diff_between_snapshots(&snap_from, &snap_base.join(id)).await,
            None => {
                let live = self.data_root.join(ws_id);
                btrfs_common::diff_against_live(&snap_from, &live, &snap_base).await
            }
        }
    }

    async fn cleanup_snapshots(
        &self,
        ws_id: &str,
        snapshot_ids: &[String],
    ) -> anyhow::Result<Vec<String>> {
        let snap_dir = self.snapshots_dir.join(ws_id);
        let mut removed = Vec::new();
        for snap_id in snapshot_ids {
            let snap_path = snap_dir.join(snap_id);
            match btrfs_common::delete_subvolume(&snap_path).await {
                Ok(()) => {
                    removed.push(snap_id.clone());
                    info!("cleanup: removed snapshot {}", snap_id);
                }
                Err(e) => {
                    warn!("cleanup: failed to delete snapshot {}: {:#}", snap_id, e);
                }
            }
        }
        Ok(removed)
    }

    async fn fork(&self, ws_id: &str, snapshot_id: &str, new_ws_id: &str) -> anyhow::Result<()> {
        let snap_path = self.snapshots_dir.join(ws_id).join(snapshot_id);
        let new_ws_path = self.data_root.join(new_ws_id);
        btrfs_common::create_snapshot(&snap_path, &new_ws_path, false).await
    }

    async fn gc_generations(&self, _ws_id: &str) -> anyhow::Result<GcResult> {
        Ok(GcResult::default())
    }

    async fn check_environment(&self) -> anyhow::Result<EnvironmentStatus> {
        let mut details = Vec::new();
        let mut healthy = true;

        // Check btrfs-progs
        match Command::new("which").arg("btrfs").output().await {
            Ok(output) if output.status.success() => {
                details.push("btrfs-progs: installed".to_string())
            }
            _ => {
                healthy = false;
                details.push("btrfs-progs: NOT installed".to_string());
            }
        }

        // Check root privileges
        if nix::unistd::geteuid().is_root() {
            details.push("privileges: root".to_string());
        } else {
            healthy = false;
            details.push("privileges: NOT root".to_string());
        }

        // Check btrfs partition availability
        if btrfs_common::is_on_btrfs(&self.data_root).await {
            details.push(format!(
                "btrfs partition: {} available",
                self.data_root.display()
            ));
        } else {
            // data_root might not exist yet; check parent
            let parent = self.data_root.parent().unwrap_or(&self.data_root);
            if btrfs_common::is_on_btrfs(parent).await {
                details.push(format!("btrfs partition: {} available", parent.display()));
            } else {
                healthy = false;
                details.push("btrfs partition: NOT available".to_string());
            }
        }

        // Check write permission on data_root (or its parent)
        let check_path = if self.data_root.exists() {
            &self.data_root
        } else {
            self.data_root.parent().unwrap_or(&self.data_root)
        };
        match tokio::fs::metadata(check_path).await {
            Ok(meta) => {
                let mode = meta.mode();
                if mode & 0o200 != 0 {
                    details.push("write permission: ok".to_string());
                } else {
                    healthy = false;
                    details.push("write permission: DENIED".to_string());
                }
            }
            Err(_) => {
                healthy = false;
                details.push("write permission: path not accessible".to_string());
            }
        }

        Ok(EnvironmentStatus {
            backend: BackendType::BtrfsBase,
            healthy,
            details,
        })
    }

    async fn get_usage(&self) -> anyhow::Result<(u64, u64)> {
        btrfs_common::get_filesystem_usage(&self.data_root).await
    }

    /// Ensure data_root and snapshots_dir exist on the already-mounted btrfs partition.
    async fn bootstrap(&self, _config: &DaemonConfig) -> anyhow::Result<()> {
        for dir in [&self.data_root, &self.snapshots_dir] {
            tokio::fs::create_dir_all(dir)
                .await
                .with_context(|| format!("Failed to ensure directory exists: {:?}", dir))?;
        }
        // Startup awaits bootstrap before rebuilding workspace watchers.
        self.recover_interrupted_rollbacks().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::{restore_root_metadata, BtrfsBaseBackend, BtrfsBaseScenario};
    use ws_ckpt_common::backend::StorageBackend;
    use ws_ckpt_common::{CleanupRetention, DaemonConfig, SNAPSHOTS_DIR};

    fn dummy_config() -> DaemonConfig {
        DaemonConfig {
            mount_path: std::path::PathBuf::from("/tmp/unused"),
            socket_path: std::path::PathBuf::from("/tmp/unused.sock"),
            workspace_root: None,
            log_level: "info".to_string(),
            auto_cleanup: false,
            auto_cleanup_keep: CleanupRetention::Count(20),
            auto_cleanup_interval_secs: 86_400,
            health_check_interval_secs: 300,
            backend_type: "btrfs-base".to_string(),
            img_size: 1,
            img_max_percent: 1.0,
            min_free_bytes: 0,
            min_free_percent: 0.0,
        }
    }

    #[tokio::test]
    async fn bootstrap_creates_data_root_and_snapshots_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = BtrfsBaseBackend::new(tmp.path().to_path_buf(), BtrfsBaseScenario::InPlace);
        let data_root = tmp.path().join("ws-ckpt-data");
        let snapshots_dir = data_root.join(ws_ckpt_common::SNAPSHOTS_DIR);

        backend.bootstrap(&dummy_config()).await.unwrap();

        assert!(data_root.is_dir(), "data_root must be created");
        assert!(snapshots_dir.is_dir(), "snapshots_dir must be created");
    }

    #[tokio::test]
    async fn bootstrap_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = BtrfsBaseBackend::new(tmp.path().to_path_buf(), BtrfsBaseScenario::InPlace);

        backend.bootstrap(&dummy_config()).await.unwrap();
        // A second call on existing directories must succeed.
        backend.bootstrap(&dummy_config()).await.unwrap();
    }

    #[test]
    fn explicit_data_root_is_not_rewritten() {
        let configured = std::path::PathBuf::from("/var/lib/anolisa-data/ws-ckpt");
        let backend =
            BtrfsBaseBackend::from_data_root(configured.clone(), BtrfsBaseScenario::CrossDisk);

        assert_eq!(backend.data_root(), configured);
        assert_eq!(backend.snapshots_root(), configured.join(SNAPSHOTS_DIR));
    }

    #[tokio::test]
    async fn replacement_root_preserves_mode_0750() {
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("original");
        let replacement = tmp.path().join("replacement");
        tokio::fs::create_dir(&original).await.unwrap();
        tokio::fs::create_dir(&replacement).await.unwrap();
        tokio::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o750))
            .await
            .unwrap();
        tokio::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o777))
            .await
            .unwrap();
        let metadata = tokio::fs::metadata(&original).await.unwrap();

        restore_root_metadata(
            &replacement,
            std::os::unix::fs::MetadataExt::uid(&metadata),
            std::os::unix::fs::MetadataExt::gid(&metadata),
            std::os::unix::fs::MetadataExt::mode(&metadata),
        )
        .await
        .unwrap();

        let replacement_mode = tokio::fs::metadata(&replacement)
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(replacement_mode, 0o750);
    }

    #[tokio::test]
    async fn bootstrap_recovery_scans_backend_data_root_not_config_mount_path() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = BtrfsBaseBackend::new(tmp.path().to_path_buf(), BtrfsBaseScenario::InPlace);
        let data_root = tmp.path().join("ws-ckpt-data");
        tokio::fs::create_dir_all(&data_root).await.unwrap();
        let candidate = data_root.join("ws-abc123.rollback-tmp");
        tokio::fs::write(&candidate, b"foreign").await.unwrap();
        tokio::fs::create_dir_all(data_root.join(SNAPSHOTS_DIR).join("ws-abc123"))
            .await
            .unwrap();

        let error = backend.bootstrap(&dummy_config()).await.unwrap_err();

        assert!(format!("{error:#}").contains("ambiguous interrupted rollback"));
        assert!(candidate.exists(), "unsafe candidate must be preserved");
    }
}
