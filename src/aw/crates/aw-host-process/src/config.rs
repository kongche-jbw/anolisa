//! Operator-declared launch provenance; selected file pins are not a sandbox.

use crate::Error;
use aw_contracts::canonical;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};

/// Maximum configured byte ceiling for each native protocol stream.
pub const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;
const MAX_PIN_BYTES: u64 = 64 * 1024 * 1024;

/// Independent hard limits; invocation budgets can only tighten them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Maximum child lifetime, including pipe exchange, at most five minutes.
    pub timeout_ms: u64,
    /// Maximum exact native stdin size, at most four MiB.
    pub input_bytes: usize,
    /// Maximum native stdout size, at most four MiB.
    pub output_bytes: usize,
    /// Maximum discarded stderr size, at most four MiB.
    pub stderr_bytes: usize,
}

/// Expected state of a selected native dependency or user configuration file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PinState {
    /// Exact lowercase SHA-256 of a regular file, serialized as `{"sha256":"..."}`.
    Sha256(String),
    /// Path must have no directory entry, serialized as `"absent"`.
    Absent,
}

/// A caller-selected file pin, including a deliberate absent-file assertion.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePin {
    /// Absolute UTF-8 path to a selected module, policy or user rule file.
    pub path: PathBuf,
    /// State expected before and after every native call.
    pub state: PinState,
}

/// Trusted launch configuration; no inherited environment is forwarded.
///
/// Pins cover only files explicitly selected by the operator. They do not attest
/// an interpreter's complete import graph or prevent changes during execution.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Host registry identity.
    pub provider_id: String,
    /// Operator-pinned Provider release, independent of the native CLI version.
    pub provider_version: String,
    /// Absolute executable path.
    pub program: PathBuf,
    /// Expected executable SHA-256 supplied independently by the operator.
    pub program_sha256: String,
    /// Absolute working directory, preserved for native configuration resolution.
    pub cwd: PathBuf,
    /// Prefix arguments preceding the native CLI operation.
    pub args: Vec<String>,
    /// Complete child environment supplied by the trusted embedding application.
    pub environment: BTreeMap<String, String>,
    /// Explicit selected-file assertions; at most 32 files, each at most 64 MiB.
    pub pins: Vec<FilePin>,
    /// Hard resource bounds for the native process.
    pub limits: Limits,
}

impl Config {
    /// Checks launch shape before reading pins or starting a native version probe.
    ///
    /// # Errors
    /// Rejects unsupported platforms, malformed paths, limits, arguments or pins.
    pub fn validate(&self) -> Result<(), Error> {
        if !cfg!(target_os = "linux")
            || !absolute_utf8(&self.program)
            || !absolute_utf8(&self.cwd)
            || !self.cwd.is_dir()
            || !digest_shape(&self.program_sha256)
        {
            return Err(Error::Configuration(
                "invalid Linux program, cwd or expected digest",
            ));
        }
        let limits = &self.limits;
        if !(1..=300_000).contains(&limits.timeout_ms)
            || [limits.input_bytes, limits.output_bytes, limits.stderr_bytes]
                .iter()
                .any(|n| !(1..=MAX_STREAM_BYTES).contains(n))
        {
            return Err(Error::Configuration(
                "limits exceed the bounded native profile",
            ));
        }
        if self.args.iter().any(|arg| arg.contains('\0'))
            || self.environment.iter().any(|(key, value)| {
                key.is_empty() || key.contains(['\0', '=']) || value.contains('\0')
            })
        {
            return Err(Error::Configuration(
                "invalid argument or environment encoding",
            ));
        }
        let mut paths = BTreeSet::from([&self.program]);
        if self.pins.len() > 32 {
            return Err(Error::Configuration("too many selected file pins"));
        }
        for pin in &self.pins {
            if !absolute_utf8(&pin.path)
                || !paths.insert(&pin.path)
                || matches!(&pin.state, PinState::Sha256(digest) if !digest_shape(digest))
            {
                return Err(Error::Configuration(
                    "invalid or duplicate selected file pin",
                ));
            }
        }
        Ok(())
    }

    /// Rechecks selected bytes and explicit absences without exposing their content.
    ///
    /// # Errors
    /// Rejects changed, unavailable or oversized selected files and broken absences.
    pub fn check_pins(&self) -> Result<(), Error> {
        if digest_file(&self.program)? != self.program_sha256 {
            return Err(Error::Configuration("program_changed"));
        }
        for pin in &self.pins {
            match &pin.state {
                PinState::Sha256(expected) if digest_file(&pin.path)? != *expected => {
                    return Err(Error::Configuration("pinned_file_changed"))
                }
                PinState::Sha256(_) => {}
                PinState::Absent => match fs::symlink_metadata(&pin.path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return Err(Error::Configuration("pinned_absence_changed")),
                },
            }
        }
        Ok(())
    }

    /// Binds the launch configuration to a Host-selected native protocol profile.
    ///
    /// The Host supplies fixed, reviewed profile values, never native tool arguments.
    ///
    /// # Errors
    /// Returns a configuration error if the declaration cannot be serialized.
    pub fn manifest(
        &self,
        native_cli_version: &str,
        protocol_profile: &str,
        operation: &str,
    ) -> Result<Value, Error> {
        let config = serde_json::to_value(self)
            .map_err(|_| Error::Configuration("configuration cannot be serialized"))?;
        Ok(
            json!({"format":1,"config":config,"native_cli_version":native_cli_version,
            "protocol_profile":protocol_profile,"operation":operation}),
        )
    }
}

fn absolute_utf8(path: &Path) -> bool {
    path.is_absolute() && path.to_str().is_some_and(|s| !s.contains('\0'))
}

fn digest_shape(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn digest_file(path: &Path) -> Result<String, Error> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A path replaced by a FIFO must not block the pin check before metadata.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| Error::Configuration("pinned_file_unavailable"))?;
    let metadata = file
        .metadata()
        .map_err(|_| Error::Configuration("pinned_file_unavailable"))?;
    if !metadata.is_file() || metadata.len() > MAX_PIN_BYTES {
        return Err(Error::Configuration("invalid_pinned_file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_PIN_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Configuration("pinned_file_unavailable"))?;
    if bytes.len() as u64 > MAX_PIN_BYTES {
        return Err(Error::Configuration("invalid_pinned_file"));
    }
    Ok(canonical::digest(&bytes))
}
