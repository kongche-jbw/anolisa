//! Main-agent stopping authority and failure isolation over real Host processes.

use super::{
    lifecycle::{lifecycle_config, native},
    *,
};
use aw_hook_cli::interactive::stop_callback_with_cancellation as respond;

fn fixture_response(code: &str) -> Directory {
    let dir = lifecycle_config("print('{\"format\":1,\"observed\":true}')", |config| {
        let mut command = config["notifications"]["turn.stop"][0].clone();
        command["provider_id"] = json!("stop-policy");
        command["args"] = json!(["-c", code]);
        config["stop_response"] = command;
        config["notifications"] = json!({});
    });
    callback(&dir.0, native(&dir, "SessionStart", "s1", "unused")).unwrap();
    dir
}

fn stop(dir: &Directory) -> Value {
    let mut event = native(dir, "Stop", "s1", "unused");
    event["last_assistant_message"] = json!("private answer");
    event
}

#[test]
fn stop_and_continue_have_distinct_native_effects_and_private_journals() {
    for (decision, expected) in [
        (json!({"format":1,"decision":"allow_stop"}), json!({})),
        (
            json!({"format":1,"decision":"continue","reason":"private instruction"}),
            json!({"decision":"deny","reason":"private instruction"}),
        ),
    ] {
        let code = format!("import json,sys,pathlib\nr=json.load(sys.stdin)\nassert r['scope']=='turn.stop.respond' and r['format']==1\nassert r['event']['turn_id'] is None\nassert r['event']['payload']['last_assistant_message']=='private answer'\npathlib.Path('called').touch()\nprint({:?})", decision.to_string());
        let dir = fixture_response(&code);
        assert_eq!(respond(&dir.0, stop(&dir), &|| false).unwrap(), expected);
        for entry in fs::read_dir(dir.0.join("stop-response-journal")).unwrap() {
            let text = fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(!text.contains("private answer"));
            assert!(!text.contains("private instruction"));
            assert!(text.contains("unconfirmed"));
        }
        fs::remove_file(dir.0.join("called")).unwrap();
        assert_eq!(
            query(&dir.0).unwrap()["stop_response"],
            "experimental_native_response"
        );
        assert!(!dir.0.join("called").exists());
    }
}

#[test]
fn active_stop_skips_policy_and_new_native_stop_can_run_it() {
    let dir = fixture_response("import pathlib;pathlib.Path('called').touch();print('{\"format\":1,\"decision\":\"continue\",\"reason\":\"again\"}')");
    let mut event = stop(&dir);
    event["stop_hook_active"] = json!(true);
    for _ in 0..2 {
        assert_eq!(
            respond(&dir.0, event.clone(), &|| false).unwrap(),
            aw_adapters::qoder_stop::unavailable()
        );
        assert!(!dir.0.join("called").exists());
    }
    assert_eq!(
        respond(&dir.0, stop(&dir), &|| false).unwrap()["decision"],
        "deny"
    );
    assert!(dir.0.join("called").exists());
}

#[test]
fn malformed_output_nonzero_and_overflow_never_request_more_work() {
    for code in [
        "print('not json')",
        "print('{}')",
        "print('{\"format\":1,\"observed\":true}')",
        "print('{\"format\":1,\"decision\":\"continue\"}')",
        "print('{\"format\":1,\"decision\":\"allow_stop\",\"reason\":\"mixed\"}')",
        "print('{\"format\":1,\"decision\":\"allow_stop\",\"hookSpecificOutput\":{}}')",
        "print('{\"format\":1,\"format\":1,\"decision\":\"allow_stop\"}')",
        "print('x'*2048)",
        "import sys;print('{\"format\":1,\"decision\":\"allow_stop\"}');sys.exit(1)",
    ] {
        let dir = fixture_response(code);
        assert_eq!(
            respond(&dir.0, stop(&dir), &|| false).unwrap(),
            aw_adapters::qoder_stop::unavailable(),
            "{code}"
        );
    }
}

#[test]
fn changed_pin_or_invalid_identity_never_runs_policy() {
    let dir = fixture_response("import pathlib;pathlib.Path('called').touch()");
    for (key, value) in [
        ("session_id", json!("foreign")),
        ("agent_id", json!("child")),
        ("cwd", json!("/wrong")),
        ("stop_hook_active", json!(null)),
    ] {
        let mut event = stop(&dir);
        event[key] = value;
        assert_eq!(
            respond(&dir.0, event, &|| false).unwrap()["continue"],
            false
        );
        assert!(!dir.0.join("called").exists());
    }
    let path = dir.0.join("binding.json");
    let mut binding: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    binding["prepared"]["config"]["stop_response"]["program_sha256"] = json!("0".repeat(64));
    write(&path, &binding);
    assert_eq!(
        respond(&dir.0, stop(&dir), &|| false).unwrap()["continue"],
        false
    );
    assert!(!dir.0.join("called").exists());
    fs::remove_file(path).unwrap();
    assert!(respond(&dir.0, stop(&dir), &|| false).is_err());
}

