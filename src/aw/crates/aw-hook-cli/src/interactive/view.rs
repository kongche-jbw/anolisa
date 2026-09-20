//! Read-only attachment projection; no handler calls or adoption writes.

use super::{hooks::Occurrence, storage, Binding, Error, State};
use serde_json::{json, Value};
use std::{fs, path::Path};

/// Reads bounded current-attachment counters and live native process identity.
///
/// Process disappearance is reported as exited with unknown exit status. This
/// is not a claim that the shell has reaped every descendant or the task succeeded.
///
/// # Errors
/// Rejects missing/corrupt local evidence instead of presenting false zero counts.
pub fn query(root: &Path) -> Result<Value, Error> {
    let binding: Binding = storage::read(&root.join("binding.json"))?;
    let _lock = storage::lock(root)?;
    let state: State = storage::read(&root.join("state.json"))?;
    if binding.prepared.config.format == 2 {
        return super::notifications::query(root, &binding, &state);
    }
    let alive = crate::process_identity(binding.agent_pid)
        .is_ok_and(|(_, ticks)| ticks == binding.agent_ticks);
    let viewer_path = root.parent().ok_or(Error::Evidence)?.join("viewer.json");
    let viewer: Value = if viewer_path.try_exists()? {
        storage::read(&viewer_path)?
    } else {
        json!({"status":"not_configured"})
    };
    let mut called = 0u64;
    let mut observed = 0u64;
    let mut failed = 0u64;
    let mut pending = 0u64;
    for (index, entry) in fs::read_dir(root.join("calls"))?.enumerate() {
        if index >= 1024 {
            return Err(Error::Evidence);
        }
        let path = entry?.path();
        let occurrence: Occurrence = storage::read(&path.join("before.json"))?;
        if occurrence.attachment != state.attachment {
            continue;
        }
        if path.join("claimed.json").try_exists()? {
            called += 1;
            if path.join("completion.json").try_exists()? {
                let completion: Value = storage::read(&path.join("completion.json"))?;
                if completion["observed"] == true && completion["stale"] == false {
                    observed += 1;
                } else {
                    failed += 1;
                }
            } else {
                pending += 1;
            }
        }
    }
    Ok(json!({
        "format":1,"profile":"qoder-1.1.47/interactive-observe-v1",
        "runtime_id":binding.runtime_id,"runtime_generation":1,
        "source_epoch":binding.runtime_id,"agent_pid":binding.agent_pid,
        "agent_start_ticks":binding.agent_ticks,"runtime_alive":alive,
        "runtime_status":if alive { "observed_running" } else { "exited_or_unavailable" },
        "exit_status":null,"session_id":state.session_id,"attachment":state.attachment,
        "attached":state.attached && alive,"config_revision":binding.prepared.revision,
        "extension":binding.prepared.config.legacy_handler()?.provider_id,
        "bridge_status":if state.attachment == 0 { "awaiting_native_callback" } else { "callback_observed" },
        "calls":called,"observed":observed,"failed":failed,"pending":pending,
        "observation_gap":state.gap,"replacement":"unsupported","adoption":"unsupported",
        "os_coverage":"not_attached","required_safety":"unsupported","viewer":viewer
    }))
}
