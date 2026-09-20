//! Explicit input authority, malformed output, cancellation and native mapping.

use super::{
    lifecycle::{lifecycle_config, native},
    *,
};
use aw_hook_cli::interactive::input_callback_with_cancellation as respond;

fn fixture_response(code: &str) -> Directory {
    lifecycle_config("print('{\"format\":1,\"observed\":true}')", |config| {
        let mut command = config["notifications"]["input.submit"][0].clone();
        command["provider_id"] = json!("input-policy");
        command["args"] = json!(["-c", code]);
        config["input_response"] = command;
        config["notifications"] = json!({});
    })
}

fn submit(dir: &Directory) -> Value {
    native(dir, "UserPromptSubmit", "s1", "unused")
}

#[test]
fn explicit_context_is_mapped_without_replacing_input_or_inventing_turns() {
    let dir = fixture_response("import json,sys,pathlib\nr=json.load(sys.stdin)\nassert r['scope']=='input.submit.respond' and r['format']==1\nassert r['event']['turn_id'] is None\nassert r['event']['payload']['prompt']=='same input is not a replay'\npathlib.Path('received').write_text('yes')\nprint('{\"format\":1,\"decision\":\"continue\",\"additional_context\":\"private context\"}')");
    let output = respond(&dir.0, submit(&dir), &|| false).unwrap();
    assert_eq!(
        output,
        json!({"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"private context"}})
    );
    assert!(dir.0.join("received").exists());
    for entry in fs::read_dir(dir.0.join("input-response-journal")).unwrap() {
        let text = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!text.contains("private context"));
        assert!(!text.contains("same input is not a replay"));
    }
    assert_eq!(
        query(&dir.0).unwrap()["input_response"],
        "experimental_native_response"
    );
    fs::remove_file(dir.0.join("received")).unwrap();
    query(&dir.0).unwrap();
    assert!(!dir.0.join("received").exists());
}

#[test]
fn continue_and_reject_map_only_their_own_effect() {
    for (output, expected) in [
        (json!({"format":1,"decision":"continue"}), json!({})),
        (
            json!({"format":1,"decision":"reject","reason":"policy declined"}),
            json!({"decision":"deny","reason":"policy declined"}),
        ),
    ] {
        let dir = fixture_response(&format!("print({:?})", output.to_string()));
        assert_eq!(respond(&dir.0, submit(&dir), &|| false).unwrap(), expected);
    }
}

#[test]
fn malformed_nonzero_and_overflow_responses_reject_without_context() {
    for code in [
        "print('not json')", "print('{}')", "print('{\"format\":1,\"observed\":true}')",
        "print('{\"format\":1,\"decision\":\"continue\",\"reason\":\"mixed\"}')",
        "print('{\"format\":1,\"decision\":\"reject\",\"reason\":\"no\",\"additional_context\":\"mixed\"}')",
        "print('{\"format\":1,\"decision\":\"continue\",\"hookSpecificOutput\":{}}')",
        "print('{\"format\":1,\"decision\":\"continue\",\"additional_context\":null}')",
        "print('{\"format\":1,\"format\":1,\"decision\":\"continue\"}')",
        "print('x'*2048)", "import sys;print('{\"format\":1,\"decision\":\"continue\"}');sys.exit(1)",
    ] {
        let dir = fixture_response(code);
        assert_eq!(respond(&dir.0, submit(&dir), &|| false).unwrap(), aw_adapters::qoder_input::unavailable(), "{code}");
    }
}

#[test]
fn changed_pin_or_missing_binding_never_runs_the_response_command() {
    let dir = fixture_response("import pathlib;pathlib.Path('called').touch()");
    let path = dir.0.join("binding.json");
    let mut binding: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    binding["prepared"]["config"]["input_response"]["program_sha256"] = json!("0".repeat(64));
    write(&path, &binding);
    assert_eq!(
        respond(&dir.0, submit(&dir), &|| false).unwrap()["decision"],
        "deny"
    );
    assert!(!dir.0.join("called").exists());
    fs::remove_file(path).unwrap();
    assert!(respond(&dir.0, submit(&dir), &|| false).is_err());
}

#[test]
fn wrong_event_and_unconfigured_entry_cannot_acquire_response_authority() {
    let dir = fixture_response("print('{\"format\":1,\"decision\":\"continue\"}')");
    assert!(respond(
        &dir.0,
        native(&dir, "SessionStart", "s1", "unused"),
        &|| false
    )
    .is_err());
    let path = dir.0.join("binding.json");
    let mut binding: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    binding["prepared"]["config"]
        .as_object_mut()
        .unwrap()
        .remove("input_response");
    write(&path, &binding);
    assert!(respond(&dir.0, submit(&dir), &|| false).is_err());
}

#[test]
fn foreign_session_child_and_wrong_workspace_inputs_never_run_policy() {
    let dir = fixture_response("import pathlib;pathlib.Path('called').touch()");
    callback(&dir.0, native(&dir, "SessionStart", "s1", "unused")).unwrap();
    for (key, value) in [
        ("session_id", "foreign"),
        ("agent_id", "child"),
        ("cwd", "/wrong"),
    ] {
        let mut event = submit(&dir);
        event[key] = json!(value);
        assert_eq!(
            respond(&dir.0, event, &|| false).unwrap()["decision"],
            "deny"
        );
        assert!(!dir.0.join("called").exists());
    }
}

#[test]
fn timeout_and_cancellation_reap_the_response_process() {
    let code = "import pathlib,os,time\npathlib.Path('pid').write_text(str(os.getpid()))\ntime.sleep(10)\nprint('{\"format\":1,\"decision\":\"continue\"}')";
    for cancel in [false, true] {
        let dir = fixture_response(code);
        let output = respond(&dir.0, submit(&dir), &|| {
            cancel && dir.0.join("pid").exists()
        })
        .unwrap();
        assert_eq!(output["decision"], "deny");
        let pid = fs::read_to_string(dir.0.join("pid")).unwrap();
        assert!(!Path::new("/proc").join(pid).exists());
    }
}

#[test]
fn reset_withholds_context_and_cancels_old_attachment() {
    let dir = fixture_response("import pathlib,os,time\npathlib.Path('pid').write_text(str(os.getpid()))\ntime.sleep(10)\nprint('{\"format\":1,\"decision\":\"continue\",\"additional_context\":\"stale\"}')");
    let root = dir.0.clone();
    let input = submit(&dir);
    let worker = thread::spawn(move || respond(&root, input, &|| false));
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dir.0.join("pid").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    callback(&dir.0, native(&dir, "SessionStart", "s2", "unused")).unwrap();
    assert_eq!(worker.join().unwrap().unwrap()["decision"], "deny");
    let pid = fs::read_to_string(dir.0.join("pid")).unwrap();
    assert!(!Path::new("/proc").join(pid).exists());
}

#[test]
fn notification_control_fields_do_not_override_explicit_response() {
    let dir = lifecycle_config(
        "print('{\"decision\":\"deny\",\"reason\":\"not authorized\"}')",
        |config| {
            let mut command = config["notifications"]["input.submit"][0].clone();
            command["args"] = json!(["-c", "print('{\"format\":1,\"decision\":\"continue\"}')"]);
            config["input_response"] = command;
        },
    );
    assert_eq!(respond(&dir.0, submit(&dir), &|| false).unwrap(), json!({}));
    assert_eq!(query(&dir.0).unwrap()["failed"], 1);
}
