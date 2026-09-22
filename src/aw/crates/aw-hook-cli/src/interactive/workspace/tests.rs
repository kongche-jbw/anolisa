use super::*;

fn view(activity: &str, received: u64) -> Value {
    let events: Vec<_> = ROWS
        .iter()
        .flat_map(|(_, events)| *events)
        .map(|event| {
            let unsupported = matches!(*event, "model.before_request" | "security.violation");
            json!({"event":event,"status":if unsupported {"unsupported"} else {"received"},
            "received":if unsupported {None} else {Some(received)},
            "handlers":{"completed":0,"failed":0,"pending":0}})
        })
        .collect();
    json!({"activity":activity,"events":events,"observation_gap":false,
        "tool_guard":"configured_not_certified","tool_response":"experimental_bash_result_response",
        "effects":{"status":"available","checks":{"passed":0,"denied":0,"failed":0,"pending":0},
            "projections":{"candidates":0,"preserved":0,"failed":0,"pending":0}},
        "runtime_id":"PRIVATE_RUNTIME","session_id":"PRIVATE_SESSION",
        "payload":{"prompt":"PRIVATE_PROMPT","command":"PRIVATE_COMMAND"}})
}

fn row_mut<'a>(view: &'a mut Value, event: &str) -> &'a mut Value {
    view["events"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["event"] == event)
        .unwrap()
}

#[test]
fn empty_and_missing_reports_do_not_claim_success() {
    let empty = tokens(&[], 0);
    assert_eq!(empty["aw_ws_agents"], "no active Agents / idle");
    assert_eq!(empty["aw_ws_03"], "tool.before: idle");
    let missing = tokens(&[], 2);
    assert!(missing["aw_ws_agents"]
        .as_str()
        .unwrap()
        .contains("partial"));
    assert!(!missing.to_string().contains("no active"));
    for index in 1..=13 {
        let text = missing[format!("aw_ws_{index:02}")].as_str().unwrap();
        assert!(text.contains("unavailable") && text.contains("missing 2"));
        assert!(!text.contains("pass 0"));
    }
}

#[test]
fn sums_live_views_independent_of_order_and_drops_exited_runtime_counts() {
    let mut first = view("working", 1);
    let mut second = view("idle", 2);
    first["effects"]["checks"]["passed"] = json!(2);
    second["effects"]["checks"]["denied"] = json!(1);
    first["effects"]["projections"]["candidates"] = json!(1);
    second["effects"]["projections"]["preserved"] = json!(2);
    let combined = tokens(&[first.clone(), second.clone()], 0);
    assert_eq!(combined, tokens(&[second.clone(), first], 0));
    assert_eq!(
        combined["aw_ws_agents"],
        "live Agents 2: working 1 idle 1 blocked 0 unknown 0"
    );
    assert_eq!(combined["aw_ws_01"], "session.start/end: seen 3/3");
    assert_eq!(
        combined["aw_ws_03"],
        "tool.before: seen 3 pass 2 deny 1 failed 0 pending 0 ALERT"
    );
    assert_eq!(
        combined["aw_ws_04"],
        "tool.after: seen 3 candidate 1 kept 2 failed 0 pending 0"
    );
    assert_eq!(
        tokens(&[second], 0)["aw_ws_03"],
        "tool.before: seen 2 pass 0 deny 1 failed 0 pending 0 ALERT"
    );
}

#[test]
fn denial_and_failure_and_pending_are_all_visible() {
    let mut first = view("blocked", 3);
    first["effects"]["checks"] = json!({"passed":1,"denied":2,"failed":3,"pending":4});
    first["effects"]["projections"] = json!({"candidates":1,"preserved":2,"failed":3,"pending":4});
    let report = tokens(&[first], 0);
    assert_eq!(
        report["aw_ws_03"],
        "tool.before: seen 3 pass 1 deny 2 failed 3 pending 4 ALERT"
    );
    assert_eq!(
        report["aw_ws_04"],
        "tool.after: seen 3 candidate 1 kept 2 failed 3 pending 4 ALERT"
    );
}

