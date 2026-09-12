//! Private retained execution material, anchored to the existing Core journal.

use crate::{history, input::read_private, Error, ProjectionSettings};
use aw_contracts::{canonical, orchestration::InvocationEvidence, Registry};
use aw_core::{journal::FileJournal, Execution};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

/// Full payloads retained privately so Journal digests can be revalidated.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Call {
    /// Admitted invocation, including the exact source artifact.
    pub(crate) invocation: Value,
    /// Host-authored terminal receipt.
    pub(crate) receipt: Value,
    /// Complete output envelope when produced.
    pub(crate) output: Option<Value>,
}

/// Original execution and independently captured native-history binding.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    /// Private retention format; distinct from AW wire schemas.
    pub(crate) format: u32,
    /// Core scope/event reservation key.
    pub(crate) event_key: String,
    /// Existing trusted Core storage directory.
    pub(crate) journal: PathBuf,
    /// Independent terminal execution tip.
    pub(crate) journal_ack: Value,
    /// Complete policy-selected plan.
    pub(crate) plan: Value,
    /// Pinned adapter descriptor governing adoption admission.
    pub(crate) boundary: Value,
    /// Validated terminal plan outcome.
    pub(crate) execution: Value,
    /// Dispatched invocation material, in execution order.
    pub(crate) calls: Vec<Call>,
    /// Native file prefix and tool identity captured before providers.
    pub(crate) history: history::Binding,
}

impl Record {
    /// Borrows retained calls for the shared full-plan validator.
    pub(crate) fn evidence(&self) -> Vec<InvocationEvidence<'_>> {
        self.calls
            .iter()
            .map(|c| InvocationEvidence {
                invocation: &c.invocation,
                receipt: &c.receipt,
                output: c.output.as_ref(),
            })
            .collect()
    }

    /// Selects the executed projection, if the plan reached that step.
    pub(crate) fn projection(&self) -> Option<&Call> {
        self.calls
            .iter()
            .find(|c| c.invocation["capability"] == "context.projection.prepare/v2")
    }
}

/// Validated destination and history binding prepared before native probes.
pub(crate) struct PendingRecord {
    directory: PathBuf,
    journal: PathBuf,
    history: history::Binding,
}

/// Requires explicit retention and captures independent history before execution.
pub(crate) fn prepare(
    settings: &ProjectionSettings,
    payload: &Value,
) -> Result<PendingRecord, Error> {
    if settings.retention != "source_and_candidate"
        || settings.history_profile != history::PROFILE
        || !(1..=300_000).contains(&settings.max_observation_delay_ms)
        || payload["cwd"].as_str() != settings.hook.provider.cwd.to_str()
        || payload
            .get("transcript_path")
            .is_some_and(|p| p.as_str() != settings.history_path.to_str())
    {
        return Err(Error::Input);
    }
    private_directory(&settings.record_directory)?;
    let history = history::Binding::capture(settings, payload)?;
    Ok(PendingRecord {
        directory: settings.record_directory.clone(),
        journal: settings.hook.journal.clone(),
        history,
    })
}

impl PendingRecord {
    /// Writes an immutable context only after verifying the terminal Core chain.
    pub(crate) fn persist(&self, execution: &Execution) -> Result<PathBuf, Error> {
        let event_key = event_key(execution.plan())?;
        let record = Record {
            format: 1,
            event_key: event_key.clone(),
            journal: self.journal.clone(),
            journal_ack: execution.journal_ack().clone(),
            plan: execution.plan().clone(),
            boundary: execution.boundary().clone(),
            execution: execution.record().clone(),
            calls: execution
                .calls()
                .iter()
                .map(|call| Call {
                    invocation: call.invocation().clone(),
                    receipt: call.result().receipt.clone(),
                    output: call.result().output.clone(),
                })
                .collect(),
            history: self.history.clone(),
        };
        verify(&record)?;
        let path = self.directory.join(format!("{event_key}.json"));
        write_new(
            &path,
            &serde_json::to_value(record).map_err(|_| Error::Execution)?,
        )?;
        Ok(path)
    }
}

/// Derives the same scope/event reservation key as Core.
pub(crate) fn event_key(plan: &Value) -> Result<String, Error> {
    canonical::document_digest(&json!({"scope":plan["scope"],"event_id":plan["event_id"]}))
        .map_err(|_| Error::Execution)
}

