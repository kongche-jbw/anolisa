//! Lifecycle routing, privacy and authority regressions over real command processes.

use super::*;
use aw_adapters::qoder_events::NATIVE_EVENTS;

const RECORD: &str = "import json,sys,pathlib\ne=json.load(sys.stdin)\nwith pathlib.Path('events').open('a') as f:f.write(json.dumps(e)+'\\n')\nprint('{\"format\":1,\"observed\":true}')";

fn lifecycle(code: &str) -> Directory {
    lifecycle_config(code, |_| {})
}

pub(super) fn lifecycle_config(code: &str, update: impl FnOnce(&mut Value)) -> Directory {
    fixture_config(code, |config| {
        let handler = config.as_object_mut().unwrap().remove("handler").unwrap();
        config["format"] = json!(2);
        config["cwd"] = handler["cwd"].clone();
        config["notifications"] = json!({});
        for native in NATIVE_EVENTS {
            let name = serde_json::to_value(aw_adapters::qoder_events::event_name(native).unwrap())
                .unwrap();
            config["notifications"][name.as_str().unwrap()] = json!([handler]);
        }
        update(config);
    })
}

#[test]
fn a_later_command_pin_is_checked_before_any_command_executes() {
    let dir = lifecycle_config(RECORD, |config| {
        let mut later = config["notifications"]["session.start"][0].clone();
        later["provider_id"] = json!("second");
        later["program_sha256"] = json!("0".repeat(64));
        config["notifications"]["session.start"]
            .as_array_mut()
            .unwrap()
            .push(later);
    });
    let response = callback(&dir.0, native(&dir, "SessionStart", "s1", "t1")).unwrap();
    assert!(response.get("systemMessage").is_some());
    assert!(!dir.0.join("events").exists());
    assert_eq!(query(&dir.0).unwrap()["failed"], 1);
    let view = query(&dir.0).unwrap();
    assert_eq!(view["events"][0]["received"], 1);
    assert_eq!(view["events"][0]["handlers"]["failed"], 1);
}

#[test]
fn ordered_commands_receive_the_same_fact_without_transform_authority() {
    let dir = lifecycle_config(RECORD, |config| {
        let mut later = config["notifications"]["session.start"][0].clone();
        later["provider_id"] = json!("second");
        later["args"] = json!(["-c", "import json,sys,pathlib\ne=json.load(sys.stdin)\nfirst=json.loads(pathlib.Path('events').read_text())\nassert first==e\npathlib.Path('second').write_text('done')\nprint('{\"format\":1,\"observed\":true}')"]);
        config["notifications"]["session.start"]
            .as_array_mut()
            .unwrap()
            .push(later);
    });
    callback(&dir.0, native(&dir, "SessionStart", "s1", "t1")).unwrap();
    assert_eq!(fs::read_to_string(dir.0.join("second")).unwrap(), "done");
    assert_eq!(query(&dir.0).unwrap()["observed"], 1);
}

#[test]
fn reset_cancels_an_inflight_command_and_leaves_old_evidence_scoped() {
    let code = "import pathlib,os,time\npathlib.Path('handler-pid').write_text(str(os.getpid()))\ntime.sleep(10)";
    let dir = lifecycle_config(code, |config| {
        let route = config["notifications"]["tool.after"].clone();
        config["notifications"] = json!({"tool.after":route});
    });
    callback(&dir.0, native(&dir, "SessionStart", "s1", "t1")).unwrap();
    callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap();
    let root = dir.0.clone();
    let post = native(&dir, "PostToolUse", "s1", "t1");
    let worker = thread::spawn(move || callback(&root, post));
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dir.0.join("handler-pid").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    callback(&dir.0, native(&dir, "SessionStart", "s2", "t2")).unwrap();
    assert!(worker
        .join()
        .unwrap()
        .unwrap()
        .get("systemMessage")
        .is_some());
    let pid = fs::read_to_string(dir.0.join("handler-pid")).unwrap();
    assert!(!Path::new("/proc").join(pid).exists());
    assert_eq!(query(&dir.0).unwrap()["calls"], 0);
}

pub(super) fn native(dir: &Directory, kind: &str, session: &str, tool: &str) -> Value {
    let mut value = event(dir, kind, session, tool);
    value["reason"] = json!("prompt_input_exit");
    value["prompt"] = json!("same input is not a replay");
    value["trigger"] = json!("manual");
    value["compact_summary"] = json!("retained context");
    value["stop_hook_active"] = json!(false);
    value["error"] = json!("failed tool result");
    if matches!(kind, "SubagentStart" | "SubagentStop") {
        value["agent_id"] = json!("child-one");
        value["agent_type"] = json!("Explore");
    }
    value
}

