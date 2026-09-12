//! Fixed Qoder history observations, bound to a captured append-only prefix.
//!
//! Live observation checks the whole snapshot for uniqueness. Replay checks its
//! retained rows; the observation journal authenticates that past scan, not the
//! current history file or an adversary with the same local identity.

use crate::{parse_payload, wall_time, Error, ProjectionSettings};
use aw_contracts::canonical;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, fs::OpenOptions, io::Read, path::PathBuf};

pub(crate) const PROFILE: &str = "qoder-cli-1.1.47/jsonl-v1";
const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;

/// Trusted capture context; native tool input stays encoded to preserve numbers.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    pub(crate) profile: String,
    pub(crate) scope: Value,
    pub(crate) max_observation_delay_ms: u64,
    path: PathBuf,
    cwd: PathBuf,
    device: u64,
    inode: u64,
    prefix_bytes: u64,
    prefix_digest: String,
    captured_at_ms: u64,
    tool_name: String,
    // Store native JSON as a string because native numbers need not satisfy
    // AW's integer-only metadata canonicalization rules.
    tool_input: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    line: u64,
    offset: u64,
    raw: String,
    digest: String,
}

/// Retained matching rows from one complete, bounded native history scan.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Proof {
    binding_digest: String,
    snapshot_digest: String,
    snapshot_bytes: u64,
    tool: Row,
    result: Row,
    pub(crate) observed_at_ms: u64,
}

impl Binding {
    /// Pins the file prefix before returning a candidate to the native hook.
    pub(crate) fn capture(settings: &ProjectionSettings, payload: &Value) -> Result<Self, Error> {
        if settings.history_profile != PROFILE
            || payload["cwd"].as_str() != settings.hook.provider.cwd.to_str()
            || payload
                .get("transcript_path")
                .is_some_and(|p| p.as_str() != settings.history_path.to_str())
            || payload["session_id"] != settings.hook.scope["session_id"]
            || payload["tool_use_id"] != settings.hook.scope["tool_use_id"]
        {
            return Err(Error::Input);
        }
        let Snapshot {
            bytes,
            device,
            inode,
        } = read(&settings.history_path)?.ok_or(Error::Input)?;
        let binding = Self {
            profile: PROFILE.into(),
            scope: settings.hook.scope.clone(),
            max_observation_delay_ms: settings.max_observation_delay_ms,
            path: settings.history_path.clone(),
            cwd: settings.hook.provider.cwd.clone(),
            device,
            inode,
            prefix_bytes: bytes.len() as u64,
            prefix_digest: canonical::digest(&bytes),
            captured_at_ms: wall_time()?,
            tool_name: text(payload, "tool_name")?.into(),
            tool_input: serde_json::to_string(&tool_input(&payload["tool_input"])?)
                .map_err(|_| Error::Input)?,
        };
        binding.check()?;
        let (_, result) = binding.scan(&bytes)?;
        if result.is_some() {
            return Err(Error::Input);
        }
        Ok(binding)
    }

    /// Scans appended history; missing files or results remain unverified.
    pub(crate) fn observe(&self) -> Result<Option<Proof>, Error> {
        self.check()?;
        let Some(Snapshot {
            bytes,
            device,
            inode,
        }) = read(&self.path)?
        else {
            return Ok(None);
        };
        let prefix = usize::try_from(self.prefix_bytes).map_err(|_| Error::Input)?;
        if device != self.device
            || inode != self.inode
            || bytes.len() < prefix
            || canonical::digest(&bytes[..prefix]) != self.prefix_digest
        {
            return Err(Error::Input);
        }
        let (tool, result) = self.scan(&bytes)?;
        let Some(result) = result else {
            return Ok(None);
        };
        let tool = tool.ok_or(Error::Input)?;
        let proof = Proof {
            binding_digest: self.digest()?,
            snapshot_digest: canonical::digest(&bytes),
            snapshot_bytes: bytes.len() as u64,
            tool,
            result,
            observed_at_ms: wall_time()?,
        };
        self.validate(&proof)?;
        Ok(Some(proof))
    }

    /// Replays a committed scan's rows without consulting the live filesystem.
    pub(crate) fn validate(&self, proof: &Proof) -> Result<String, Error> {
        self.check()?;
        if proof.binding_digest != self.digest()?
            || !is_digest(&proof.snapshot_digest)
            || proof.snapshot_bytes > MAX_HISTORY_BYTES as u64
            || proof.snapshot_bytes <= self.prefix_bytes
            || proof.observed_at_ms < self.captured_at_ms
            || proof.tool.line >= proof.result.line
            || proof.tool.end()? > proof.result.offset
            || proof.result.offset < self.prefix_bytes
            || proof.result.end()? > proof.snapshot_bytes
        {
            return Err(Error::Input);
        }
        let (tool, result) = self.rows([&proof.tool, &proof.result].into_iter())?;
        if tool.is_none() || result.is_none() {
            return Err(Error::Input);
        }
        let row = parse_payload(proof.result.raw.as_bytes())?;
        row["message"]["content"]
            .as_array()
            .and_then(|blocks| {
                blocks.iter().find(|b| {
                    b["type"] == "tool_result" && b["tool_use_id"] == self.scope["tool_use_id"]
                })
            })
            .and_then(|b| b["content"].as_str())
            .map(str::to_owned)
            .ok_or(Error::Input)
    }

    fn digest(&self) -> Result<String, Error> {
        canonical::document_digest(&serde_json::to_value(self).map_err(|_| Error::Input)?)
            .map_err(|_| Error::Input)
    }

