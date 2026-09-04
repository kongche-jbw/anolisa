use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixListener;

use super::*;

fn owner_uid() -> u32 {
    nix::unistd::Uid::effective().as_raw()
}

fn private_directory(parent: &Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(&path).unwrap();
    path
}

#[test]
fn a_new_audit_log_has_a_header_and_can_be_reopened() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let directory = private_directory(root.path(), "audit");
    let path = directory.join("security.jsonl");

    let first = open_audit_file(&path, owner_uid()).unwrap();
    drop(first);

    assert_eq!(fs::read(&path).unwrap(), AUDIT_HEADER);
    let reopened = open_audit_file(&path, owner_uid()).unwrap();
    drop(reopened);
}

#[test]
fn a_concurrently_locked_audit_log_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let directory = private_directory(root.path(), "audit");
    let path = directory.join("security.jsonl");
    let first = open_audit_file(&path, owner_uid()).unwrap();

    assert!(matches!(
        open_audit_file(&path, owner_uid()),
        Err(CheckpointAdmissionError::Audit)
    ));

    drop(first);
}

#[test]
fn an_arbitrary_existing_private_file_is_not_an_audit_log() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let directory = private_directory(root.path(), "audit");
    let path = directory.join("private-data");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(b"not an audit log\n").unwrap();
    file.sync_all().unwrap();
    drop(file);

    assert!(matches!(
        open_audit_file(&path, owner_uid()),
        Err(CheckpointAdmissionError::Audit)
    ));
    assert_eq!(fs::read(&path).unwrap(), b"not an audit log\n");
}

#[test]
fn a_non_sticky_writable_ancestor_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let writable = root.path().join("writable");
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o777).create(&writable).unwrap();
    fs::set_permissions(&writable, fs::Permissions::from_mode(0o777)).unwrap();
    let directory = private_directory(&writable, "audit");
    let path = directory.join("security.jsonl");

    assert!(matches!(
        open_audit_file(&path, owner_uid()),
        Err(CheckpointAdmissionError::Audit)
    ));
    assert!(!path.exists());
}

#[test]
fn root_daemon_and_unprivileged_gateway_uids_remain_distinct() {
    let socket = TrustedSocketIdentity {
        device: 21,
        inode: 34,
        daemon_uid: 0,
    };
    let binding = CheckpointBinding {
        version: super::super::BINDING_VERSION,
        protocol_version: 2,
        socket_device: socket.device,
        socket_inode: socket.inode,
        daemon_uid: socket.daemon_uid,
        runtime_workspace_device: 55,
        runtime_workspace_inode: 89,
        ws_id: "ws-abc123".to_owned(),
        registered_path: "/workspace".to_owned(),
        generation: [7; 32],
        owner_uid: 1_000,
        permit_id: PermitId::new(),
        execution_id: ExecutionId::new(),
    };

    assert_eq!(binding.socket_device, socket.device);
    assert_eq!(binding.socket_inode, socket.inode);
    assert_eq!(binding.daemon_uid, 0);
    assert_eq!(binding.owner_uid, 1_000);
    assert_ne!(binding.daemon_uid, binding.owner_uid);
}

#[test]
fn permissive_socket_mode_does_not_replace_peer_authentication() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.path().join("ws-ckpt.sock");
    let _listener = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();

    let identity = verify_socket_trust(&path, owner_uid()).unwrap();

    assert_eq!(identity.daemon_uid, owner_uid());
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o666
    );
}