fn recorded(dir: &Directory) -> Vec<Value> {
    fs::read_to_string(dir.0.join("events"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}

#[test]
fn eleven_lifecycle_routes_preserve_native_payloads_and_unknown_turns() {
    let dir = lifecycle(RECORD);
    let sequence = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PermissionRequest",
        "PostToolUse",
        "PreCompact",
        "PostCompact",
        "SubagentStart",
        "SubagentStop",
        "Stop",
        "SessionEnd",
    ];
    for name in sequence {
        assert_eq!(
            callback(&dir.0, native(&dir, name, "s1", "t1")).unwrap(),
            json!({})
        );
    }
    let events = recorded(&dir);
    assert_eq!(events.len(), 11);
    for (event, name) in events.iter().zip(sequence) {
        assert_eq!(event["payload"], native(&dir, name, "s1", "t1"));
        assert_eq!(event["native_event"], name);
        assert_eq!(event["turn_id"], Value::Null);
        assert_eq!(event["session_epoch"], 1);
    }
    assert_eq!(events[7]["subagent_id"], "child-one");
    assert_eq!(query(&dir.0).unwrap()["observed"], 11);
    assert!(!query(&dir.0).unwrap()["attached"].as_bool().unwrap());
    assert_eq!(recorded(&dir).len(), 11);
    for entry in fs::read_dir(dir.0.join("notifications")).unwrap() {
        let text = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!text.contains("private result"));
        assert!(!text.contains("same input is not a replay"));
    }
}

#[test]
fn failed_tools_keep_error_shape_and_cannot_be_replayed_as_success() {
    let dir = lifecycle(RECORD);
    callback(&dir.0, native(&dir, "SessionStart", "s1", "t1")).unwrap();
    callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap();
    let mut failed = native(&dir, "PostToolUseFailure", "s1", "t1");
    failed.as_object_mut().unwrap().remove("tool_response");
    callback(&dir.0, failed.clone()).unwrap();
    assert_eq!(recorded(&dir)[2]["payload"], failed);
    assert_eq!(recorded(&dir)[2]["event"], "tool.after");
    assert!(callback(&dir.0, native(&dir, "PostToolUse", "s1", "t1")).is_err());
    assert_eq!(recorded(&dir).len(), 3);
}

#[test]
fn identical_prompts_remain_distinct_occurrences_without_invented_turns() {
    let dir = lifecycle(RECORD);
    callback(&dir.0, native(&dir, "SessionStart", "s1", "t1")).unwrap();
    let prompt = native(&dir, "UserPromptSubmit", "s1", "t1");
    callback(&dir.0, prompt.clone()).unwrap();
    callback(&dir.0, prompt).unwrap();
    let events = recorded(&dir);
    assert_ne!(events[1]["occurrence_id"], events[2]["occurrence_id"]);
    assert_eq!(events[1]["turn_id"], Value::Null);
}

#[test]
fn source_spoofing_and_child_main_loop_callbacks_cannot_execute_commands() {
    let dir = lifecycle(RECORD);
    callback(&dir.0, native(&dir, "SessionStart", "s1", "t1")).unwrap();
    for name in [
        "runtime.exited",
        "RuntimeExited",
        "model.before_request",
        "security.violation",
    ] {
        assert!(callback(&dir.0, native(&dir, name, "s1", "t1")).is_err());
    }
    let mut child = native(&dir, "Stop", "s1", "t1");
    child["agent_id"] = json!("unbound-child");
    assert!(callback(&dir.0, child).is_err());
    let mut malformed = native(&dir, "UserPromptSubmit", "s1", "t1");
    malformed["prompt"] = json!({"not":"text"});
    assert!(callback(&dir.0, malformed).is_err());
    assert_eq!(recorded(&dir).len(), 1);
}

