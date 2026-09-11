//! Pinned native hook capabilities, without assuming final dispatch or adoption.

use std::collections::HashSet;

use aw_contracts::Registry;
use serde_json::Value;

use crate::{Error, Host};

/// Returns a validated built-in profile for the supported adapter revision.
///
/// Profiles describe this adapter's supported surface, not every feature of
/// the host. Native hook registration and runtime verification remain external.
///
/// # Errors
/// Rejects invalid profile resources and unsupported boundary declarations.
pub fn profile(host: Host) -> Result<Value, Error> {
    load(host, &Registry::new()?)
}

/// Validates a bundled profile using the adapter's existing schema registry.
pub(crate) fn load(host: Host, registry: &Registry) -> Result<Value, Error> {
    let value = serde_json::from_str(source(host))
        .map_err(|_| Error::InvalidProfile("built-in profile is not valid JSON"))?;
    validate_profile(host, &value, registry)?;
    Ok(value)
}

/// Returns the descriptor matching an exact native event name.
///
/// # Errors
/// Rejects unknown events and invalid built-in profiles. Event names are case
/// sensitive; aliases must never silently select a different hook boundary.
pub fn boundary(host: Host, native_event: &str) -> Result<Value, Error> {
    let value = profile(host)?;
    descriptor(&value, native_event)
}

/// Selects an exact event descriptor from an already validated profile.
pub(crate) fn descriptor(value: &Value, native_event: &str) -> Result<Value, Error> {
    value["boundaries"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["native_event"] == native_event)
        })
        .map(|entry| entry["descriptor"].clone())
        .ok_or(Error::UnsupportedEvent)
}

fn source(host: Host) -> &'static str {
    match host {
        Host::Qoder => include_str!("../profiles/qoder.json"),
        Host::Codex => include_str!("../profiles/codex.json"),
        Host::QwenCode => include_str!("../profiles/qwencode.json"),
        Host::Hermes => include_str!("../profiles/hermes.json"),
        Host::OpenClaw => include_str!("../profiles/openclaw.json"),
        Host::Cosh => include_str!("../profiles/cosh.json"),
    }
}

fn require(condition: bool, reason: &'static str) -> Result<(), Error> {
    if condition {
        Ok(())
    } else {
        Err(Error::InvalidProfile(reason))
    }
}

fn exact_keys(value: &Value, keys: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == keys.len() && keys.iter().all(|k| object.contains_key(*k))
    })
}

/// Maps native event names without accepting aliases from other hosts.
pub(crate) fn phase(host: Host, event: &str) -> Result<&'static str, Error> {
    match (host, event) {
        (Host::Hermes, "pre_tool_call") | (Host::OpenClaw, "before_tool_call") => Ok("pre_tool"),
        (Host::Hermes, "post_tool_call") | (Host::OpenClaw, "after_tool_call") => Ok("post_tool"),
        (Host::Qoder | Host::Codex | Host::QwenCode | Host::Cosh, "PreToolUse") => Ok("pre_tool"),
        (Host::Qoder | Host::Codex | Host::QwenCode | Host::Cosh, "PostToolUse") => Ok("post_tool"),
        _ => Err(Error::UnsupportedEvent),
    }
}

fn validate_profile(host: Host, value: &Value, registry: &Registry) -> Result<(), Error> {
    require(
        exact_keys(value, &["format", "host", "boundaries"]),
        "profile fields do not match format 1",
    )?;
    require(value["format"] == 1, "unsupported profile format")?;
    require(value["host"] == host.as_str(), "profile host mismatch")?;
    let entries = value["boundaries"]
        .as_array()
        .ok_or(Error::InvalidProfile("boundaries must be an array"))?;
    require(
        entries.len() == 2,
        "profile must describe both tool boundaries",
    )?;
    let mut events = HashSet::new();
    let mut ids = HashSet::new();
    for entry in entries {
        require(
            exact_keys(entry, &["native_event", "descriptor"]),
            "boundary entry has unsupported fields",
        )?;
        let event = entry["native_event"]
            .as_str()
            .ok_or(Error::InvalidProfile("native event must be a string"))?;
        let phase =
            phase(host, event).map_err(|_| Error::InvalidProfile("unsupported native event"))?;
        require(events.insert(event), "duplicate native event")?;
        let descriptor = &entry["descriptor"];
        registry.validate_boundary(descriptor)?;
        require(
            ids.insert(descriptor["boundary_id"].as_str()),
            "duplicate boundary identifier",
        )?;
        require(
            descriptor["adapter_id"] == format!("aw.native.{}", host.as_str())
                && descriptor["adapter_version"] == "0.1.0"
                && descriptor["revision"] == 1
                && descriptor["boundary_id"] == format!("{}.{}", host.as_str(), phase)
                && descriptor["boundary"] == phase,
            "boundary identity differs from the pinned native mapping",
        )?;
        // Native denial is not proof of a guard over the final dispatched input.
        require(
            descriptor["can_deny_dispatch"] == false
                && descriptor["has_final_input_guard"] == false
                && descriptor["proof_boundaries"] == serde_json::json!([])
                && descriptor["ledger_policy"] == "best_effort"
                && descriptor["composition"]["input_finality"] == "uncontrolled"
                && descriptor["composition"]["gate"] == "none"
                && descriptor["composition"]["result_finality"] == "subject_to_later_change",
            "native profiles cannot claim final enforcement or adoption",
        )?;
        let replaces = matches!(host, Host::Qoder) && phase == "post_tool";
        let mode = if phase == "post_tool" && !replaces {
            "observe_only"
        } else {
            "awaitable"
        };
        require(
            descriptor["can_replace_text"] == replaces
                && descriptor["invocation_mode"] == mode
                && descriptor["media_types"] == serde_json::json!(["text/plain"])
                && descriptor["reversibility"] == serde_json::json!([]),
            "boundary powers differ from the supported native adapter",
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/profiles/validation.rs"]
mod tests;
