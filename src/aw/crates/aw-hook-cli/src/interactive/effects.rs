//! Read current-attachment check and candidate facts without asserting native adoption.

use super::{evidence, storage, Binding, Error, State};
use aw_contracts::events::Notification;
use aw_core::journal::FileJournal;
use serde_json::{json, Value};
use std::{fs, path::Path};

pub(super) fn snapshot(root: &Path, binding: &Binding, state: &State) -> Value {
    if state.effect_gap_attachment == Some(state.attachment) {
        return json!({"status":"unavailable"});
    }
    match read(root, binding, state) {
        Ok(value) => value,
        // A concurrently incomplete journal or corrupt prefix is unavailable,
        // never a zero count or a successful check.
        Err(_) => json!({"status":"unavailable"}),
    }
}

pub(super) fn guard_unavailable(root: &Path, event: &Notification) -> Result<(), Error> {
    let _lock = storage::lock(root)?;
    let mut state: State = storage::read(&root.join("state.json"))?;
    // An old admission failure must not taint the replacement attachment.
    if state.attached
        && event.session_epoch == Some(state.attachment)
        && event.session_id == state.session_id
    {
        state.gap = true;
        state.effect_gap_attachment = Some(state.attachment);
        storage::replace(&root.join("state.json"), &state)?;
    }
    Ok(())
}

fn read(root: &Path, binding: &Binding, state: &State) -> Result<Value, Error> {
    let mut checks = json!({"passed":0,"denied":0,"failed":0,"pending":0});
    let mut projections = json!({"candidates":0,"preserved":0,"failed":0,"pending":0});
    for (directory, plan, counts) in [
        ("tool-check-journal", "tool_check", &mut checks),
        ("tool-response-journal", "tool_response", &mut projections),
    ] {
        let path = root.join(directory);
        if !path.try_exists()? {
            continue;
        }
        let journal = FileJournal::open_read_only(&path).map_err(evidence)?;
        for (index, entry) in fs::read_dir(path)?.enumerate() {
            let entry = entry?;
            if index >= 1024 || entry.metadata()?.len() > 262_144 {
                return Err(Error::Evidence);
            }
            let name = entry.file_name();
            let key = name
                .to_str()
                .and_then(|name| name.strip_suffix(".jsonl"))
                .ok_or(Error::Evidence)?;
            let rows = journal.read(key).map_err(evidence)?;
            let event = &rows[0]["record"]["plan"][plan];
            if event["runtime_id"] != binding.runtime_id
                || event["session_epoch"] != state.attachment
                || event["session_id"].as_str() != state.session_id.as_deref()
            {
                continue;
            }
            let mut outcome = "pending";
            for row in &rows {
                let record = &row["record"];
                outcome = match record["kind"].as_str() {
                    Some("tool_check_completed") => match record["reason"].as_str() {
                        Some("check_passed") if record["allowed"] == true => "passed",
                        Some("policy_denied") => "denied",
                        _ => "failed",
                    },
                    Some("tool_response_completed") => match record["decision"].as_str() {
                        Some("replace") => "candidates",
                        Some("preserve") => "preserved",
                        _ => "failed",
                    },
                    Some("tool_response_withheld") => "failed",
                    _ => outcome,
                };
            }
            let count = counts[outcome].as_u64().ok_or(Error::Evidence)?;
            counts[outcome] = json!(count + 1);
        }
    }
    Ok(
        json!({"status":"available","checks":checks,"projections":projections,
        "native_execution":"unconfirmed","native_adoption":"unconfirmed"}),
    )
}
