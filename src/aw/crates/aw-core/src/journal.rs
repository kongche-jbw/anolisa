//! Private, digest-linked execution records with durable Linux reservations.
//!
//! Use a service-owned directory on a local filesystem that honors `fsync`.
//! Hashes detect corruption, not an attacker who can rewrite the journal and its
//! external evidence. A complete prefix is valid unless checked against a retained
//! tip with [`FileJournal::read_verified`]. Storage format 1 is not a wire schema.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use aw_contracts::canonical;
use serde_json::{json, Value};

use crate::ports::{Journal, JournalError};

const SOURCE_ID: &str = "aw-core-file-journal-v1";
// Execution journals contain bounded metadata, not provider streams. This cap
// leaves room for multi-provider plans while bounding aggregate read allocation.
const MAX_JOURNAL_BYTES: u64 = 128 * 1024 * 1024;

struct Claim {
    file: File,
    sequence: u64,
    digest: String,
    poisoned: bool,
}

/// Append-only journal whose reservations survive process exit and restart.
///
/// Only the object that successfully claimed an event may append to it. Opening
/// another journal never recovers write ownership or retries interrupted work.
/// [`Journal::release`] closes the event's writer and frees its bookkeeping;
/// its reservation and readable records remain on disk.
/// Callers supply metadata records; this storage layer does not obtain or redact
/// capability input/output. Directory access is a trusted local storage boundary.
pub struct FileJournal {
    directory: PathBuf,
    claims: HashMap<String, Claim>,
    read_only: bool,
}

impl FileJournal {
    /// Opens or creates a private journal directory, syncing newly created entries.
    ///
    /// # Errors
    /// Rejects non-Linux platforms because this backend's directory durability
    /// contract is Linux-specific. Reports directory creation or sync failures.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        if !cfg!(target_os = "linux") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "file journal durability is supported only on Linux",
            )
            .into());
        }
        let path = path.as_ref();
        let directory = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()?.join(path)
        };
        create_directory(&directory)?;
        // Another opener may observe a just-created ancestor before its creator
        // syncs the parent entry. Acknowledgement must not depend on that process.
        for ancestor in directory.ancestors() {
            File::open(ancestor)?.sync_all()?;
        }
        Ok(Self {
            directory,
            claims: HashMap::new(),
            read_only: false,
        })
    }

    /// Opens existing private storage without creating or syncing any entries.
    ///
    /// The final directory must belong to the effective user, deny group/other
    /// access and not be a symlink. Its parent path is trusted by the caller;
    /// this is not protection against another process with the same identity.
    /// The returned object rejects write reservations.
    ///
    /// # Errors
    /// Rejects non-Linux systems, absent or unsafe directories and I/O failures.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

            let path = path.as_ref();
            let directory = if path.is_absolute() {
                path.to_owned()
            } else {
                std::env::current_dir()?.join(path)
            };
            let directory: PathBuf = directory.components().collect();
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_DIRECTORY)
                .open(&directory)?;
            let metadata = file.metadata()?;
            // Match the supported Linux owner's identity without adding unsafe
            // process APIs to Core. The native hook uses the same procfs check.
            let owner = fs::metadata("/proc/self")?.uid();
            if !metadata.is_dir() || metadata.uid() != owner || metadata.mode() & 0o077 != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "journal directory must be private and owned by the effective user",
                )
                .into());
            }
            Ok(Self {
                directory,
                claims: HashMap::new(),
                read_only: true,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "file journal storage is supported only on Linux",
            )
            .into())
        }
    }

    /// Reads validated private envelopes, including the initial plan reservation.
    ///
    /// # Errors
    /// Rejects malformed keys, missing files, empty or partial records, invalid
    /// canonical metadata, sequence gaps, broken digests, non-regular or symlink
    /// files, and journals exceeding 128 MiB. A valid prefix cannot
    /// prove that no complete tail records were removed; use [`Self::read_verified`]
    /// when an independently retained final evidence object is available.
    pub fn read(&self, event_key: &str) -> Result<Vec<Value>, JournalError> {
        validate_key(event_key)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Nonblocking open rejects FIFOs below without waiting for a writer.
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(self.path(event_key))?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_JOURNAL_BYTES {
            return Err(JournalError::InvalidRecord);
        }
        let mut reader = BufReader::new(file);
        let mut records = Vec::new();
        let mut previous = Value::Null;
        let mut total_bytes = 0_u64;
        loop {
            // Bound each line before parsing instead of allocating an unbounded file.
            let mut line = Vec::new();
            let count = reader
                .by_ref()
                .take(canonical::MAX_DOCUMENT_BYTES as u64 + 2)
                .read_until(b'\n', &mut line)?;
            if count == 0 {
                break;
            }
            total_bytes += count as u64;
            // Recheck bytes consumed: a writer may grow the file after metadata.
            if total_bytes > MAX_JOURNAL_BYTES {
                return Err(JournalError::InvalidRecord);
            }
            if line.pop() != Some(b'\n') || line.len() > canonical::MAX_DOCUMENT_BYTES {
                return Err(JournalError::InvalidRecord);
            }
            let envelope = canonical::parse(&line).map_err(|_| JournalError::InvalidRecord)?;
            let sequence = u64::try_from(records.len()).map_err(|_| JournalError::InvalidRecord)?;
            validate_envelope(&envelope, event_key, sequence, &previous)?;
            previous = envelope["digest"].clone();
            records.push(envelope);
        }
        if records.is_empty() {
            return Err(JournalError::InvalidRecord);
        }
        Ok(records)
    }

    /// Checks the journal tip against evidence retained outside the journal file.
    ///
    /// # Errors
    /// Returns read errors or rejects any tip that differs from the expected
    /// evidence, including removal of an otherwise valid complete-record suffix.
    pub fn read_verified(
        &self,
        event_key: &str,
        expected: &Value,
    ) -> Result<Vec<Value>, JournalError> {
        let records = self.read(event_key)?;
        let last = records.last().ok_or(JournalError::InvalidRecord)?;
        let actual = evidence(
            event_key,
            last["sequence"]
                .as_u64()
                .ok_or(JournalError::InvalidRecord)?,
            last["digest"].as_str().ok_or(JournalError::InvalidRecord)?,
        );
        if actual != *expected {
            return Err(JournalError::InvalidRecord);
        }
        Ok(records)
    }

    fn path(&self, event_key: &str) -> PathBuf {
        self.directory.join(format!("{event_key}.jsonl"))
    }
}

