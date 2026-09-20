//! Result projection, occurrence fencing and optional failure semantics over real Hosts.

use super::{
    lifecycle::{lifecycle_config, native},
    *,
};

const REPLACE: &str = "import json,sys,pathlib\nr=json.load(sys.stdin)\nassert r['format']==1 and r['scope']=='tool.after.respond'\nassert r['event']['turn_id'] is None\nassert r['accepted_reversibility']==['unrecoverable']\nassert r['source']['text']=='private original'\npathlib.Path('called').touch()\nprint(json.dumps({'format':1,'decision':'replace','text':'private candidate','source_digest':r['source']['digest']}))";

fn response_config(config: &mut Value, code: &str) {
    let mut command = config["notifications"]["tool.after"][0].clone();
    command["provider_id"] = json!("result-projector");
    command["args"] = json!(["-c", code]);
    command["limits"]["timeout_ms"] = json!(1000);
    config["tool_response"] = json!({"command":command,"accepted_reversibility":["unrecoverable"]});
    config["notifications"] = json!({});
}

fn fixture_response(code: &str) -> Directory {
    let dir = lifecycle_config("print('{\"format\":1,\"observed\":true}')", |config| {
        response_config(config, code)
    });
    callback(&dir.0, native(&dir, "SessionStart", "s1", "unused")).unwrap();
    callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap();
    dir
}

fn post(dir: &Directory) -> Value {
    let mut event = native(dir, "PostToolUse", "s1", "t1");
    event.as_object_mut().unwrap().remove("error");
    event["tool_response"] = json!({"kind":"completed","exitCode":0,"signal":null,
        "interrupted":false,"isImage":false,"noOutputExpected":false,"stderr":"",
        "stdout":"private original"});
    event
}

#[test]
fn replaces_only_the_native_text_slot_without_claiming_adoption() {
    let dir = fixture_response(REPLACE);
    let event = post(&dir);
    let result = callback(&dir.0, event.clone()).unwrap();
    assert_eq!(
        result,
        json!({"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":"private candidate"}})
    );
    assert_eq!(event["tool_response"]["stdout"], "private original");
    for entry in fs::read_dir(dir.0.join("tool-response-journal")).unwrap() {
        let record = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!record.contains("private original") && !record.contains("private candidate"));
    }
    fs::remove_file(dir.0.join("called")).unwrap();
    let view = query(&dir.0).unwrap();
    assert_eq!(view["effect"], "experimental_native_tool_response");
    assert_eq!(view["tool_response"], "experimental_bash_result_response");
    assert_eq!(view["adoption"], "unsupported");
    assert_eq!(view["required_safety"], "unsupported");
    assert!(!dir.0.join("called").exists());
    assert!(callback(&dir.0, event)
        .unwrap()
        .get("systemMessage")
        .is_some());
    assert!(!dir.0.join("called").exists());
}

#[test]
fn preserve_and_missing_authority_leave_original_result_unchanged() {
    let dir = fixture_response("print('{\"format\":1,\"decision\":\"preserve\"}')");
    assert_eq!(callback(&dir.0, post(&dir)).unwrap(), json!({}));
    let dir = lifecycle_config(
        "print('{\"format\":1,\"decision\":\"replace\",\"text\":\"bad\"}')",
        |_| {},
    );
    callback(&dir.0, native(&dir, "SessionStart", "s1", "unused")).unwrap();
    callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap();
    let result = callback(&dir.0, post(&dir)).unwrap();
    assert!(result.get("hookSpecificOutput").is_none());
}

#[test]
fn failed_malformed_and_foreign_source_responses_cannot_replace_or_retry() {
    for code in ["print('not json')", "raise SystemExit(1)",
        "print('{\"format\":1,\"decision\":\"replace\",\"text\":\"x\",\"source_digest\":\"foreign\"}')",
        "print('{\"format\":1,\"format\":1,\"decision\":\"preserve\"}')",
        "print('{\"format\":1,\"decision\":\"preserve\",\"continue\":true}')", "print('x'*65537)"]
    {
        let dir = fixture_response(code);
        let response = callback(&dir.0, post(&dir)).unwrap();
        assert!(response.get("systemMessage").is_some() && response.get("hookSpecificOutput").is_none());
        assert_eq!(query(&dir.0).unwrap()["observation_gap"], true);
        assert!(callback(&dir.0, post(&dir)).unwrap().get("hookSpecificOutput").is_none());
    }
}

#[test]
fn unsupported_result_shapes_do_not_run_projection() {
    for (key, value) in [
        ("kind", json!("running")),
        ("exitCode", json!(7)),
        ("signal", json!(9)),
        ("interrupted", json!(true)),
        ("isImage", json!(true)),
        ("noOutputExpected", json!(true)),
        ("stderr", json!("error")),
        ("error", json!("error")),
        ("isError", json!(true)),
        ("stdout", json!([])),
    ] {
        let dir = fixture_response(REPLACE);
        let mut event = post(&dir);
        event["tool_response"][key] = value;
        assert!(callback(&dir.0, event)
            .unwrap()
            .get("hookSpecificOutput")
            .is_none());
        assert!(!dir.0.join("called").exists());
        // A later success cannot replace an already consumed unsupported result.
        callback(&dir.0, post(&dir)).unwrap();
        assert!(!dir.0.join("called").exists());
    }
}

#[test]
fn failed_terminal_callback_reserves_occurrence_without_a_notification_route() {
    let dir = fixture_response(REPLACE);
    let mut failed = post(&dir);
    failed["hook_event_name"] = json!("PostToolUseFailure");
    failed["error"] = json!("failed");
    callback(&dir.0, failed).unwrap();
    assert!(callback(&dir.0, post(&dir))
        .unwrap()
        .get("hookSpecificOutput")
        .is_none());
    assert!(!dir.0.join("called").exists());
}