    fn check(&self) -> Result<(), Error> {
        if self.profile != PROFILE
            || !self.path.is_absolute()
            || !self.cwd.is_absolute()
            || self.prefix_bytes > MAX_HISTORY_BYTES as u64
            || !is_digest(&self.prefix_digest)
            || !(1..=300_000).contains(&self.max_observation_delay_ms)
            || self.captured_at_ms == 0
            || self.tool_name.is_empty()
        {
            return Err(Error::Input);
        }
        text(&self.scope, "session_id")?;
        text(&self.scope, "tool_use_id")?;
        tool_input(&Value::String(self.tool_input.clone()))?;
        Ok(())
    }

    fn scan(&self, bytes: &[u8]) -> Result<(Option<Row>, Option<Row>), Error> {
        let raw = std::str::from_utf8(bytes).map_err(|_| Error::Input)?;
        if !raw.is_empty() && !raw.ends_with('\n') {
            return Err(Error::Input);
        }
        let mut offset = 0_u64;
        let rows = raw.split_inclusive('\n').enumerate().map(|(index, line)| {
            let raw = line.strip_suffix('\n').unwrap_or(line);
            let row = Row {
                line: index as u64 + 1,
                offset,
                raw: raw.into(),
                digest: canonical::digest(raw.as_bytes()),
            };
            offset += line.len() as u64;
            row
        });
        // Parse the whole bounded snapshot, including unrelated rows, so malformed
        // records or a second matching block cannot silently weaken uniqueness.
        let mut state = (None, None);
        for row in rows {
            self.row(&row, &mut state)?;
        }
        Ok(state)
    }

    fn rows<'a>(
        &self,
        rows: impl Iterator<Item = &'a Row>,
    ) -> Result<(Option<Row>, Option<Row>), Error> {
        let mut state = (None, None);
        for row in rows {
            self.row(row, &mut state)?;
        }
        Ok(state)
    }

    fn row(&self, item: &Row, state: &mut (Option<Row>, Option<Row>)) -> Result<(), Error> {
        if item.line == 0
            || item.raw.contains('\n')
            || canonical::digest(item.raw.as_bytes()) != item.digest
        {
            return Err(Error::Input);
        }
        let row = parse_payload(item.raw.as_bytes())?;
        if !row.is_object() {
            return Err(Error::Input);
        }
        let Some(blocks) = row["message"]["content"].as_array() else {
            return Ok(());
        };
        for block in blocks {
            let is_tool = block["type"] == "tool_use" && block["id"] == self.scope["tool_use_id"];
            let is_result =
                block["type"] == "tool_result" && block["tool_use_id"] == self.scope["tool_use_id"];
            if !is_tool && !is_result {
                continue;
            }
            if row["sessionId"] != self.scope["session_id"]
                || row["cwd"].as_str() != self.cwd.to_str()
                || row["isSidechain"] != false
            {
                return Err(Error::Input);
            }
            if is_tool {
                if row["type"] != "assistant"
                    || state.0.is_some()
                    || state.1.is_some()
                    || block["name"] != self.tool_name
                    || tool_input(&block["input"])?
                        != tool_input(&Value::String(self.tool_input.clone()))?
                {
                    return Err(Error::Input);
                }
                state.0 = Some(item.clone());
            } else {
                if row["type"] != "user"
                    || state.0.is_none()
                    || state.1.is_some()
                    // Successful Qoder 1.1.47 history omits this optional flag.
                    || block.get("is_error").is_some_and(|value| *value != false)
                    || !block["content"].is_string()
                {
                    return Err(Error::Input);
                }
                state.1 = Some(item.clone());
            }
        }
        Ok(())
    }
}

impl Proof {
    /// Binds AW's native evidence reference to the exact result row and snapshot.
    pub(crate) fn evidence(&self) -> Value {
        serde_json::json!({"source_id":"qoder-native-local-history",
            "record_id":format!("{}:{}", self.snapshot_digest, self.result.line),
            "digest":self.result.digest})
    }
}

impl Row {
    fn end(&self) -> Result<u64, Error> {
        self.offset
            .checked_add(self.raw.len() as u64)
            .and_then(|v| v.checked_add(1))
            .ok_or(Error::Input)
    }
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value[key]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or(Error::Input)
}

fn tool_input(value: &Value) -> Result<Value, Error> {
    let input = match value.as_str() {
        Some(raw) => parse_payload(raw.as_bytes())?,
        None => value.clone(),
    };
    if !input.is_object() {
        return Err(Error::Input);
    }
    Ok(input)
}

struct Snapshot {
    bytes: Vec<u8>,
    device: u64,
    inode: u64,
}

fn read(path: &PathBuf) -> Result<Option<Snapshot>, Error> {
    if !cfg!(target_os = "linux") || !path.is_absolute() {
        return Err(Error::Input);
    }
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Input),
    };
    let meta = file.metadata().map_err(|_| Error::Input)?;
    if !meta.is_file()
        || meta.uid() != fs::metadata("/proc/self").map_err(|_| Error::Input)?.uid()
        || meta.len() > MAX_HISTORY_BYTES as u64
    {
        return Err(Error::Input);
    }
    let mut bytes = Vec::new();
    file.take(MAX_HISTORY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Input)?;
    if bytes.len() > MAX_HISTORY_BYTES {
        return Err(Error::Input);
    }
    Ok(Some(Snapshot {
        bytes,
        device: meta.dev(),
        inode: meta.ino(),
    }))
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
