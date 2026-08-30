//! Kernel-credential authorization for privileged workspace operations.

use std::path::Path;
use std::sync::Arc;

use ws_ckpt_common::{ErrorCode, Request, ResolveError, Response};

use crate::backends::btrfs_common::{RegisteredWorkspaceBinding, WorkspacePathBinding};
use crate::state::DaemonState;

pub(crate) struct AuthorizedRequest {
    pub(crate) request: Request,
    pub(crate) init_binding: Option<WorkspacePathBinding>,
    pub(crate) recover_binding: Option<RegisteredWorkspaceBinding>,
}

impl std::fmt::Debug for AuthorizedRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorizedRequest")
            .field("request", &self.request)
            .field("has_init_binding", &self.init_binding.is_some())
            .field("has_recover_binding", &self.recover_binding.is_some())
            .finish()
    }
}

/// Authorizes a side-effecting request against kernel-derived peer credentials.
pub(crate) async fn authorize_request(
    state: &Arc<DaemonState>,
    request: Request,
    peer_uid: Option<u32>,
) -> Result<AuthorizedRequest, Box<Response>> {
    match request {
        Request::Init { workspace } => {
            let uid = require_peer_uid(peer_uid)?;
            let init_binding = authorize_init_path(state, &workspace, uid).await?;
            Ok(AuthorizedRequest {
                request: Request::Init { workspace },
                init_binding,
                recover_binding: None,
            })
        }
        Request::Checkpoint {
            workspace,
            id,
            message,
            metadata,
            pin,
        } => {
            let uid = require_peer_uid(peer_uid)?;
            // Legacy checkpoint auto-initializes unregistered paths, so its
            // authorization must apply the same canonical path fence as Init.
            let init_binding = authorize_init_path(state, &workspace, uid).await?;
            Ok(AuthorizedRequest {
                request: Request::Checkpoint {
                    workspace,
                    id,
                    message,
                    metadata,
                    pin,
                },
                init_binding,
                recover_binding: None,
            })
        }
        Request::Rollback {
            workspace,
            to,
            num_ancestors,
        } => {
            let uid = require_peer_uid(peer_uid)?;
            authorize_workspace(state, &workspace, uid).await?;
            Ok(authorized(Request::Rollback {
                workspace,
                to,
                num_ancestors,
            }))
        }
        Request::Delete {
            workspace: Some(workspace),
            snapshot,
            force,
        } => {
            let uid = require_peer_uid(peer_uid)?;
            authorize_workspace(state, &workspace, uid).await?;
            Ok(authorized(Request::Delete {
                workspace: Some(workspace),
                snapshot,
                force,
            }))
        }
        Request::Delete {
            workspace: None,
            snapshot,
            force,
        } => {
            let uid = require_peer_uid(peer_uid)?;
            let (workspace, resolved_snapshot) = resolve_global_delete(state, &snapshot).await?;
            let ws_id = authorize_registered(state, workspace, uid).await?;
            // Pin both identities. Dispatch must never repeat the global prefix
            // lookup after authorization because its result can change.
            Ok(authorized(Request::Delete {
                workspace: Some(ws_id),
                snapshot: resolved_snapshot,
                force,
            }))
        }
        Request::Cleanup { workspace, keep } => {
            let uid = require_peer_uid(peer_uid)?;
            authorize_workspace(state, &workspace, uid).await?;
            Ok(authorized(Request::Cleanup { workspace, keep }))
        }
        Request::ReloadWorkspacePolicy { workspace } => {
            let uid = require_peer_uid(peer_uid)?;
            authorize_workspace(state, &workspace, uid).await?;
            Ok(authorized(Request::ReloadWorkspacePolicy { workspace }))
        }
        Request::ResetWorkspacePolicy { workspace } => {
            let uid = require_peer_uid(peer_uid)?;
            authorize_workspace(state, &workspace, uid).await?;
            Ok(authorized(Request::ResetWorkspacePolicy { workspace }))
        }
        Request::PatchWorkspacePolicy {
            workspace,
            auto_cleanup,
            auto_cleanup_keep,
        } => {
            let uid = require_peer_uid(peer_uid)?;
            authorize_workspace(state, &workspace, uid).await?;
            Ok(authorized(Request::PatchWorkspacePolicy {
                workspace,
                auto_cleanup,
                auto_cleanup_keep,
            }))
        }
        Request::Recover { workspace } => {
            let uid = require_peer_uid(peer_uid)?;
            let registered = state
                .resolve_workspace(&workspace)
                .await
                .ok_or_else(|| denied(format!("workspace is not registered: {}", workspace)))?;
            let (ws_id, recover_binding) =
                authorize_registered_binding(state, registered, uid).await?;
            Ok(AuthorizedRequest {
                request: Request::Recover { workspace: ws_id },
                init_binding: None,
                recover_binding: Some(recover_binding),
            })
        }
        Request::GuardedCheckpointV2 {
            ws_id,
            expected_generation,
            checkpoint_id,
            operation_digest,
            message,
            metadata,
            pin,
        } => {
            // Preserve the guarded protocol's specific missing-credential
            // rejection while applying the same ownership fence when present.
            if let Some(uid) = peer_uid {
                authorize_workspace(state, &ws_id, uid).await?;
            }
            Ok(authorized(Request::GuardedCheckpointV2 {
                ws_id,
                expected_generation,
                checkpoint_id,
                operation_digest,
                message,
                metadata,
                pin,
            }))
        }
        request => Ok(authorized(request)),
    }
}

