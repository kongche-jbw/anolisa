#![cfg(target_os = "linux")]

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use aw_core::journal::FileJournal;
use aw_core::ports::Journal;
use serde_json::json;

const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/journal-read-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }

    fn file(&self) -> PathBuf {
        self.0.join(format!("{KEY}.jsonl"))
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn read_only_open_never_creates_storage_or_grants_write_ownership() {
    let directory = Directory::new();
    let missing = directory.0.join("absent/child");
    assert!(FileJournal::open_read_only(&missing).is_err());
    assert!(!directory.0.join("absent").exists());

    let mut writer = FileJournal::new(&directory.0).unwrap();
    let tip = writer.claim(KEY, &json!({"plan_id": "example"})).unwrap();
    writer.release(KEY);
    let original = fs::read(directory.file()).unwrap();
    let mut reader = FileJournal::open_read_only(&directory.0).unwrap();
    assert_eq!(reader.read_verified(KEY, &tip).unwrap().len(), 1);
    assert!(reader.claim(&"b".repeat(64), &json!({})).is_err());
    assert!(reader.append(KEY, &json!({})).is_err());
    reader.release(KEY);
    assert_eq!(fs::read(directory.file()).unwrap(), original);
    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
}

#[test]
fn read_only_open_rejects_nonprivate_regular_and_symlink_directories() {
    let directory = Directory::new();
    fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(FileJournal::open_read_only(&directory.0).is_err());
    fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700)).unwrap();
    let regular = directory.0.join("regular");
    fs::write(&regular, b"unchanged").unwrap();
    assert!(FileJournal::open_read_only(&regular).is_err());
    let link = directory.0.join("alias");
    symlink(&directory.0, &link).unwrap();
    assert!(FileJournal::open_read_only(&link).is_err());
    assert!(FileJournal::open_read_only(format!("{}/", link.display())).is_err());
}

#[test]
fn every_reader_rejects_symlink_records_without_following_them() {
    let source = Directory::new();
    let mut writer = FileJournal::new(&source.0).unwrap();
    writer.claim(KEY, &json!({})).unwrap();
    writer.release(KEY);
    let directory = Directory::new();
    symlink(source.file(), directory.file()).unwrap();
    let writers = [
        FileJournal::new(&directory.0).unwrap(),
        FileJournal::open_read_only(&directory.0).unwrap(),
    ];
    for journal in writers {
        assert!(journal.read(KEY).is_err());
    }
    assert_eq!(writer.read(KEY).unwrap().len(), 1);
}

#[test]
fn every_reader_rejects_fifo_without_waiting_for_a_writer() {
    let directory = Directory::new();
    let path = CString::new(directory.file().as_os_str().as_bytes()).unwrap();
    // SAFETY: the CString is alive and NUL terminated; the owned test path is new.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    for journal in [
        FileJournal::new(&directory.0).unwrap(),
        FileJournal::open_read_only(&directory.0).unwrap(),
    ] {
        assert!(journal.read(KEY).is_err());
    }
}

#[test]
fn every_reader_rejects_oversize_sparse_records_before_parsing() {
    let directory = Directory::new();
    let file = fs::File::create(directory.file()).unwrap();
    file.set_len(128 * 1024 * 1024 + 1).unwrap();
    for journal in [
        FileJournal::new(&directory.0).unwrap(),
        FileJournal::open_read_only(&directory.0).unwrap(),
    ] {
        assert!(journal.read(KEY).is_err());
    }
}

#[test]
fn read_only_validation_distinguishes_valid_prefix_from_retained_tip() {
    let directory = Directory::new();
    let mut writer = FileJournal::new(&directory.0).unwrap();
    let first = writer.claim(KEY, &json!({})).unwrap();
    let tip = writer.append(KEY, &json!({"status": "completed"})).unwrap();
    writer.release(KEY);
    let reader = FileJournal::open_read_only(&directory.0).unwrap();
    assert_eq!(reader.read_verified(KEY, &tip).unwrap().len(), 2);
    let bytes = fs::read(directory.file()).unwrap();
    let end = bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1;
    fs::write(directory.file(), &bytes[..end]).unwrap();
    assert_eq!(reader.read_verified(KEY, &first).unwrap().len(), 1);
    assert!(reader.read_verified(KEY, &tip).is_err());
    fs::write(directory.file(), &bytes[..end - 1]).unwrap();
    assert!(reader.read(KEY).is_err());
}