#[test]
fn other_events_and_unconfigured_entry_cannot_gain_stop_authority() {
    let dir = fixture_response("import pathlib;pathlib.Path('called').touch()");
    for name in ["UserPromptSubmit", "SessionStart", "SubagentStop"] {
        assert!(respond(&dir.0, native(&dir, name, "s1", "unused"), &|| false).is_err());
    }
    let path = dir.0.join("binding.json");
    let mut binding: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    binding["prepared"]["config"]
        .as_object_mut()
        .unwrap()
        .remove("stop_response");
    write(&path, &binding);
    assert!(respond(&dir.0, stop(&dir), &|| false).is_err());
    assert!(!dir.0.join("called").exists());
}

#[test]
fn timeout_and_cancellation_reap_the_owned_response_process() {
    for cancel in [false, true] {
        let dir = fixture_response("import pathlib,os,time\npathlib.Path('pid').write_text(str(os.getpid()))\ntime.sleep(10)");
        assert_eq!(
            respond(&dir.0, stop(&dir), &|| cancel && dir.0.join("pid").exists()).unwrap()
                ["continue"],
            false
        );
        let pid = fs::read_to_string(dir.0.join("pid")).unwrap();
        assert!(!Path::new("/proc").join(pid).exists());
    }
}

#[test]
fn reset_and_session_end_withhold_old_continuation() {
    for reset in [false, true] {
        let dir = fixture_response("import pathlib,os,time\npathlib.Path('pid').write_text(str(os.getpid()))\ntime.sleep(10)\nprint('{\"format\":1,\"decision\":\"continue\",\"reason\":\"stale\"}')");
        let root = dir.0.clone();
        let event = stop(&dir);
        let worker = thread::spawn(move || respond(&root, event, &|| false));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !dir.0.join("pid").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        callback(
            &dir.0,
            native(
                &dir,
                if reset { "SessionStart" } else { "SessionEnd" },
                if reset { "s2" } else { "s1" },
                "unused",
            ),
        )
        .unwrap();
        assert_eq!(worker.join().unwrap().unwrap()["continue"], false);
        let pid = fs::read_to_string(dir.0.join("pid")).unwrap();
        assert!(!Path::new("/proc").join(pid).exists());
    }
}

#[test]
fn observer_cannot_grant_control_or_suppress_the_separate_response() {
    let dir = lifecycle_config(
        "print('{\"decision\":\"deny\",\"reason\":\"observer instruction\"}')",
        |config| {
            let mut command = config["notifications"]["turn.stop"][0].clone();
            command["provider_id"] = json!("stop-policy");
            command["args"] = json!(["-c", "print('{\"format\":1,\"decision\":\"continue\",\"reason\":\"response instruction\"}')"]);
            config["stop_response"] = command;
            let route = config["notifications"]["turn.stop"].clone();
            config["notifications"] = json!({"turn.stop":route});
        },
    );
    callback(&dir.0, native(&dir, "SessionStart", "s1", "unused")).unwrap();
    assert_eq!(
        respond(&dir.0, stop(&dir), &|| false).unwrap()["reason"],
        "response instruction"
    );
    assert_eq!(query(&dir.0).unwrap()["failed"], 1);
}

#[test]
fn stop_response_shares_the_original_notification_deadline() {
    let dir = lifecycle_config(
        "import time;time.sleep(.7);print('{\"format\":1,\"observed\":true}')",
        |config| {
            let mut notify = config["notifications"]["turn.stop"][0].clone();
            notify["limits"]["timeout_ms"] = json!(1000);
            let mut second = notify.clone();
            second["provider_id"] = json!("second");
            let mut response = notify.clone();
            response["provider_id"] = json!("stop-policy");
            response["args"] = json!(["-c", "import pathlib,os,time;pathlib.Path('pid').write_text(str(os.getpid()));time.sleep(.9);print('{\"format\":1,\"decision\":\"continue\",\"reason\":\"late\"}')"]);
            config["notifications"] = json!({"turn.stop":[notify,second]});
            config["stop_response"] = response;
        },
    );
    callback(&dir.0, native(&dir, "SessionStart", "s1", "unused")).unwrap();
    let start = Instant::now();
    assert_eq!(
        respond(&dir.0, stop(&dir), &|| false).unwrap()["continue"],
        false
    );
    // The two-second dispatch budget excludes Host's one-second cleanup grace.
    // Allow scheduling overhead while asserting that no late continuation escapes.
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "{:?}",
        start.elapsed()
    );
    if dir.0.join("pid").exists() {
        let pid = fs::read_to_string(dir.0.join("pid")).unwrap();
        assert!(!Path::new("/proc").join(pid).exists());
    }
}

#[test]
fn stop_response_admission_keeps_required_safety_and_budget_limits() {
    let dir = fixture_response("print('{\"format\":1,\"decision\":\"allow_stop\"}')");
    let path = dir.0.join("config.json");
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for case in 0..3 {
        let mut config = original.clone();
        match case {
            0 => config["required_safety"] = json!(true),
            1 => config["stop_response"]["limits"]["timeout_ms"] = json!(1001),
            _ => config["format"] = json!(1),
        }
        write(&path, &config);
        let root = dir.0.join(format!("rejected-{case}"));
        assert!(prepare(
            &path,
            &canonical::digest(&fs::read(&path).unwrap()),
            &root,
            Path::new("/bin/echo")
        )
        .is_err());
        assert!(!root.exists());
    }
}
