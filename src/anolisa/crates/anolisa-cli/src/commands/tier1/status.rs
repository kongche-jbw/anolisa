//! `anolisa status [CAPABILITY]` — read-only view of installed capabilities.
//!
//! Reads `installed.toml` via the shared [`crate::commands::common`] helper
//! and lists every `Capability`-kind object, or filters down to a single
//! name. A missing state file is the expected fresh-install case and yields
//! an empty result; an unknown capability name surfaces a synthetic
//! `not_installed` record rather than an error (launch spec §7.1).
//!
//! This handler does NOT consult the catalog or resolver — it reports only
//! what is already on disk. Every field in [`CapabilityRecord`] is projected
//! straight from [`InstalledObject`]; nothing is synthesized that the state
//! file does not already know about.

use clap::Parser;
use serde::Serialize;

use anolisa_core::{HealthEntry, InstalledObject, InstalledState, ObjectKind};

use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

const COMMAND: &str = "status";

#[derive(Parser)]
pub struct StatusArgs {
    /// Show detail for a specific capability (omit for aggregate view).
    pub capability: Option<String>,
}

/// JSON-shaped record for a single capability, used in both the wire
/// envelope and the human renderer. Fields are projected straight from
/// the matching [`InstalledObject`] on disk; optional/empty fields are
/// skipped when absent so synthetic `not_installed` records stay compact.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct CapabilityRecord {
    name: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    installed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_operation_id: Option<String>,
    /// Components reported by the install record (from `component_refs`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    components: Vec<String>,
    /// Feature flags the install record marks as enabled.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    enabled_features: Vec<String>,
    /// Last-known health probe entries persisted in state. Empty until a
    /// background probe wires up — but still surfaced verbatim today so
    /// users see whatever the install runner recorded.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    health: Vec<HealthEntry>,
}

pub fn handle(args: StatusArgs, ctx: &CliContext) -> Result<(), CliError> {
    let state = common::load_installed_state(ctx, COMMAND)?;
    let records = select_capabilities(&state, args.capability.as_deref());

    if ctx.json {
        let data = serde_json::json!({ "capabilities": records });
        return render_json(COMMAND, data);
    }

    if !ctx.quiet {
        render_human(&records, ctx.verbose);
    }
    Ok(())
}

/// Pure selector: project [`InstalledState`] down to capability records,
/// optionally filtered to a single name. Extracted so tests can exercise
/// the filtering/synthetic-not-installed logic without mocking
/// `CliContext` or touching the filesystem.
fn select_capabilities(state: &InstalledState, name: Option<&str>) -> Vec<CapabilityRecord> {
    let installed: Vec<&InstalledObject> = state
        .objects
        .iter()
        .filter(|o| o.kind == ObjectKind::Capability)
        .collect();

    match name {
        None => installed.iter().map(|o| record_from_object(o)).collect(),
        Some(target) => match installed.iter().find(|o| o.name == target) {
            Some(obj) => vec![record_from_object(obj)],
            None => vec![CapabilityRecord {
                name: target.to_string(),
                status: "not_installed".to_string(),
                version: None,
                installed_at: None,
                last_operation_id: None,
                components: Vec::new(),
                enabled_features: Vec::new(),
                health: Vec::new(),
            }],
        },
    }
}

fn record_from_object(obj: &InstalledObject) -> CapabilityRecord {
    CapabilityRecord {
        name: obj.name.clone(),
        status: common::object_status_str(obj.status).to_string(),
        version: Some(obj.version.clone()),
        installed_at: Some(obj.installed_at.clone()),
        last_operation_id: obj.last_operation_id.clone(),
        components: obj.component_refs.clone(),
        enabled_features: obj.enabled_features.clone(),
        health: obj.health.clone(),
    }
}

