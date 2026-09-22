//! Publish one workspace dashboard from fresh, pane-owned structured reports.

use super::{process_identity, rpc, write_json, PaneBinding, SessionDescriptor};
use crate::runtime::aw::pane::read;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    pane_id: String,
    workspace_id: String,
    owner_pid: u32,
    owner_ticks: u64,
    root: PathBuf,
}

fn registration(directory: &Path, pane: &str) -> PathBuf {
    directory
        .join("panes")
        .join(format!("{:x}.json", Sha256::digest(pane.as_bytes())))
}

pub(in crate::runtime::aw) fn register(
    directory: &Path,
    binding: &PaneBinding,
    root: &Path,
) -> Result<(), String> {
    write_json(
        &registration(directory, &binding.pane_id),
        &Scope {
            pane_id: binding.pane_id.clone(),
            workspace_id: binding.workspace_id.clone(),
            owner_pid: binding.shell_pid,
            owner_ticks: process_identity(binding.shell_pid)?,
            root: root.into(),
        },
    )
}

pub(super) fn publish(
    directory: &Path,
    descriptor: &SessionDescriptor,
    panes: &[&Value],
) -> Result<(), String> {
    let workspace = panes
        .first()
        .and_then(|pane| pane["workspace_id"].as_str())
        .ok_or("AW workspace missing")?;
    if panes.len() > 128 {
        return Err("AW workspace exceeds 128 panes".into());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let mut views = Vec::new();
    let mut missing = 0;
    let mut runtimes = std::collections::BTreeSet::new();
    for pane in panes {
        let pane_id = pane["pane_id"].as_str().ok_or("AW pane identity missing")?;
        match report(directory, descriptor.server_pid, workspace, pane_id, now) {
            Ok(Some(view)) => {
                let identity = (
                    view["agent_pid"].as_u64(),
                    view["agent_start_ticks"].as_u64(),
                );
                if runtimes.insert(identity) {
                    views.push(view);
                }
            }
            Ok(None) => {}
            Err(_) => missing += 1,
        }
    }
    let tokens = aw_hook_cli::interactive::workspace_tokens(&views, missing);
    rpc(
        &descriptor.socket,
        "workspace.report_metadata",
        json!({
            "workspace_id":workspace,"source":"anolisa.aw.workspace","seq":now,
            "ttl_ms":3000,"tokens":tokens
        }),
    )?;
    Ok(())
}

fn report(
    directory: &Path,
    server: u32,
    workspace: &str,
    pane: &str,
    now: u128,
) -> Result<Option<Value>, String> {
    let scope: Scope = read(&registration(directory, pane))?;
    if scope.pane_id != pane
        || scope.workspace_id != workspace
        || scope.root.file_name().and_then(|v| v.to_str()) != Some("scope")
        || scope.root.parent().and_then(Path::parent) != Some(directory)
    {
        return Err("AW workspace scope binding mismatch".into());
    }
    let (parent, ticks) =
        aw_hook_cli::process_identity(scope.owner_pid).map_err(|e| e.to_string())?;
    if parent != server || ticks != scope.owner_ticks {
        return Err("AW workspace owner changed".into());
    }
    let snapshot: Value = read(&scope.root.join("viewer-snapshot.json"))?;
    let stamp = snapshot["observed_at_ms"]
        .as_u64()
        .ok_or("AW snapshot time missing")?;
    if snapshot["format"] != 1
        || snapshot["pane_id"] != pane
        || snapshot["owner_pid"] != scope.owner_pid
        || snapshot["owner_ticks"] != scope.owner_ticks
        || now
            .checked_sub(u128::from(stamp))
            .is_none_or(|age| age > 3000)
    {
        return Err("AW workspace snapshot expired or changed owner".into());
    }
    let view = snapshot.get("view").ok_or("AW snapshot view missing")?;
    if view.is_null() {
        return Ok(None);
    }
    if view["format"] != 2 {
        return Err("AW workspace requires structured event counts".into());
    }
    let pid = view["agent_pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or("AW Agent identity missing")?;
    let ticks = view["agent_start_ticks"]
        .as_u64()
        .ok_or("AW Agent generation missing")?;
    if process_identity(pid) != Ok(ticks) {
        return Ok(None);
    }
    Ok(Some(view.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_report_rejects_expiry_owner_changes_and_cross_pane_binding() {
        let dir = tempfile::Builder::new()
            .prefix("aw-ws-")
            .tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
            .unwrap();
        std::fs::create_dir(dir.path().join("panes")).unwrap();
        let root = dir.path().join("cosh-aw-test/scope");
        std::fs::create_dir_all(&root).unwrap();
        let pid = std::process::id();
        let (parent, ticks) = aw_hook_cli::process_identity(pid).unwrap();
        let binding = PaneBinding {
            pane_id: "pane-1".into(),
            workspace_id: "space-1".into(),
            shell_pid: pid,
        };
        register(dir.path(), &binding, &root).unwrap();
        let mut snapshot = json!({"format":1,"pane_id":"pane-1","owner_pid":pid,"owner_ticks":ticks,"observed_at_ms":10000,"view":null});
        write_json(&root.join("viewer-snapshot.json"), &snapshot).unwrap();
        assert!(report(dir.path(), parent, "space-1", "pane-1", 10000)
            .unwrap()
            .is_none());
        assert!(report(dir.path(), parent, "space-1", "pane-1", 13001).is_err());
        assert!(report(dir.path(), parent, "space-1", "pane-1", 9999).is_err());
        assert!(report(dir.path(), parent, "space-2", "pane-1", 10000).is_err());
        snapshot["owner_ticks"] = json!(ticks + 1);
        write_json(&root.join("viewer-snapshot.json"), &snapshot).unwrap();
        assert!(report(dir.path(), parent, "space-1", "pane-1", 10000).is_err());
        snapshot["owner_ticks"] = json!(ticks);
        snapshot["view"] = json!({"format":2,"agent_pid":pid,"agent_start_ticks":ticks});
        write_json(&root.join("viewer-snapshot.json"), &snapshot).unwrap();
        assert!(report(dir.path(), parent, "space-1", "pane-1", 10000)
            .unwrap()
            .is_some());
        snapshot["view"]["agent_start_ticks"] = json!(ticks + 1);
        write_json(&root.join("viewer-snapshot.json"), &snapshot).unwrap();
        assert!(report(dir.path(), parent, "space-1", "pane-1", 10000)
            .unwrap()
            .is_none());
    }
}