#[test]
fn unsupported_off_unavailable_and_handler_problems_stay_distinct() {
    let mut first = view("unknown", 1);
    let mut second = view("idle", 2);
    first["observation_gap"] = json!(true);
    row_mut(&mut first, "runtime.observed")["status"] = json!("not_configured");
    row_mut(&mut second, "runtime.observed")["status"] = json!("unavailable");
    row_mut(&mut first, "turn.stop")["handlers"] = json!({"failed":1,"pending":2});
    first["tool_guard"] = json!("not_configured");
    second["effects"]["status"] = json!("unavailable");
    let report = tokens(&[first, second], 0);
    assert_eq!(report["aw_ws_10"], "runtime.observed: off 1 unavailable 1");
    assert_eq!(
        report["aw_ws_08"],
        "turn.stop: seen 3 handler-failed 1 handler-pending 2 ALERT"
    );
    assert_eq!(report["aw_ws_09"], "model.before_request: not wired 2");
    assert_eq!(report["aw_ws_12"], "security.violation: not wired 2 final");
    assert_eq!(
        report["aw_ws_03"],
        "tool.before: seen 3 effects-off 1 effects-unavailable 1"
    );
    assert!(report["aw_ws_health"].as_str().unwrap().contains("gap 1"));
    assert!(report["aw_ws_health"].as_str().unwrap().contains("GAP!"));
}

#[test]
fn known_counts_remain_partial_when_reports_are_missing() {
    let report = tokens(&[view("working", 2)], 1);
    for index in 1..=13 {
        assert!(report[format!("aw_ws_{index:02}")]
            .as_str()
            .unwrap()
            .contains("partial"));
    }
    assert!(report["aw_ws_02"].as_str().unwrap().contains("seen 2"));
    assert!(report["aw_ws_health"].as_str().unwrap().contains("partial"));
}

#[test]
fn bounded_overflow_never_leaks_private_fields_or_truncates_into_success() {
    let mut huge = view("PRIVATE_ACTIVITY", u64::MAX);
    huge["effects"]["checks"] =
        json!({"passed":u64::MAX,"denied":u64::MAX,"failed":u64::MAX,"pending":u64::MAX});
    let report = tokens(&[huge.clone(), huge], usize::MAX);
    assert!(report.as_object().unwrap().len() <= 16);
    for value in report.as_object().unwrap().values() {
        let text = value.as_str().unwrap();
        assert!(text.is_ascii() && text.len() <= 80, "{text}");
    }
    assert!(report["aw_ws_03"].as_str().unwrap().contains("overflow"));
    assert!(report["aw_ws_03"].as_str().unwrap().contains("ALERT"));
    assert!(report["aw_ws_01"].as_str().unwrap().contains("overflow"));
    assert!(!report.to_string().contains("PRIVATE"));
}

#[test]
fn unavailable_effects_and_rows_never_become_zero_success() {
    let mut item = view("idle", 0);
    item["events"] = json!([]);
    item["effects"]["checks"]["passed"] = json!("PRIVATE_INVALID_NUMBER");
    let report = tokens(&[item], 0);
    assert_eq!(
        report["aw_ws_03"],
        "tool.before: unavailable 1 effects-unavailable 1"
    );
    assert!(!report.to_string().contains("PRIVATE"));
}

#[test]
fn color_markers_require_positive_counts_and_prioritize_failure() {
    let mut item = view("working", 1);
    let report = tokens(&[item.clone()], 0);
    assert!(!report.to_string().contains("ALERT"));
    assert!(!report.to_string().contains("WAIT"));
    assert!(!report.to_string().contains("GAP!"));
    item["effects"]["checks"]["pending"] = json!(1);
    item["effects"]["projections"]["pending"] = json!(1);
    row_mut(&mut item, "turn.stop")["handlers"]["pending"] = json!(1);
    let report = tokens(&[item.clone()], 0);
    for key in ["aw_ws_03", "aw_ws_04", "aw_ws_08"] {
        assert!(report[key].as_str().unwrap().ends_with(" WAIT"));
    }
    item["effects"]["checks"]["failed"] = json!(1);
    item["effects"]["projections"]["failed"] = json!(1);
    row_mut(&mut item, "turn.stop")["handlers"]["failed"] = json!(1);
    let report = tokens(&[item], 0);
    for key in ["aw_ws_03", "aw_ws_04", "aw_ws_08"] {
        assert!(report[key].as_str().unwrap().ends_with(" ALERT"));
        assert!(!report[key].as_str().unwrap().contains("WAIT"));
    }
}

#[test]
fn grouped_events_preserve_direction_and_unknown_receipts() {
    let mut item = view("idle", 1);
    row_mut(&mut item, "session.end")["received"] = json!(0);
    row_mut(&mut item, "compact.after")["received"] = json!(2);
    row_mut(&mut item, "subagent.stop")["status"] = json!("unavailable");
    let report = tokens(&[item], 0);
    assert_eq!(report["aw_ws_01"], "session.start/end: seen 1/0");
    assert_eq!(report["aw_ws_06"], "compact.before/after: seen 1/2");
    assert_eq!(
        report["aw_ws_07"],
        "subagent.start/stop: seen 1/? unavailable 1"
    );
}
