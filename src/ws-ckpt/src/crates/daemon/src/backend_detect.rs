use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tracing::info;

use ws_ckpt_common::backend::{BackendType, StorageBackend};
use ws_ckpt_common::DaemonConfig;

use crate::backends::btrfs_base::{BtrfsBaseBackend, BtrfsBaseScenario};
use crate::backends::btrfs_common;
use crate::backends::btrfs_loop::BtrfsLoopBackend;

/// Result of backend detection, including the backend instance and how it was chosen.
pub struct DetectResult {
    pub backend: Arc<dyn StorageBackend>,
    pub method: String, // "config" or "auto-detect"
}

/// Detect and create the appropriate storage backend based on configuration.
///
/// When `config.backend_type` is "auto", runs the three-level auto-detection:
///   1. Check if the default data path is already on btrfs → BtrfsBase
///   2. Check if any btrfs partition is mounted → BtrfsBase (CrossDisk)
///   3. Fallback → BtrfsLoop (creates a loop device)
///
/// When an explicit backend type is configured, creates that backend directly.
pub(crate) async fn detect_and_create_backend(
    config: &DaemonConfig,
) -> anyhow::Result<DetectResult> {
    match config.parse_backend_type() {
        Some(backend_type) => {
            info!(
                "Backend explicitly configured as '{}', skipping auto-detection",
                config.backend_type
            );
            let backend = create_backend(backend_type, config).await?;
            Ok(DetectResult {
                backend,
                method: "config".to_string(),
            })
        }
        None => {
            // "auto" or unrecognized → run auto-detection
            info!(
                "Backend type is '{}', running auto-detection...",
                config.backend_type
            );
            let backend_type = auto_detect(config).await?;
            info!("Auto-detection selected backend: {}", backend_type);
            let backend = create_backend(backend_type, config).await?;
            Ok(DetectResult {
                backend,
                method: "auto-detect".to_string(),
            })
        }
    }
}

/// Three-level auto-detection logic.
async fn auto_detect(config: &DaemonConfig) -> anyhow::Result<BackendType> {
    // Level 1: Prefer the filesystem that contains the configured mount path.
    // This prevents a host with multiple Btrfs filesystems from binding storage
    // to whichever entry happened to appear first in /proc/mounts.
    match btrfs_common::find_btrfs_partition_for_path(&config.mount_path).await {
        Ok(mount_info) => {
            info!(
                "Auto-detect: mount_path {} is on btrfs mount {} (device: {})",
                config.mount_path.display(),
                mount_info.mount_point,
                mount_info.device
            );
            return Ok(BackendType::BtrfsBase);
        }
        Err(error) => info!(
            "Auto-detect: mount_path {} is not on usable btrfs: {:#}",
            config.mount_path.display(),
            error
        ),
    }

    // Level 2: Is there an already-mounted btrfs partition?
    if let Ok(mount_info) = btrfs_common::find_available_btrfs_partition().await {
        info!(
            "Auto-detect: found mounted btrfs partition at {} (device: {})",
            mount_info.mount_point, mount_info.device
        );
        return Ok(BackendType::BtrfsBase);
    }

    // Level 3: No btrfs partition found → fallback to BtrfsLoop
    info!("Auto-detect: no mounted btrfs partition found, falling back to BtrfsLoop");
    Ok(BackendType::BtrfsLoop)
}

/// Create a backend instance for the given type.
pub(crate) async fn create_backend(
    backend_type: BackendType,
    config: &DaemonConfig,
) -> anyhow::Result<Arc<dyn StorageBackend>> {
    match backend_type {
        BackendType::BtrfsLoop => {
            // Decide effective image path before constructing the backend; this
            // also performs the one-shot legacy → target migration on upgrade.
            // On migration failure we transparently fall back to legacy so the
            // daemon keeps serving — see decide_effective_img_path for the tree.
            let target = PathBuf::from(ws_ckpt_common::BTRFS_IMG_PATH);
            let legacy = PathBuf::from(ws_ckpt_common::LEGACY_BTRFS_IMG_PATH);
            let effective = crate::backends::btrfs_loop::decide_effective_img_path(
                &config.mount_path,
                &target,
                &legacy,
            )
            .await
            .context("Failed to resolve effective btrfs image path")?;
            let backend = BtrfsLoopBackend::new(config.mount_path.clone(), effective);
            Ok(Arc::new(backend))
        }
        BackendType::BtrfsBase => {
            let explicitly_configured = config.parse_backend_type() == Some(BackendType::BtrfsBase);
            let mount_info = if explicitly_configured {
                btrfs_common::find_btrfs_partition_for_path(&config.mount_path)
                    .await
                    .with_context(|| {
                        format!(
                            "Backend type 'btrfs-base' requires configured mount_path {} to be \
                             located on a writable Btrfs filesystem",
                            config.mount_path.display()
                        )
                    })?
            } else {
                match btrfs_common::find_btrfs_partition_for_path(&config.mount_path).await {
                    Ok(mount_info) => mount_info,
                    Err(_) => btrfs_common::find_available_btrfs_partition()
                        .await
                        .context(
                            "Backend type 'btrfs-base' selected but no mounted btrfs partition \
                             found. Please mount a btrfs partition or switch to 'btrfs-loop' \
                             backend.",
                        )?,
                }
            };

            // Determine scenario: daemon-level default is CrossDisk.
            // The actual scenario (InPlace vs CrossDisk) is refined per-workspace
            // during init_workspace based on whether the workspace path is on the
            // same btrfs partition.
            let scenario = BtrfsBaseScenario::CrossDisk;
            let backend = if explicitly_configured {
                info!(
                    "Creating explicitly configured BtrfsBase backend: mount={}, data_root={}, \
                     scenario={:?}",
                    mount_info.mount_point,
                    config.mount_path.display(),
                    scenario
                );
                BtrfsBaseBackend::from_data_root(config.mount_path.clone(), scenario)
            } else {
                info!(
                    "Creating auto-selected BtrfsBase backend: mount={}, scenario={:?}",
                    mount_info.mount_point, scenario
                );
                BtrfsBaseBackend::new(PathBuf::from(&mount_info.mount_point), scenario)
            };
            Ok(Arc::new(backend))
        }
    }
}