#[test]
fn reset_fences_notifications_and_retires_the_old_session() {
    let dir = lifecycle(RECORD);
    callback(&dir.0, native(&dir, "SessionStart", "s1", "t1")).unwrap();
    callback(&dir.0, native(&dir, "SessionEnd", "s1", "t1")).unwrap();
    let mut start = native(&dir, "SessionStart", "s2", "t2");
    start["source"] = json!("clear");
    callback(&dir.0, start).unwrap();
    assert!(callback(&dir.0, native(&dir, "Stop", "s1", "t1")).is_err());
    assert_eq!(query(&dir.0).unwrap()["observed"], 1);
    assert_eq!(recorded(&dir)[2]["session_epoch"], 2);
}

#[test]
fn notification_stdout_cannot_grant_permission_or_replace_native_content() {
    let dir = lifecycle(
        "print('{\"continue\":false,\"hookSpecificOutput\":{\"permissionDecision\":\"allow\"}}')",
    );
    let response = callback(&dir.0, native(&dir, "SessionStart", "s1", "t1")).unwrap();
    assert!(response.get("systemMessage").is_some());
    assert!(response.get("continue").is_none());
    assert!(response.get("hookSpecificOutput").is_none());
    assert_eq!(query(&dir.0).unwrap()["failed"], 1);
}

#[test]
fn unconnected_event_routes_fail_admission_before_creating_scope() {
    let dir = Directory::new();
    for name in ["model.before_request", "security.violation"] {
        let mut value = config(&dir, RECORD);
        let handler = value.as_object_mut().unwrap().remove("handler").unwrap();
        value["format"] = json!(2);
        value["cwd"] = handler["cwd"].clone();
        value["notifications"] = json!({name:[handler]});
        let path = dir.0.join("config.json");
        write(&path, &value);
        let digest = canonical::digest(&fs::read(&path).unwrap());
        assert!(prepare(&path, &digest, &dir.0.join("scope"), Path::new("/bin/echo")).is_err());
        assert!(!dir.0.join("scope").exists());
    }
}

#[test]
fn native_login_failure_can_end_without_a_session_start() {
    let dir = lifecycle(RECORD);
    // Field set observed from the real isolated Qoder 1.1.47 admission probe.
    let end = json!({"cwd":dir.0,"hook_event_name":"SessionEnd", "permission_mode":"default",
        "reason":"other","session_id":"failed-login","transcript_path":"/unused"});
    assert_eq!(callback(&dir.0, end).unwrap(), json!({}));
    assert_eq!(recorded(&dir)[0]["event"], "session.end");
    assert_eq!(query(&dir.0).unwrap()["attached"], false);
    assert!(callback(
        &dir.0,
        native(&dir, "UserPromptSubmit", "failed-login", "t1")
    )
    .is_err());
}

#[test]
fn first_trusted_input_binds_session_but_preserves_missing_start_evidence() {
    let dir = lifecycle(RECORD);
    let mut child = native(&dir, "UserPromptSubmit", "s1", "t1");
    child["agent_id"] = json!("unbound-child");
    assert!(callback(&dir.0, child).is_err());
    assert!(callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).is_err());
    callback(&dir.0, native(&dir, "UserPromptSubmit", "s1", "t1")).unwrap();
    let view = query(&dir.0).unwrap();
    assert_eq!(view["attached"], true);
    assert_eq!(view["observation_gap"], true);
    assert_eq!(view["session_start_observed"], false);
    callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap();
    callback(&dir.0, native(&dir, "PostToolUse", "s1", "t1")).unwrap();
    assert_eq!(recorded(&dir).len(), 3);
    assert_eq!(recorded(&dir)[0]["event"], "input.submit");
    assert!(callback(&dir.0, native(&dir, "UserPromptSubmit", "s2", "t2")).is_err());
    callback(&dir.0, native(&dir, "SessionEnd", "s1", "t1")).unwrap();
    assert!(callback(&dir.0, native(&dir, "UserPromptSubmit", "s2", "t2")).is_err());
    callback(&dir.0, native(&dir, "SessionStart", "s2", "t2")).unwrap();
    assert_eq!(query(&dir.0).unwrap()["session_start_observed"], true);
    assert_eq!(query(&dir.0).unwrap()["attachment"], 2);
}

