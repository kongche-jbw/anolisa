//! Aggregate current runtime facts without merging their evidence or claiming adoption.

use serde_json::{json, Map, Value};

const ROWS: [(&str, &[&str]); 13] = [
    ("session.start/end", &["session.start", "session.end"]),
    ("input.submit", &["input.submit"]),
    ("tool.before", &["tool.before"]),
    ("tool.after", &["tool.after"]),
    ("permission.request", &["permission.request"]),
    ("compact.before/after", &["compact.before", "compact.after"]),
    ("subagent.start/stop", &["subagent.start", "subagent.stop"]),
    ("turn.stop", &["turn.stop"]),
    ("model.before_request", &["model.before_request"]),
    ("runtime.observed", &["runtime.observed"]),
    ("runtime.exited", &["runtime.exited"]),
    ("security.violation", &["security.violation"]),
    ("coverage.changed", &["coverage.changed"]),
];

/// Summarize live runtime queries in bounded, payload-free workspace metadata.
///
/// `missing` counts panes whose current report is unknown or expired. Counts
/// cover only `views`; every event row is explicitly partial when reports are
/// missing. Callers must exclude exited runtimes and deduplicate runtime IDs.
pub fn tokens(views: &[Value], missing: usize) -> Value {
    let mut result = Map::new();
    let mut states = [0usize; 4];
    for view in views {
        states[match view["activity"].as_str() {
            Some("working") => 0,
            Some("idle") => 1,
            Some("blocked") => 2,
            _ => 3,
        }] += 1;
    }
    let agents = if views.is_empty() && missing == 0 {
        "no active Agents / idle".into()
    } else {
        format!(
            "live Agents {}: working {} idle {} blocked {} unknown {}{}",
            views.len(),
            states[0],
            states[1],
            states[2],
            states[3],
            if missing > 0 { " / partial" } else { "" }
        )
    };
    result.insert("aw_ws_agents".into(), bounded("Agents", agents, false, ""));
    let gaps = views
        .iter()
        .filter(|view| view["observation_gap"] == true)
        .count();
    result.insert(
        "aw_ws_health".into(),
        bounded(
            "reports",
            format!(
                "live reports {} missing {} gap {}; effects unconfirmed{}",
                views.len(),
                missing,
                gaps,
                if missing > 0 { " / partial" } else { "" }
            ),
            false,
            if gaps > 0 { " GAP!" } else { "" },
        ),
    );
    for (index, (label, events)) in ROWS.iter().enumerate() {
        let mut counts = Counts::default();
        for view in views {
            for (event_index, event) in events.iter().enumerate() {
                counts.event(view, event, event_index);
            }
            if *label == "tool.before" {
                counts.effects(
                    view,
                    "tool_guard",
                    "checks",
                    &["passed", "denied", "failed", "pending"],
                );
            } else if *label == "tool.after" {
                counts.effects(
                    view,
                    "tool_response",
                    "projections",
                    &["candidates", "preserved", "failed", "pending"],
                );
            }
        }
        let mut text = format!("{label}:");
        if views.is_empty() {
            text.push_str(if missing > 0 { " unavailable" } else { " idle" });
        } else if counts.known > 0 {
            text.push_str(&format!(" seen {}", counts.event_count(0)));
            if events.len() == 2 {
                text.push_str(&format!("/{}", counts.event_count(1)));
            }
        }
        for (name, value) in [
            ("not wired", counts.unsupported),
            ("off", counts.off),
            ("unavailable", counts.unavailable),
            ("handler-failed", counts.failed),
            ("handler-pending", counts.pending),
        ] {
            if value > 0 {
                text.push_str(&format!(" {name} {value}"));
            }
        }
        if *label == "security.violation" && counts.unsupported > 0 {
            text.push_str(" final");
        }
        if counts.effect_known > 0 {
            let names = if *label == "tool.before" {
                ["pass", "deny", "failed", "pending"]
            } else {
                ["candidate", "kept", "failed", "pending"]
            };
            for (name, value) in names.into_iter().zip(counts.effect_counts) {
                text.push_str(&format!(" {name} {value}"));
            }
        }
        if counts.effect_off > 0 {
            text.push_str(&format!(" effects-off {}", counts.effect_off));
        }
        if counts.effect_unavailable > 0 {
            text.push_str(&format!(
                " effects-unavailable {}",
                counts.effect_unavailable
            ));
        }
        if missing > 0 {
            text.push_str(&format!(" partial(missing {missing})"));
        }
        let marker = if counts.failed > 0
            || counts.effect_counts[2] > 0
            || (*label == "tool.before" && counts.effect_counts[1] > 0)
        {
            " ALERT"
        } else if counts.pending > 0 || counts.effect_counts[3] > 0 {
            " WAIT"
        } else {
            ""
        };
        result.insert(
            format!("aw_ws_{:02}", index + 1),
            bounded(label, text, counts.overflow, marker),
        );
    }
    Value::Object(result)
}