fn render_human(records: &[CapabilityRecord], verbose: bool) {
    for record in records {
        let version = record.version.as_deref().unwrap_or("-");
        let installed_at = record.installed_at.as_deref().unwrap_or("-");
        println!(
            "{name:<28}  {status:<14}  {version:<10}  {installed_at}",
            name = record.name,
            status = record.status,
            version = version,
            installed_at = installed_at,
        );
        if verbose {
            if let Some(op) = record.last_operation_id.as_deref() {
                println!("    last_operation_id: {}", op);
            }
            if !record.components.is_empty() {
                println!("    components: {}", record.components.join(", "));
            }
            if !record.enabled_features.is_empty() {
                println!(
                    "    enabled_features: {}",
                    record.enabled_features.join(", ")
                );
            }
            for entry in &record.health {
                println!(
                    "    health[{}]: {} @ {}",
                    entry.name, entry.status, entry.checked_at
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anolisa_core::{
        FileOwner, HealthEntry, InstalledObject, InstalledState, ObjectKind, ObjectStatus,
        OwnedFile, SubscriptionScope,
    };
    use std::path::PathBuf;

    fn capability_object(name: &str, version: &str, status: ObjectStatus) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Capability,
            name: name.to_string(),
            version: version.to_string(),
            status,
            manifest_digest: Some("sha256:abc".to_string()),
            distribution_source: Some("builtin".to_string()),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-20260601-001".to_string()),
            managed: true,
            adopted: false,
            subscription_scope: SubscriptionScope::None,
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: vec![OwnedFile {
                path: PathBuf::from("/tmp/anolisa/bin/foo"),
                owner: FileOwner::Anolisa,
                sha256: None,
            }],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        }
    }

    /// A missing `installed.toml` is the fresh-install case and must
    /// surface as an empty result, not an error. Verifies the helper
    /// stack (`InstalledState::load` -> `select_capabilities`) collapses
    /// "no file" to "no capabilities".
    #[test]
    fn missing_state_file_yields_empty_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");
        let state = InstalledState::load(&path).expect("missing file is not an error");
        let records = select_capabilities(&state, None);
        assert!(records.is_empty());
    }

    #[test]
    fn unfiltered_listing_returns_all_capabilities() {
        let mut state = InstalledState::default();
        state.upsert_object(capability_object(
            "agent-observability",
            "0.1.0",
            ObjectStatus::Installed,
        ));
        state.upsert_object(capability_object(
            "tokenless",
            "0.2.0",
            ObjectStatus::Partial,
        ));

        let records = select_capabilities(&state, None);
        assert_eq!(records.len(), 2);
        let names: Vec<&str> = records.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"agent-observability"));
        assert!(names.contains(&"tokenless"));
        // Partial maps to the wire-friendly `degraded` label.
        let tokenless = records
            .iter()
            .find(|r| r.name == "tokenless")
            .expect("present");
        assert_eq!(tokenless.status, "degraded");
    }

    #[test]
    fn filter_miss_yields_synthetic_not_installed_record() {
        let mut state = InstalledState::default();
        state.upsert_object(capability_object(
            "agent-observability",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_capabilities(&state, Some("ws-ckpt"));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "ws-ckpt");
        assert_eq!(records[0].status, "not_installed");
        assert!(records[0].version.is_none());
        assert!(records[0].installed_at.is_none());
        assert!(records[0].last_operation_id.is_none());
        assert!(records[0].components.is_empty());
    }

    #[test]
    fn filter_hit_returns_stored_record() {
        let mut state = InstalledState::default();
        let mut obj = capability_object("agent-observability", "0.3.1", ObjectStatus::Installed);
        obj.component_refs = vec!["agentsight".to_string(), "openclaw".to_string()];
        obj.enabled_features = vec!["bpf-events".to_string()];
        obj.health = vec![HealthEntry {
            name: "binary".to_string(),
            status: "ok".to_string(),
            checked_at: "2026-06-01T10:01:00Z".to_string(),
        }];
        state.upsert_object(obj);

        let records = select_capabilities(&state, Some("agent-observability"));
        assert_eq!(records.len(), 1);
        let only = &records[0];
        assert_eq!(only.name, "agent-observability");
        assert_eq!(only.status, "installed");
        assert_eq!(only.version.as_deref(), Some("0.3.1"));
        assert_eq!(only.installed_at.as_deref(), Some("2026-06-01T10:00:00Z"));
        assert_eq!(only.last_operation_id.as_deref(), Some("op-20260601-001"));
        // State-projected fields must reach the wire record verbatim.
        assert_eq!(only.components, vec!["agentsight", "openclaw"]);
        assert_eq!(only.enabled_features, vec!["bpf-events"]);
        assert_eq!(only.health.len(), 1);
        assert_eq!(only.health[0].name, "binary");
        assert_eq!(only.health[0].status, "ok");
    }
}