fn authorized(request: Request) -> AuthorizedRequest {
    AuthorizedRequest {
        request,
        init_binding: None,
        recover_binding: None,
    }
}

fn require_peer_uid(peer_uid: Option<u32>) -> Result<u32, Box<Response>> {
    peer_uid.ok_or_else(|| denied("workspace mutation requires kernel peer credentials"))
}

async fn authorize_init_path(
    state: &Arc<DaemonState>,
    workspace: &str,
    peer_uid: u32,
) -> Result<Option<WorkspacePathBinding>, Box<Response>> {
    if let Some(registered) = state.resolve_workspace(workspace).await {
        authorize_registered(state, registered, peer_uid).await?;
        return Ok(None);
    }

    let root = state
        .workspace_root_binding()
        .map_err(|error| denied(format!("cannot use workspace root: {error:#}")))?;
    let binding = root
        .pin_workspace(Path::new(workspace), peer_uid)
        .map_err(|error| denied(format!("cannot pin workspace directory: {error:#}")))?;
    Ok(Some(binding))
}

async fn authorize_workspace(
    state: &Arc<DaemonState>,
    workspace: &str,
    peer_uid: u32,
) -> Result<(), Box<Response>> {
    let registered = state
        .resolve_workspace(workspace)
        .await
        .ok_or_else(|| denied(format!("workspace is not registered: {}", workspace)))?;
    authorize_registered(state, registered, peer_uid).await?;
    Ok(())
}

async fn authorize_registered(
    state: &Arc<DaemonState>,
    workspace: Arc<tokio::sync::RwLock<crate::state::WorkspaceState>>,
    peer_uid: u32,
) -> Result<String, Box<Response>> {
    authorize_registered_binding(state, workspace, peer_uid)
        .await
        .map(|(ws_id, _)| ws_id)
}

async fn authorize_registered_binding(
    state: &Arc<DaemonState>,
    workspace: Arc<tokio::sync::RwLock<crate::state::WorkspaceState>>,
    peer_uid: u32,
) -> Result<(String, RegisteredWorkspaceBinding), Box<Response>> {
    let (ws_id, registered_path) = {
        let workspace = workspace.read().await;
        (workspace.ws_id.clone(), workspace.path.clone())
    };
    let root = state
        .workspace_root_binding()
        .map_err(|error| denied(format!("cannot use workspace root: {error:#}")))?;
    let binding = root
        .pin_registered_workspace(
            &registered_path,
            &state.backend.data_root().join(&ws_id),
            peer_uid,
        )
        .map_err(|error| denied(format!("cannot pin registered workspace: {error:#}")))?;
    Ok((ws_id, binding))
}

async fn resolve_global_delete(
    state: &DaemonState,
    snapshot: &str,
) -> Result<
    (
        Arc<tokio::sync::RwLock<crate::state::WorkspaceState>>,
        String,
    ),
    Box<Response>,