#[test]
fn event_receipts_do_not_require_handlers_or_retain_private_payloads() {
    let dir = super::tool_guard::fixture_guard(None, super::tool_guard::SCANNER);
    let sequence = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUseFailure",
        "Stop",
    ];
    for name in sequence {
        callback(&dir.0, native(&dir, name, "s1", "t1")).unwrap();
    }
    // An envelope rejected after native shape validation is not a receipt.
    let mut invalid = native(&dir, "SubagentStart", "s1", "t1");
    invalid["agent_id"] = json!("x".repeat(257));
    assert!(callback(&dir.0, invalid).is_err());
    let view = query(&dir.0).unwrap();
    assert_eq!(view["calls"], 0);
    assert_eq!(view["callbacks_in_runtime"], 5);
    let rows = view["events"].as_array().unwrap();
    assert_eq!(rows.len(), 16);
    for name in sequence {
        let event =
            serde_json::to_value(aw_adapters::qoder_events::event_name(name).unwrap()).unwrap();
        let row = rows.iter().find(|r| r["event"] == event).unwrap();
        assert_eq!(row["received"], 1);
        assert_eq!(row["status"], "received");
        assert_eq!(row["handlers_configured"], false);
    }
    for name in ["model.before_request", "security.violation"] {
        let row = rows.iter().find(|r| r["event"] == name).unwrap();
        assert_eq!(row["status"], "unsupported");
        assert!(row["received"].is_null());
    }
    assert_eq!(
        rows.iter()
            .find(|r| r["event"] == "runtime.observed")
            .unwrap()["status"],
        "not_configured"
    );
    assert_eq!(
        rows.iter().find(|r| r["event"] == "session.end").unwrap()["status"],
        "waiting"
    );
    // A replay is rejected before a second receipt is persisted.
    let replay = callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap();
    assert_eq!(replay["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(query(&dir.0).unwrap()["callbacks_in_runtime"], 5);
    let state = fs::read_to_string(dir.0.join("state.json")).unwrap();
    for private in [
        "private result",
        "same input",
        "failed tool result",
        "echo private",
    ] {
        assert!(!state.contains(private));
        assert!(!view.to_string().contains(private));
    }
    callback(&dir.0, native(&dir, "SessionStart", "s2", "t2")).unwrap();
    let view = query(&dir.0).unwrap();
    let rows = view["events"].as_array().unwrap();
    assert_eq!(
        rows.iter().find(|r| r["event"] == "tool.before").unwrap()["received"],
        0
    );
    assert_eq!(
        rows.iter().find(|r| r["event"] == "session.start").unwrap()["received"],
        1
    );
}

#[test]
fn old_state_without_event_counts_does_not_invent_zero_receipts() {
    let dir = super::tool_guard::fixture_guard(None, super::tool_guard::SCANNER);
    callback(&dir.0, native(&dir, "SessionStart", "s1", "t1")).unwrap();
    let path = dir.0.join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state.as_object_mut().unwrap().remove("event_counts");
    write(&path, &state);
    callback(&dir.0, native(&dir, "Stop", "s1", "t1")).unwrap();
    let view = query(&dir.0).unwrap();
    for row in view["events"].as_array().unwrap().iter().take(11) {
        assert_eq!(row["status"], "unavailable");
        assert!(row["received"].is_null());
    }
}

#[test]
fn activity_query_tracks_callbacks_and_invalidates_gaps_or_exited_owners() {
    let dir = super::tool_guard::fixture_guard(None, super::tool_guard::SCANNER);
    for (name, expected) in [
        ("SessionStart", "idle"),
        ("UserPromptSubmit", "working"),
        ("PermissionRequest", "blocked"),
        ("Stop", "idle"),
        ("UserPromptSubmit", "working"),
    ] {
        callback(&dir.0, native(&dir, name, "s1", "t1")).unwrap();
        let view = query(&dir.0).unwrap();
        assert_eq!(view["activity"], expected);
        assert_eq!(view["activity_source"], "native_callback_hint");
    }
    assert!(callback(&dir.0, native(&dir, "Stop", "wrong-session", "t1")).is_err());
    assert_eq!(query(&dir.0).unwrap()["activity"], "working");
    let path = dir.0.join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state["gap"] = json!(true);
    write(&path, &state);
    assert_eq!(query(&dir.0).unwrap()["activity"], "unknown");
    state["gap"] = json!(false);
    write(&path, &state);
    callback(&dir.0, native(&dir, "SessionEnd", "s1", "t1")).unwrap();
    assert_eq!(query(&dir.0).unwrap()["activity"], "unknown");
    callback(&dir.0, native(&dir, "SessionStart", "s2", "t1")).unwrap();
    let path = dir.0.join("binding.json");
    let mut binding: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    binding["agent_ticks"] = json!(0);
    write(&path, &binding);
    assert_eq!(query(&dir.0).unwrap()["activity"], "unknown");
}