impl Journal for FileJournal {
    fn claim(&mut self, event_key: &str, plan: &Value) -> Result<Value, JournalError> {
        if self.read_only {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "journal was opened read-only",
            )
            .into());
        }
        validate_key(event_key)?;
        if !plan.is_object() {
            return Err(JournalError::InvalidRecord);
        }
        let envelope = envelope(event_key, 0, Value::Null, json!({"plan": plan}))?;
        let bytes = line_bytes(&envelope)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(self.path(event_key)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(JournalError::AlreadyClaimed);
            }
            Err(error) => return Err(error.into()),
        };
        // Persist the reservation before writing it. Any later error leaves it
        // unavailable for retry, even when the first record is incomplete.
        File::open(&self.directory)?.sync_all()?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        let digest = envelope["digest"]
            .as_str()
            .ok_or(JournalError::InvalidRecord)?
            .to_owned();
        let result = evidence(event_key, 0, &digest);
        self.claims.insert(
            event_key.to_owned(),
            Claim {
                file,
                sequence: 0,
                digest,
                poisoned: false,
            },
        );
        Ok(result)
    }

    fn append(&mut self, event_key: &str, record: &Value) -> Result<Value, JournalError> {
        validate_key(event_key)?;
        let claim = self
            .claims
            .get_mut(event_key)
            .ok_or(JournalError::InvalidRecord)?;
        if claim.poisoned {
            return Err(JournalError::InvalidRecord);
        }
        // All errors after ownership lookup poison the claim, including malformed
        // caller metadata: subsequent effects must not outlive an unrecorded step.
        claim.poisoned = true;
        if !record.is_object() {
            return Err(JournalError::InvalidRecord);
        }
        let sequence = claim
            .sequence
            .checked_add(1)
            .filter(|value| *value <= canonical::MAX_SAFE_INTEGER)
            .ok_or(JournalError::InvalidRecord)?;
        let envelope = envelope(event_key, sequence, json!(claim.digest), record.clone())?;
        claim.file.write_all(&line_bytes(&envelope)?)?;
        claim.file.sync_all()?;
        claim.sequence = sequence;
        claim.digest = envelope["digest"]
            .as_str()
            .ok_or(JournalError::InvalidRecord)?
            .to_owned();
        claim.poisoned = false;
        Ok(evidence(event_key, sequence, &claim.digest))
    }

    fn release(&mut self, event_key: &str) {
        self.claims.remove(event_key);
    }
}

fn create_directory(path: &Path) -> Result<(), JournalError> {
    if !path.exists() {
        let parent = path.parent().ok_or(JournalError::InvalidRecord)?;
        create_directory(parent)?;
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        File::open(parent)?.sync_all()?;
    }
    if !path.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "journal storage path is not a directory",
        )
        .into());
    }
    File::open(path)?.sync_all()?;
    Ok(())
}

fn validate_key(event_key: &str) -> Result<(), JournalError> {
    if event_key.len() != 64
        || !event_key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(JournalError::InvalidRecord);
    }
    Ok(())
}

fn envelope(
    event_key: &str,
    sequence: u64,
    previous_digest: Value,
    record: Value,
) -> Result<Value, JournalError> {
    let mut value = json!({
        "format": 1,
        "event_key": event_key,
        "sequence": sequence,
        "previous_digest": previous_digest,
        "record": record,
    });
    let digest = canonical::document_digest(&value).map_err(|_| JournalError::InvalidRecord)?;
    value["digest"] = json!(digest);
    Ok(value)
}

fn line_bytes(value: &Value) -> Result<Vec<u8>, JournalError> {
    let mut bytes = canonical::bytes(value).map_err(|_| JournalError::InvalidRecord)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn validate_envelope(
    value: &Value,
    event_key: &str,
    sequence: u64,
    previous: &Value,
) -> Result<(), JournalError> {
    if !value.as_object().is_some_and(|object| object.len() == 6)
        || value["format"] != 1
        || value["event_key"] != event_key
        || value["sequence"] != sequence
        || value["previous_digest"] != *previous
        || !value["record"].is_object()
        || (sequence == 0
            && (!value["record"]
                .as_object()
                .is_some_and(|object| object.len() == 1)
                || !value["record"]["plan"].is_object()))
    {
        return Err(JournalError::InvalidRecord);
    }
    let expected = envelope(
        event_key,
        sequence,
        previous.clone(),
        value["record"].clone(),
    )?;
    if *value != expected {
        return Err(JournalError::InvalidRecord);
    }
    Ok(())
}

fn evidence(event_key: &str, sequence: u64, digest: &str) -> Value {
    json!({
        "source_id": SOURCE_ID,
        "record_id": format!("{event_key}:{sequence}"),
        "digest": digest,
    })
}