#[test]
fn foreign_context_changed_pin_and_missing_pretool_cannot_invoke_projector() {
    for case in 0..6 {
        let dir = fixture_response(REPLACE);
        let mut event = post(&dir);
        match case {
            0 => event["session_id"] = json!("other"),
            1 => event["tool_use_id"] = json!("other"),
            2 => event["agent_id"] = json!("child"),
            3 => event["cwd"] = json!("/other"),
            4 => event["tool_name"] = json!("Read"),
            _ => {
                let path = dir.0.join("binding.json");
                let mut binding: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                binding["prepared"]["config"]["tool_response"]["command"]["program_sha256"] =
                    json!("0".repeat(64));
                write(&path, &binding);
            }
        }
        let _ = callback(&dir.0, event);
        assert!(!dir.0.join("called").exists());
    }
}

#[test]
fn concurrent_duplicate_callbacks_dispatch_only_once() {
    let dir = fixture_response("import json,sys,pathlib,time\nr=json.load(sys.stdin)\npathlib.Path('entered').touch()\ntime.sleep(.1)\nprint(json.dumps({'format':1,'decision':'replace','text':'once','source_digest':r['source']['digest']}))");
    let root = dir.0.clone();
    let event = post(&dir);
    let worker = thread::spawn(move || callback(&root, event));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !dir.0.join("entered").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(callback(&dir.0, post(&dir))
        .unwrap()
        .get("hookSpecificOutput")
        .is_none());
    assert_eq!(
        worker.join().unwrap().unwrap()["hookSpecificOutput"]["updatedToolOutput"],
        "once"
    );
}

#[test]
fn timeout_and_cancellation_reap_projector_and_withhold_output() {
    for cancelled in [false, true] {
        let dir = fixture_response("import os,pathlib,time\npathlib.Path('pid').write_text(str(os.getpid()))\ntime.sleep(10)");
        let response = callback_with_cancellation(&dir.0, post(&dir), &|| {
            cancelled && dir.0.join("pid").exists()
        })
        .unwrap();
        assert!(
            response.get("systemMessage").is_some() && response.get("hookSpecificOutput").is_none()
        );
        let pid = fs::read_to_string(dir.0.join("pid")).unwrap();
        assert!(!Path::new("/proc").join(pid).exists());
    }
}

#[test]
fn reset_and_session_end_withhold_stale_candidates() {
    for reset in [false, true] {
        let dir = fixture_response("import os,pathlib,time\npathlib.Path('pid').write_text(str(os.getpid()))\ntime.sleep(10)");
        let root = dir.0.clone();
        let event = post(&dir);
        let worker = thread::spawn(move || callback(&root, event));
        let deadline = Instant::now() + Duration::from_secs(3);
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
        assert!(worker
            .join()
            .unwrap()
            .unwrap()
            .get("hookSpecificOutput")
            .is_none());
        let pid = fs::read_to_string(dir.0.join("pid")).unwrap();
        assert!(!Path::new("/proc").join(pid).exists());
    }
}

#[test]
fn independent_observer_failure_does_not_grant_or_remove_response_authority() {
    let dir = lifecycle_config("print('{\"decision\":\"deny\"}')", |config| {
        let route = config["notifications"]["tool.after"].clone();
        response_config(config, REPLACE);
        config["notifications"] = json!({"tool.after":route});
    });
    callback(&dir.0, native(&dir, "SessionStart", "s1", "unused")).unwrap();
    callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap();
    assert_eq!(
        callback(&dir.0, post(&dir)).unwrap()["hookSpecificOutput"]["updatedToolOutput"],
        "private candidate"
    );
    assert_eq!(query(&dir.0).unwrap()["failed"], 1);
}

#[test]
fn notification_work_and_projection_share_the_original_deadline() {
    let dir = lifecycle_config(
        "import time;time.sleep(.7);print('{\"format\":1,\"observed\":true}')",
        |config| {
            let mut first = config["notifications"]["tool.after"][0].clone();
            first["limits"]["timeout_ms"] = json!(1000);
            let mut second = first.clone();
            second["provider_id"] = json!("second");
            response_config(config, "import os,pathlib,time,json,sys\nr=json.load(sys.stdin)\npathlib.Path('pid').write_text(str(os.getpid()))\ntime.sleep(.9)\nprint(json.dumps({'format':1,'decision':'replace','text':'late','source_digest':r['source']['digest']}))");
            config["notifications"] = json!({"tool.after":[first,second]});
        },
    );
    callback(&dir.0, native(&dir, "SessionStart", "s1", "unused")).unwrap();
    callback(&dir.0, native(&dir, "PreToolUse", "s1", "t1")).unwrap();
    let start = Instant::now();
    assert!(callback(&dir.0, post(&dir))
        .unwrap()
        .get("hookSpecificOutput")
        .is_none());
    assert!(start.elapsed() < Duration::from_secs(4));
    if let Ok(pid) = fs::read_to_string(dir.0.join("pid")) {
        assert!(!Path::new("/proc").join(pid).exists());
    }
}

#[test]
fn admission_requires_explicit_loss_acceptance_and_optional_safety() {
    let dir = fixture_response(REPLACE);
    let path = dir.0.join("config.json");
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for case in 0..5 {
        let mut config = original.clone();
        match case {
            0 => config["required_safety"] = json!(true),
            1 => config["format"] = json!(1),
            2 => config["tool_response"]["accepted_reversibility"] = json!(["lossless"]),
            3 => config["tool_response"]
                .as_object_mut()
                .unwrap()
                .remove("accepted_reversibility")
                .map(|_| ())
                .unwrap(),
            _ => config["tool_response"]["command"]["limits"]["timeout_ms"] = json!(1001),
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
