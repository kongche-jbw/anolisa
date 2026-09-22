//! Per-attachment receipt counts stay separate from handler and native effects.

use super::{Binding, Error, State};
use aw_contracts::events::EventName;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) const EVENTS: [EventName; 16] = {
    use EventName::*;
    [
        SessionStart,
        InputSubmit,
        ToolBefore,
        ToolAfter,
        PermissionRequest,
        CompactBefore,
        CompactAfter,
        SubagentStart,
        SubagentStop,
        TurnStop,
        SessionEnd,
        ModelBeforeRequest,
        RuntimeObserved,
        RuntimeExited,
        SecurityViolation,
        CoverageChanged,
    ]
};

#[derive(Default, Serialize)]
pub(super) struct Handlers {
    completed: u64,
    failed: u64,
    pending: u64,
}

impl Handlers {
    pub(super) fn record(&mut self, status: Option<&str>) -> Result<(), Error> {
        match status {
            Some("completed") => self.completed += 1,
            Some("failed") => self.failed += 1,
            Some("pending") => self.pending += 1,
            _ => return Err(Error::Evidence),
        }
        Ok(())
    }
}

pub(super) fn snapshot(
    binding: &Binding,
    state: &State,
    handlers: &BTreeMap<EventName, Handlers>,
) -> Value {
    let rows: Vec<_> = EVENTS
        .into_iter()
        .map(|event| {
            let native: Vec<_> = aw_adapters::qoder_events::NATIVE_EVENTS
                .into_iter()
                .filter(|name| aw_adapters::qoder_events::event_name(name).ok() == Some(event))
                .collect();
            let unsupported = matches!(
                event,
                EventName::ModelBeforeRequest | EventName::SecurityViolation
            );
            let configured = binding
                .prepared
                .config
                .notifications
                .as_ref()
                .is_some_and(|routes| routes.contains_key(&event));
            let empty = Handlers::default();
            let handler = handlers.get(&event).unwrap_or(&empty);
            let received = if !native.is_empty() {
                state
                    .event_counts
                    .as_ref()
                    .map(|counts| counts.get(&event).copied().unwrap_or(0))
            } else if !unsupported && configured {
                Some(handler.completed + handler.failed + handler.pending)
            } else {
                None
            };
            let status = if unsupported {
                "unsupported"
            } else if native.is_empty() && !configured {
                "not_configured"
            } else {
                match received {
                    Some(0) => "waiting",
                    Some(_) => "received",
                    None => "unavailable",
                }
            };
            json!({"event":event,"source":if unsupported {"unsupported"} else if native.is_empty() {"owner"} else {"native_callback"},
            "native_hooks":native,"status":status,"received":received,
            "handlers_configured":configured,"handlers":handler})
        })
        .collect();
    json!(rows)
}

/// Fixed ASCII labels prevent native payloads from entering terminal metadata.
pub(super) fn tokens(view: Option<&Value>) -> Value {
    let mut tokens = serde_json::Map::new();
    for (index, event) in EVENTS.iter().enumerate() {
        let name = serde_json::to_value(event).unwrap_or(Value::Null);
        let label = name.as_str().unwrap_or("unavailable");
        let row = view
            .and_then(|v| v["events"].as_array())
            .and_then(|rows| rows.iter().find(|row| row["event"] == name));
        let status = match row.and_then(|r| r["status"].as_str()) {
            Some("received") => format!(
                "seen {}",
                row.map(|r| &r["received"]).unwrap_or(&Value::Null)
            ),
            Some("waiting") => "waiting".into(),
            Some("unsupported") if *event == EventName::SecurityViolation => {
                "final not wired".into()
            }
            Some("unsupported") => "not wired".into(),
            Some("not_configured") => "not attached".into(),
            _ if view.is_none() => "idle".into(),
            _ => "unavailable".into(),
        };
        let failure = row
            .and_then(|r| r["handlers"]["failed"].as_u64())
            .unwrap_or(0);
        let mut suffix = if failure > 0 {
            format!(" / handler failed {failure}")
        } else {
            String::new()
        };
        if let Some(v) = view {
            if *event == EventName::ToolBefore && v["tool_guard"] != "not_configured" {
                suffix.push_str(&effect_suffix(v, "checks"));
            } else if *event == EventName::ToolAfter && v["tool_response"] != "not_configured" {
                suffix.push_str(&effect_suffix(v, "projections"));
            }
            if index == 0 && v["observation_gap"] == true {
                suffix.push_str(" / GAP!");
            }
        }
        tokens.insert(
            format!("aw_event_{:02}", index + 1),
            json!(format!("{label}: {status}{suffix}")),
        );
    }
    Value::Object(tokens)
}

fn effect_suffix(view: &Value, kind: &str) -> String {
    if view["effects"]["status"] != "available" {
        return " / evidence unavailable".into();
    }
    let counts = &view["effects"][kind];
    if counts["failed"].as_u64().unwrap_or(0) > 0 {
        return format!(" / failed {}", counts["failed"]);
    }
    if counts["pending"].as_u64().unwrap_or(0) > 0 {
        return format!(" / pending {}", counts["pending"]);
    }
    if kind == "checks" {
        format!(" / pass {} deny {}", counts["passed"], counts["denied"])
    } else {
        format!(
            " / candidate {} kept {}",
            counts["candidates"], counts["preserved"]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_separate_receipt_from_denial_candidate_and_missing_evidence() {
        let mut view = json!({"events":[
            {"event":"session.start","status":"received","received":1},
            {"event":"tool.before","status":"received","received":2},
            {"event":"tool.after","status":"received","received":1},
            {"event":"model.before_request","status":"unsupported"}],
            "tool_guard":"configured_not_certified","tool_response":"experimental_bash_result_response",
            "observation_gap":true,"effects":{"status":"available",
                "checks":{"passed":1,"denied":1,"failed":0,"pending":0},
                "projections":{"candidates":1,"preserved":0,"failed":0,"pending":0}}});
        let rows = tokens(Some(&view));
        assert_eq!(rows.as_object().unwrap().len(), 16);
        assert!(rows["aw_event_01"].as_str().unwrap().contains("GAP!"));
        assert_eq!(rows["aw_event_03"], "tool.before: seen 2 / pass 1 deny 1");
        assert_eq!(
            rows["aw_event_04"],
            "tool.after: seen 1 / candidate 1 kept 0"
        );
        assert_eq!(rows["aw_event_12"], "model.before_request: not wired");
        assert!(!rows.to_string().contains("adopted"));
        view["effects"]["status"] = json!("unavailable");
        assert!(tokens(Some(&view))["aw_event_03"]
            .as_str()
            .unwrap()
            .contains("evidence unavailable"));
        assert!(!tokens(Some(&view))["aw_event_03"]
            .as_str()
            .unwrap()
            .contains("pass"));
        assert_eq!(tokens(None)["aw_event_03"], "tool.before: idle");
    }
}