> {
    let mut found = None;
    for workspace in state.all_workspaces() {
        let resolved = {
            let workspace_state = workspace.read().await;
            workspace_state
                .index
                .resolve_by_prefix(snapshot)
                .map(|(snapshot_id, _)| snapshot_id.clone())
        };
        match resolved {
            Ok(snapshot_id) if found.is_none() => {
                found = Some((workspace, snapshot_id));
            }
            Ok(_) | Err(ResolveError::Ambiguous(_)) => {
                return Err(global_delete_error(
                    snapshot,
                    "matches multiple snapshots; specify a workspace",
                ));
            }
            Err(ResolveError::NotFound) => {}
        }
    }
    found.ok_or_else(|| global_delete_error(snapshot, "was not found"))
}

fn global_delete_error(snapshot: &str, reason: &str) -> Box<Response> {
    Box::new(Response::Error {
        code: ErrorCode::SnapshotNotFound,
        message: format!("global snapshot '{}' {}", snapshot, reason),
    })
}

fn denied(message: impl Into<String>) -> Box<Response> {
    Box::new(Response::Error {
        code: ErrorCode::InvalidPath,
        message: format!("access denied: {}", message.into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::sync::CancellationToken;
    use ws_ckpt_common::backend::StorageBackend;
    use ws_ckpt_common::{decode_payload, encode_frame, DaemonConfig, SnapshotIndex, SnapshotMeta};

    fn state(root: &Path, backend_root: &Path) -> Arc<DaemonState> {
        std::fs::create_dir_all(root).unwrap();
        let config = DaemonConfig {
            workspace_root: Some(root.to_path_buf()),
            mount_path: backend_root.to_path_buf(),
            socket_path: backend_root.join("daemon.sock"),
            ..DaemonConfig::default()
        };
        let backend: Arc<dyn StorageBackend> =
            Arc::new(crate::backends::btrfs_loop::BtrfsLoopBackend::new(
                backend_root.to_path_buf(),
                backend_root.join("test.img"),
            ));
        Arc::new(DaemonState::new(
            config,
            backend,
            backend_root.join("state"),
        ))
    }

    fn state_without_configured_root(backend_root: &Path) -> Arc<DaemonState> {
        let config = DaemonConfig {
            workspace_root: None,
            mount_path: backend_root.to_path_buf(),
            socket_path: backend_root.join("daemon.sock"),
            ..DaemonConfig::default()
        };
        let backend: Arc<dyn StorageBackend> =
            Arc::new(crate::backends::btrfs_loop::BtrfsLoopBackend::new(
                backend_root.to_path_buf(),
                backend_root.join("test.img"),
            ));
        Arc::new(DaemonState::new(
            config,
            backend,
            backend_root.join("state"),
        ))
    }

    fn register_owned_workspace(
        state: &DaemonState,
        root: &Path,
        backend_root: &Path,
        ws_id: &str,
        directory_name: &str,
        snapshots: &[&str],
    ) {
        std::fs::create_dir_all(root).unwrap();
        let live = backend_root.join(ws_id);
        std::fs::create_dir_all(&live).unwrap();
        let workspace = root.join(directory_name);
        symlink(&live, &workspace).unwrap();
        let mut index = SnapshotIndex::new(workspace.clone());
        for snapshot in snapshots {
            index.snapshots.insert(
                (*snapshot).to_string(),
                SnapshotMeta {
                    message: None,
                    metadata: None,
                    pinned: false,
                    created_at: chrono::Utc::now(),
                    missing: false,
                    parent_id: None,
                    child_ids: Vec::new(),
                },
            );
        }
        state.register_workspace(ws_id.to_string(), workspace, index);
    }

    fn assert_denied(result: Result<AuthorizedRequest, Box<Response>>, needle: &str) {
        match result {
            Err(response) => match *response {
                Response::Error {
                    code: ErrorCode::InvalidPath,
                    message,
                } => assert!(message.contains(needle), "message: {message}"),
                other => panic!("expected access denial, got {other:?}"),
            },
            Ok(request) => panic!("expected access denial, got {request:?}"),
        }
    }

    fn assert_global_delete_failed(result: Result<AuthorizedRequest, Box<Response>>, needle: &str) {
        match result {
            Err(response) => match *response {
                Response::Error {
                    code: ErrorCode::SnapshotNotFound,
                    message,
                } => assert!(message.contains(needle), "message: {message}"),
                other => panic!("expected snapshot resolution failure, got {other:?}"),
            },
            Ok(request) => panic!("global delete must fail closed, got {request:?}"),
        }
    }

    #[tokio::test]
    async fn non_root_peer_can_init_only_its_owned_descendant() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let workspace = root.join("project");
        let backend_root = temp.path().join("backend");
        std::fs::create_dir_all(&workspace).unwrap();
        let state = state(&root, &backend_root);
        let uid = nix::unistd::geteuid().as_raw();

        let result = authorize_request(
            &state,
            Request::Init {
                workspace: workspace.to_string_lossy().into_owned(),
            },
            Some(uid),
        )
        .await;
        assert!(result.unwrap().init_binding.is_some());
    }

    #[tokio::test]
    async fn legacy_checkpoint_pins_unregistered_directory_for_auto_init() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let workspace = root.join("project");
        let backend_root = temp.path().join("backend");
        std::fs::create_dir_all(&workspace).unwrap();
        let state = state(&root, &backend_root);

        let authorized = authorize_request(
            &state,
            Request::Checkpoint {
                workspace: workspace.to_string_lossy().into_owned(),
                id: "legacy-snapshot".to_string(),
                message: None,
                metadata: None,
                pin: false,
            },
            Some(nix::unistd::geteuid().as_raw()),
        )
        .await
        .unwrap();

        assert!(authorized.init_binding.is_some());
    }

    #[tokio::test]
    async fn default_root_still_pins_init_and_legacy_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("project");
        let backend_root = temp.path().join("backend");
        std::fs::create_dir(&workspace).unwrap();
        let state = state_without_configured_root(&backend_root);
        let uid = nix::unistd::geteuid().as_raw();

        let init = authorize_request(
            &state,
            Request::Init {
                workspace: workspace.to_string_lossy().into_owned(),
            },
            Some(uid),
        )
        .await
        .unwrap();
        assert!(init.init_binding.is_some());

        let checkpoint = authorize_request(
            &state,
            Request::Checkpoint {
                workspace: workspace.to_string_lossy().into_owned(),
                id: "legacy".to_string(),
                message: None,
                metadata: None,
                pin: false,
            },
            Some(uid),
        )
        .await
        .unwrap();
        assert!(checkpoint.init_binding.is_some());
    }

    #[tokio::test]
    async fn init_rejects_symlinked_ancestor_beneath_pinned_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let outside = temp.path().join("outside");
        let backend_root = temp.path().join("backend");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(outside.join("project")).unwrap();
        symlink(&outside, root.join("ancestor")).unwrap();
        let state = state(&root, &backend_root);

        let result = authorize_request(
            &state,
            Request::Init {
                workspace: root.join("ancestor/project").to_string_lossy().into_owned(),
            },
            Some(nix::unistd::geteuid().as_raw()),
        )
        .await;

        assert_denied(result, "ancestor is not an anchored directory");
    }

    #[tokio::test]
    async fn init_claim_rejects_ancestor_symlink_flip_after_authorization() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let parent = root.join("ancestor");
        let moved_parent = root.join("authorized-parent");
        let workspace = parent.join("project");
        let outside = temp.path().join("outside");
        let victim = outside.join("project");
        let backend_root = temp.path().join("backend");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(workspace.join("authorized.txt"), b"authorized").unwrap();
        std::fs::write(victim.join("sentinel.txt"), b"untouched").unwrap();
        let state = state(&root, &backend_root);
        let authorized = authorize_request(
            &state,
            Request::Init {
                workspace: workspace.to_string_lossy().into_owned(),
            },
            Some(nix::unistd::geteuid().as_raw()),
        )
        .await
        .unwrap();

        std::fs::rename(&parent, &moved_parent).unwrap();
        symlink(&outside, &parent).unwrap();
        let error = match authorized.init_binding.unwrap().claim() {
            Ok(_) => panic!("ancestor replacement must fail before claim"),
            Err(error) => error,
        };

        assert!(format!("{error:#}").contains("workspace parent before claim"));
        assert_eq!(
            std::fs::read(victim.join("sentinel.txt")).unwrap(),
            b"untouched"
        );
        assert_eq!(
            std::fs::read(moved_parent.join("project/authorized.txt")).unwrap(),
            b"authorized"
        );
    }

    #[tokio::test]
    async fn init_rejects_workspace_outside_configured_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let outside = temp.path().join("outside");
        let backend_root = temp.path().join("backend");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let state = state(&root, &backend_root);

        let result = authorize_request(
            &state,
            Request::Init {
                workspace: outside.to_string_lossy().into_owned(),
            },
            Some(nix::unistd::geteuid().as_raw()),
        )
        .await;
        assert_denied(result, "strict descendant");
    }

    #[tokio::test]
    async fn init_rejects_symlink_escape_from_configured_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let outside = temp.path().join("outside");
        let link = root.join("escape");
        let backend_root = temp.path().join("backend");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, &link).unwrap();
        let state = state(&root, &backend_root);

        let result = authorize_request(
            &state,
            Request::Init {
                workspace: link.to_string_lossy().into_owned(),
            },
            Some(nix::unistd::geteuid().as_raw()),
        )
        .await;
        assert_denied(result, "failed to open anchored workspace");
    }

    #[tokio::test]
    async fn registered_workspace_rejects_symlinked_ancestor_escape() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let outside = temp.path().join("outside");
        let backend_root = temp.path().join("backend");
        let live = backend_root.join("ws-escaped");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&live).unwrap();
        symlink(&outside, root.join("linked-parent")).unwrap();
        let workspace = root.join("linked-parent/project");
        symlink(&live, &workspace).unwrap();
        let state = state(&root, &backend_root);
        state.register_workspace(
            "ws-escaped".to_string(),
            workspace.clone(),
            SnapshotIndex::new(workspace),
        );

        let result = authorize_request(
            &state,
            Request::Recover {
                workspace: "ws-escaped".to_string(),
            },
            Some(nix::unistd::geteuid().as_raw()),
        )
        .await;
        assert_denied(result, "ancestor is not an anchored directory");
    }

    #[tokio::test]
    async fn registered_workspace_rejects_non_owner_peer() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let backend_root = temp.path().join("backend");
        let live = backend_root.join("ws-owned");
        let workspace = root.join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&live).unwrap();
        symlink(&live, &workspace).unwrap();
        let state = state(&root, &backend_root);
        state.register_workspace(
            "ws-owned".to_string(),
            workspace.clone(),
            SnapshotIndex::new(workspace.clone()),
        );

        let wrong_uid = nix::unistd::geteuid().as_raw().wrapping_add(1);
        let result = authorize_request(
            &state,
            Request::Checkpoint {
                workspace: "ws-owned".to_string(),
                id: "snap".to_string(),
                message: None,
                metadata: None,
                pin: false,
            },
            Some(wrong_uid),
        )
        .await;
        assert_denied(result, "does not own workspace");
    }

    #[tokio::test]
    async fn global_delete_not_found_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let backend_root = temp.path().join("backend");
        std::fs::create_dir_all(&root).unwrap();
        let state = state(&root, &backend_root);

        let result = authorize_request(
            &state,
            Request::Delete {
                workspace: None,
                snapshot: "missing".to_string(),
                force: true,
            },
            Some(nix::unistd::geteuid().as_raw()),
        )
        .await;

        assert_global_delete_failed(result, "was not found");
    }

    #[tokio::test]
    async fn global_delete_ambiguous_prefix_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let backend_root = temp.path().join("backend");
        let state = state(&root, &backend_root);
        register_owned_workspace(
            &state,
            &root,
            &backend_root,
            "ws-one",
            "one",
            &["abcdef1111111111111111111111111111111111"],
        );
        register_owned_workspace(
            &state,
            &root,
            &backend_root,
            "ws-two",
            "two",
            &["abcdef2222222222222222222222222222222222"],
        );

        let result = authorize_request(
            &state,
            Request::Delete {
                workspace: None,
                snapshot: "abcdef".to_string(),
                force: true,
            },
            Some(nix::unistd::geteuid().as_raw()),
        )
        .await;

        assert_global_delete_failed(result, "matches multiple snapshots");
    }

    #[tokio::test]
    async fn global_delete_stays_pinned_after_concurrent_prefix_collision() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let backend_root = temp.path().join("backend");
        let state = state(&root, &backend_root);
        let first_snapshot = "abcdef1111111111111111111111111111111111";
        register_owned_workspace(
            &state,
            &root,
            &backend_root,
            "ws-one",
            "one",
            &[first_snapshot],
        );

        let authorized = authorize_request(
            &state,
            Request::Delete {
                workspace: None,
                snapshot: "abcdef".to_string(),
                force: true,
            },
            Some(nix::unistd::geteuid().as_raw()),
        )
        .await
        .unwrap();

        let racing_state = Arc::clone(&state);
        let racing_root = root.clone();
        let racing_backend_root = backend_root.clone();
        tokio::spawn(async move {
            register_owned_workspace(
                &racing_state,
                &racing_root,
                &racing_backend_root,
                "ws-two",
                "two",
                &["abcdef2222222222222222222222222222222222"],
            );
        })
        .await
        .unwrap();

        match authorized.request {
            Request::Delete {
                workspace,
                snapshot,
                force,
            } => {
                assert_eq!(workspace.as_deref(), Some("ws-one"));
                assert_eq!(snapshot, first_snapshot);
                assert!(force);
            }
            other => panic!("expected pinned delete, got {other:?}"),
        }

        let now_ambiguous = authorize_request(
            &state,
            Request::Delete {
                workspace: None,
                snapshot: "abcdef".to_string(),
                force: true,
            },
            Some(nix::unistd::geteuid().as_raw()),
        )
        .await;
        assert_global_delete_failed(now_ambiguous, "matches multiple snapshots");
    }

    #[tokio::test]
    async fn legacy_mutations_never_fallback_without_peer_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let backend_root = temp.path().join("backend");
        std::fs::create_dir_all(&root).unwrap();
        let state = state(&root, &backend_root);
        let requests = [
            Request::Init {
                workspace: root.join("project").to_string_lossy().into_owned(),
            },
            Request::Checkpoint {
                workspace: "ws-any".to_string(),
                id: "snap".to_string(),
                message: None,
                metadata: None,
                pin: false,
            },
            Request::Rollback {
                workspace: "ws-any".to_string(),
                to: None,
                num_ancestors: Some(1),
            },
            Request::Recover {
                workspace: "ws-any".to_string(),
            },
            Request::Delete {
                workspace: Some("ws-any".to_string()),
                snapshot: "snap".to_string(),
                force: true,
            },
            Request::Delete {
                workspace: None,
                snapshot: "snap".to_string(),
                force: true,
            },
            Request::Cleanup {
                workspace: "ws-any".to_string(),
                keep: Some(1),
            },
            Request::ReloadWorkspacePolicy {
                workspace: "ws-any".to_string(),
            },
            Request::ResetWorkspacePolicy {
                workspace: "ws-any".to_string(),
            },
            Request::PatchWorkspacePolicy {
                workspace: "ws-any".to_string(),
                auto_cleanup: ws_ckpt_common::PolicyFieldOp::Set(false),
                auto_cleanup_keep: ws_ckpt_common::PolicyFieldOp::Unchanged,
            },
        ];

        for request in requests {
            assert_denied(
                authorize_request(&state, request, None).await,
                "kernel peer credentials",
            );
        }
    }

    #[tokio::test]
    async fn listener_uses_peer_credentials_for_legacy_init_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Work");
        let outside = temp.path().join("outside");
        let backend_root = temp.path().join("backend");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&backend_root).unwrap();
        let state = state(&root, &backend_root);
        let cancel = CancellationToken::new();
        let listener_state = state.clone();
        let listener_cancel = cancel.clone();
        let listener = tokio::spawn(async move {
            crate::listener::run_listener(listener_state, listener_cancel).await
        });

        let mut client = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match tokio::net::UnixStream::connect(&state.socket_path).await {
                    Ok(client) => break client,
                    Err(_) if listener.is_finished() => {
                        panic!("listener exited before accepting")
                    }
                    Err(_) => tokio::task::yield_now().await,
                }
            }
        })
        .await
        .expect("listener socket did not become ready");
        let request = Request::Init {
            workspace: outside.to_string_lossy().into_owned(),
        };
        client
            .write_all(&encode_frame(&request).unwrap())
            .await
            .unwrap();
        let len = client.read_u32_le().await.unwrap();
        let mut payload = vec![0; len as usize];
        client.read_exact(&mut payload).await.unwrap();
        let response: Response = decode_payload(&payload).unwrap();
        match response {
            Response::Error { message, .. } => {
                assert!(message.contains("strict descendant"), "message: {message}");
                assert!(!message.contains("kernel peer credentials"));
            }
            other => panic!("expected boundary rejection, got {other:?}"),
        }

        cancel.cancel();
        listener.await.unwrap().unwrap();
    }
}