fn bounded(label: &str, mut text: String, overflow: bool, marker: &str) -> Value {
    text.push_str(marker);
    if overflow || text.len() > 80 {
        // Never truncate away a denial, failure or missing-report warning.
        json!(format!(
            "{label}: overflow; partial counts; inspect Agent details{marker}"
        ))
    } else {
        json!(text)
    }
}

#[derive(Default)]
struct Counts {
    received: [u64; 2],
    known_by_event: [u64; 2],
    known: u64,
    unsupported: u64,
    off: u64,
    unavailable: u64,
    failed: u64,
    pending: u64,
    effect_known: u64,
    effect_off: u64,
    effect_unavailable: u64,
    effect_counts: [u64; 4],
    overflow: bool,
}

impl Counts {
    fn event_count(&self, index: usize) -> String {
        if self.known_by_event[index] > 0 {
            self.received[index].to_string()
        } else {
            "?".into()
        }
    }

    fn event(&mut self, view: &Value, event: &str, index: usize) {
        let row = view["events"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["event"] == event));
        let Some(row) = row else {
            self.unavailable += 1;
            return;
        };
        match row["status"].as_str() {
            Some("received" | "waiting") => match row["received"].as_u64() {
                Some(count) => {
                    self.known += 1;
                    self.known_by_event[index] += 1;
                    add(&mut self.received[index], count, &mut self.overflow);
                }
                None => self.unavailable += 1,
            },
            Some("unsupported") => self.unsupported += 1,
            Some("not_configured") => self.off += 1,
            _ => self.unavailable += 1,
        }
        for (name, sum) in [("failed", &mut self.failed), ("pending", &mut self.pending)] {
            if let Some(count) = row["handlers"][name].as_u64() {
                add(sum, count, &mut self.overflow);
            }
        }
    }

    fn effects(&mut self, view: &Value, configuration: &str, kind: &str, fields: &[&str; 4]) {
        match view[configuration].as_str() {
            Some("not_configured") => {
                self.effect_off += 1;
                return;
            }
            None => {
                self.effect_unavailable += 1;
                return;
            }
            Some(_) => {}
        }
        let values: Option<Vec<_>> = fields
            .iter()
            .map(|field| view["effects"][kind][field].as_u64())
            .collect();
        if view["effects"]["status"] != "available" || values.is_none() {
            self.effect_unavailable += 1;
            return;
        }
        if let Some(values) = values {
            self.effect_known += 1;
            for (sum, value) in self.effect_counts.iter_mut().zip(values) {
                add(sum, value, &mut self.overflow);
            }
        }
    }
}

fn add(sum: &mut u64, value: u64, overflow: &mut bool) {
    if let Some(total) = sum.checked_add(value) {
        *sum = total;
    } else {
        *overflow = true;
    }
}

#[cfg(test)]
mod tests;
