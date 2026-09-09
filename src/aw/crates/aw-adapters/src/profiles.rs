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
    let value = serde_json::from_str(source(host))
        .map_err(|_| Error::InvalidProfile("built-in profile is not valid JSON"))?;
    validate_profile(host, &value)?;
    Ok(value)
}

/// Returns the descriptor matching an exact native event name.
///
/// # Errors
/// Rejects unknown events and invalid built-in profiles. Event names are case
/// sensitive; aliases must never silently select a different hook boundary.
pub fn boundary(host: Host, native_event: &str) -> Result<Value, Error> {
    let value = profile(host)?;
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

fn validate_profile(host: Host, value: &Value) -> Result<(), Error> {
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
    let registry = Registry::new()?;
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
        let phase = match (host, event) {
            (Host::Hermes, "pre_tool_call") | (Host::OpenClaw, "before_tool_call") => "pre_tool",
            (Host::Hermes, "post_tool_call") | (Host::OpenClaw, "after_tool_call") => "post_tool",
            (Host::Qoder | Host::Codex | Host::QwenCode | Host::Cosh, "PreToolUse") => "pre_tool",
            (Host::Qoder | Host::Codex | Host::QwenCode | Host::Cosh, "PostToolUse") => "post_tool",
            _ => return Err(Error::InvalidProfile("unsupported native event")),
        };
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
                && descriptor["revision"]
                    == if host == Host::Qoder && phase == "post_tool" {
                        2
                    } else {
                        1
                    }
                && descriptor["boundary_id"] == format!("{}.{}", host.as_str(), phase)
                && descriptor["boundary"] == phase,
            "boundary identity differs from the pinned native mapping",
        )?;
        // Native denial is not proof of a guard over the final dispatched input.
        require(
            descriptor["can_deny_dispatch"] == false
                && descriptor["has_final_input_guard"] == false
                && descriptor["proof_boundaries"]
                    == if host == Host::Qoder && phase == "post_tool" {
                        serde_json::json!(["local_history"])
                    } else {
                        serde_json::json!([])
                    }
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
                && descriptor["reversibility"]
                    == if replaces {
                        serde_json::json!(["unrecoverable"])
                    } else {
                        serde_json::json!([])
                    },
            "boundary powers differ from the supported native adapter",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profile_format_and_mapping_are_closed() {
        let original = profile(Host::Qoder).unwrap();
        for (pointer, replacement) in [
            ("/format", json!(2)),
            ("/host", json!("codex")),
            ("/boundaries/0/native_event", json!("before_tool_call")),
            ("/boundaries/0/descriptor/boundary", json!("post_tool")),
            (
                "/boundaries/0/descriptor/boundary_id",
                json!("qoder.post_tool"),
            ),
        ] {
            let mut changed = original.clone();
            *changed.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                validate_profile(Host::Qoder, &changed).is_err(),
                "{pointer}"
            );
        }
        let mut changed = original.clone();
        changed["boundaries"][1] = changed["boundaries"][0].clone();
        assert!(validate_profile(Host::Qoder, &changed).is_err());
        let mut changed = original.clone();
        changed["unexpected"] = json!(true);
        assert!(validate_profile(Host::Qoder, &changed).is_err());
        let mut changed = original;
        changed["boundaries"][0]["priority"] = json!(0);
        assert!(validate_profile(Host::Qoder, &changed).is_err());
    }

    #[test]
    fn schema_valid_claims_cannot_elevate_native_authority() {
        let original = profile(Host::Qoder).unwrap();
        let mut changed = original.clone();
        let descriptor = &mut changed["boundaries"][0]["descriptor"];
        descriptor["can_deny_dispatch"] = json!(true);
        descriptor["has_final_input_guard"] = json!(true);
        descriptor["composition"]["input_finality"] = json!("immutable_until_dispatch");
        descriptor["composition"]["gate"] = json!("required_final_guard");
        Registry::new()
            .unwrap()
            .validate_boundary(descriptor)
            .unwrap();
        assert!(validate_profile(Host::Qoder, &changed).is_err());

        let mut changed = original;
        let descriptor = &mut changed["boundaries"][1]["descriptor"];
        descriptor["proof_boundaries"] = json!(["final_tool_result"]);
        descriptor["composition"]["result_finality"] = json!("final");
        descriptor["ledger_policy"] = json!("required_before_delivery");
        Registry::new()
            .unwrap()
            .validate_boundary(descriptor)
            .unwrap();
        assert!(validate_profile(Host::Qoder, &changed).is_err());
    }
}