/// Binds retained full payloads to the pinned profile and independent Journal tip.
pub(crate) fn verify(record: &Record) -> Result<(), Error> {
    let registry = Registry::new().map_err(|_| Error::Execution)?;
    if record.format != 1
        || record.event_key != event_key(&record.plan)?
        || record.boundary
            != aw_adapters::profiles::boundary(aw_adapters::Host::Qoder, "PostToolUse")
                .map_err(|_| Error::Execution)?
        || record.history.profile != history::PROFILE
        || record.history.scope != record.plan["scope"]
        || !record.journal.is_absolute()
    {
        return Err(Error::Execution);
    }
    registry
        .validate_plan_execution(
            &record.plan,
            &record.execution,
            &record.evidence(),
            &record.boundary,
        )
        .map_err(|_| Error::Execution)?;
    let journal = FileJournal::open_read_only(&record.journal).map_err(|_| Error::Execution)?;
    let entries = journal
        .read_verified(&record.event_key, &record.journal_ack)
        .map_err(|_| Error::Execution)?;
    if entries.first().map(|v| &v["record"]["plan"]) != Some(&record.plan)
        || entries.last().map(|v| &v["record"]["execution"]) != Some(&record.execution)
    {
        return Err(Error::Execution);
    }
    // Core stores digests and receipts, so retained full calls must agree with
    // every dispatched invocation and settled result in that independent chain.
    let started: Vec<_> = entries
        .iter()
        .filter(|v| v["record"]["kind"] == "invocation_started")
        .collect();
    let settled: Vec<_> = entries
        .iter()
        .filter(|v| v["record"]["kind"] == "invocation_settled")
        .collect();
    if settled.len() != record.calls.len()
        || started.len() < settled.len()
        || started.len() > settled.len() + 1
    {
        return Err(Error::Execution);
    }
    if started.len() > settled.len() {
        // Core can cancel after the durable start and before Host dispatch.
        let tail = &started[settled.len()]["record"];
        let reference = &tail["plan_ref"];
        if record.execution["decision"] != "cancelled"
            || reference["plan_id"] != record.plan["plan_id"]
            || reference["revision"] != record.plan["revision"]
            || reference["digest"]
                != canonical::document_digest(&record.plan).map_err(|_| Error::Execution)?
            || !record.execution["steps"].as_array().is_some_and(|steps| {
                steps.iter().any(|step| {
                    step["step_id"] == reference["step_id"] && step["outcome"] == "cancelled"
                })
            })
        {
            return Err(Error::Execution);
        }
    }
    for ((call, start), end) in record.calls.iter().zip(started).zip(settled) {
        if start["record"]["invocation_digest"]
            != canonical::document_digest(&call.invocation).map_err(|_| Error::Execution)?
            || end["record"]["invocation_digest"] != start["record"]["invocation_digest"]
            || end["record"]["receipt"] != call.receipt
        {
            return Err(Error::Execution);
        }
    }
    Ok(())
}

/// Reads and validates a bounded private context without modifying storage.
pub(crate) fn load(path: &Path) -> Result<(Record, String), Error> {
    let value = read_json(path)?;
    let digest = canonical::document_digest(&value).map_err(|_| Error::Execution)?;
    let record: Record = serde_json::from_value(value).map_err(|_| Error::Execution)?;
    verify(&record)?;
    Ok((record, digest))
}

/// Reads bounded private metadata with duplicate-aware canonical parsing.
pub(crate) fn read_json(path: &Path) -> Result<Value, Error> {
    canonical::parse(&read_private(path, canonical::MAX_DOCUMENT_BYTES)?)
        .map_err(|_| Error::Execution)
}

/// Locates a context-related immutable record.
pub(crate) fn sibling(path: &Path, suffix: &str) -> PathBuf {
    path.with_extension(suffix)
}

/// Records completed local stdout delivery, never native acceptance or adoption.
///
/// # Errors
/// Rejects invalid retained execution, duplicate markers or unacknowledged storage.
pub fn mark_returned(path: &Path) -> Result<(), Error> {
    let (_, digest) = load(path)?;
    write_new(
        &sibling(path, "returned.json"),
        &json!({"format":1,"context_digest":digest,
        "written_at_ms":crate::wall_time()?}),
    )
}

/// Checks optional local transport evidence without assuming native acceptance.
pub(crate) fn returned(path: &Path, digest: &str) -> Result<bool, Error> {
    let marker = sibling(path, "returned.json");
    if !exists(&marker)? {
        return Ok(false);
    }
    let value = read_json(&marker)?;
    if value.as_object().map(|o| o.len()) != Some(3)
        || value["format"] != 1
        || value["context_digest"] != digest
        || !value["written_at_ms"]
            .as_u64()
            .is_some_and(|t| t <= crate::wall_time().unwrap_or(0))
    {
        return Err(Error::Execution);
    }
    Ok(true)
}

/// Creates one private durable record; interrupted writes are never retried.
pub(crate) fn write_new(path: &Path, value: &Value) -> Result<(), Error> {
    use std::os::unix::fs::OpenOptionsExt;
    private_directory(path.parent().ok_or(Error::Input)?)?;
    let bytes = canonical::bytes(value).map_err(|_| Error::Execution)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| Error::Execution)?;
    // A partial write remains a visible, non-retryable record, as in Core.
    File::open(path.parent().ok_or(Error::Input)?)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Execution)?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| Error::Execution)
}

fn private_directory(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    if !path.is_absolute() {
        return Err(Error::Input);
    }
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => File::open(path.parent().ok_or(Error::Input)?)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::Execution)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Error::Execution),
    }
    let m = fs::symlink_metadata(path).map_err(|_| Error::Input)?;
    let uid = fs::metadata("/proc/self").map_err(|_| Error::Input)?.uid();
    if !m.is_dir() || m.uid() != uid || m.mode() & 0o077 != 0 {
        return Err(Error::Input);
    }
    Ok(())
}

// A dangling symlink is corrupt evidence, not an absent observation.
/// Distinguishes absent evidence from a corrupt final symlink.
pub(crate) fn exists(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Error::Execution),
    }
}
