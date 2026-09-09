use aw_adapters::{native::extract, Error, Host};
use serde_json::{json, Value};

const HOSTS: [Host; 6] = [
    Host::Qoder,
    Host::Codex,
    Host::QwenCode,
    Host::Hermes,
    Host::OpenClaw,
    Host::Cosh,
];

fn fixture(host: Host, pre: bool) -> (&'static str, Value) {
    let (event, mut payload) = match host {
        Host::Hermes => (
            if pre {
                "pre_tool_call"
            } else {
                "post_tool_call"
            },
            json!({"event":{"tool_name":"terminal","args":{"command":"  printf 'hi'\n"},"result":"hi\n"},"context":{"session_id":"session-a","tool_call_id":"call-a"}}),
        ),
        Host::OpenClaw => (
            if pre {
                "before_tool_call"
            } else {
                "after_tool_call"
            },
            json!({"event":{"toolName":"exec","params":{"command":"  printf 'hi'\n"},"result":"hi\n","toolCallId":"call-a"},"context":{"sessionId":"session-a"}}),
        ),
        _ => (
            if pre { "PreToolUse" } else { "PostToolUse" },
            json!({"hook_event_name":if pre {"PreToolUse"} else {"PostToolUse"},"tool_name":match host {Host::Qoder | Host::Codex => "Bash",_ => "run_shell_command"},"tool_input":{"command":"  printf 'hi'\n"},"tool_response":"hi\n","session_id":"session-a","tool_use_id":"call-a"}),
        ),
    };
    if matches!(host, Host::Cosh) {
        payload["tool_response"] = json!({"llmContent":"hi\n","returnDisplay":"display-only"});
    } else if matches!(host, Host::Codex) {
        payload["turn_id"] = json!("turn-a");
    } else if matches!(host, Host::OpenClaw) {
        payload["context"]["runId"] = json!("turn-a");
    }
    (event, payload)
}

fn event_mut(host: Host, payload: &mut Value) -> &mut Value {
    if matches!(host, Host::Hermes | Host::OpenClaw) {
        &mut payload["event"]
    } else {
        payload
    }
}

#[test]
fn six_hosts_preserve_commands_results_and_observed_ids() {
    for host in HOSTS {
        for pre in [true, false] {
            let (event, payload) = fixture(host, pre);
            let before = payload.clone();
            let extracted = extract(host, event, &payload).unwrap();
            assert_eq!(extracted.text, if pre { "  printf 'hi'\n" } else { "hi\n" });
            assert_eq!(extracted.session_id.as_deref(), Some("session-a"));
            assert_eq!(extracted.tool_use_id.as_deref(), Some("call-a"));
            assert_eq!(
                extracted.turn_id.as_deref(),
                if matches!(host, Host::Codex | Host::OpenClaw) {
                    Some("turn-a")
                } else {
                    None
                }
            );
            assert_eq!(
                extracted.origin,
                if pre { "unspecified" } else { "command_output" }
            );
            assert_eq!(before, payload);
        }
    }
}

#[test]
fn six_hosts_preserve_empty_and_whitespace_results() {
    for host in HOSTS {
        for text in ["", " ", "\n\t\r\n"] {
            let (event, mut payload) = fixture(host, false);
            let key = if matches!(host, Host::Hermes | Host::OpenClaw) {
                "result"
            } else {
                "tool_response"
            };
            if matches!(host, Host::Cosh) {
                payload[key]["llmContent"] = json!(text);
            } else {
                event_mut(host, &mut payload)[key] = json!(text);
            }
            assert_eq!(extract(host, event, &payload).unwrap().text, text);
        }
    }
}

#[test]
fn pre_tool_rejects_non_shell_names_even_with_command_parameter() {
    for host in HOSTS {
        let (event, mut payload) = fixture(host, true);
        let key = if matches!(host, Host::OpenClaw) {
            "toolName"
        } else {
            "tool_name"
        };
        event_mut(host, &mut payload)[key] = json!("write_file");
        assert!(matches!(
            extract(host, event, &payload),
            Err(Error::UnsupportedPayload(_))
        ));
    }
}

#[test]
fn non_shell_output_is_not_claimed_as_command_output() {
    for host in HOSTS {
        let (event, mut payload) = fixture(host, false);
        let key = if matches!(host, Host::OpenClaw) {
            "toolName"
        } else {
            "tool_name"
        };
        event_mut(host, &mut payload)[key] = json!("read_file");
        assert_eq!(
            extract(host, event, &payload).unwrap().origin,
            "unspecified"
        );
    }
}

#[test]
fn six_hosts_reject_missing_non_text_and_multiblock_results() {
    for host in HOSTS {
        for bad in [
            Value::Null,
            json!(7),
            json!({"content":"text"}),
            json!({"type":"image","data":"synthetic-image"}),
            json!([{"type":"text","text":"one"},{"type":"text","text":"two"}]),
            json!({"content":[{"type":"text","text":"one"},{"type":"image","data":"two"}]}),
            json!({"output":"failure","is_error":true}),
        ] {
            let (event, mut payload) = fixture(host, false);
            let key = if matches!(host, Host::Hermes | Host::OpenClaw) {
                "result"
            } else {
                "tool_response"
            };
            event_mut(host, &mut payload)[key] = bad;
            assert!(matches!(
                extract(host, event, &payload),
                Err(Error::UnsupportedPayload(_))
            ));
        }
    }
}

#[test]
fn malformed_commands_are_not_stringified_or_coerced() {
    for host in HOSTS {
        for bad in [
            Value::Null,
            json!([]),
            json!({"command":7}),
            json!({"command":"  "}),
        ] {
            let (event, mut payload) = fixture(host, true);
            let key = match host {
                Host::Hermes => "args",
                Host::OpenClaw => "params",
                _ => "tool_input",
            };
            event_mut(host, &mut payload)[key] = bad;
            assert!(matches!(
                extract(host, event, &payload),
                Err(Error::UnsupportedPayload(_))
            ));
        }
    }
}

#[test]
fn encoded_input_is_supported_only_where_existing_hooks_decode_it() {
    for host in HOSTS {
        let (event, mut payload) = fixture(host, true);
        let key = match host {
            Host::Hermes => "args",
            Host::OpenClaw => "params",
            _ => "tool_input",
        };
        event_mut(host, &mut payload)[key] = json!(r#"{"command":"echo hello"}"#);
        let result = extract(host, event, &payload);
        if matches!(host, Host::Qoder | Host::QwenCode) {
            assert_eq!(result.unwrap().text, "echo hello");
            payload["tool_input"] = json!("{malformed");
            assert!(extract(host, event, &payload).is_err());
        } else {
            assert!(result.is_err());
        }
    }
}

#[test]
fn optional_ids_do_not_infer_alternate_session_or_call_fields() {
    for host in HOSTS {
        let (event, mut payload) = fixture(host, true);
        match host {
            Host::Hermes => {
                payload["context"] = json!({"sessionId":"ignored","toolUseId":"ignored"})
            }
            Host::OpenClaw => {
                payload["context"] = json!({"sessionKey":"ignored","callId":"ignored"});
                payload["event"]
                    .as_object_mut()
                    .unwrap()
                    .remove("toolCallId");
            }
            _ => {
                let obj = payload.as_object_mut().unwrap();
                obj.remove("session_id");
                obj.remove("tool_use_id");
                obj.insert("sessionId".into(), json!("ignored"));
                obj.insert("tool_call_id".into(), json!("ignored"));
            }
        }
        let extracted = extract(host, event, &payload).unwrap();
        assert!(extracted.session_id.is_none());
        assert!(extracted.tool_use_id.is_none());
    }
}

#[test]
fn openclaw_conflicting_callback_ids_are_rejected() {
    let (event, mut payload) = fixture(Host::OpenClaw, true);
    payload["context"]["toolCallId"] = json!("other-call");
    assert!(matches!(
        extract(Host::OpenClaw, event, &payload),
        Err(Error::IdentityMismatch("toolCallId"))
    ));
    payload["context"]["toolCallId"] = json!("call-a");
    assert!(extract(Host::OpenClaw, event, &payload).is_ok());
    payload["event"]["sessionId"] = json!("other-session");
    assert!(matches!(
        extract(Host::OpenClaw, event, &payload),
        Err(Error::IdentityMismatch("sessionId"))
    ));
}

#[test]
fn malformed_ids_are_rejected_in_all_six_hosts() {
    for host in HOSTS {
        let (event, mut payload) = fixture(host, true);
        match host {
            Host::Hermes => payload["context"]["session_id"] = json!(42),
            Host::OpenClaw => payload["context"]["sessionId"] = json!(42),
            _ => payload["session_id"] = json!(42),
        }
        assert!(extract(host, event, &payload).is_err());
    }
}

#[test]
fn explicit_callback_errors_cannot_be_lost_in_successful_text() {
    for host in [Host::Hermes, Host::OpenClaw] {
        let (event, mut payload) = fixture(host, false);
        let container = if matches!(host, Host::Hermes) {
            "context"
        } else {
            "event"
        };
        payload[container]["error"] = json!("tool execution failed");
        assert!(matches!(
            extract(host, event, &payload),
            Err(Error::UnsupportedPayload("failed tool result"))
        ));
    }
}

#[test]
fn json_hook_failures_cannot_be_lost_in_result_text() {
    for host in [Host::Qoder, Host::Codex, Host::QwenCode, Host::Cosh] {
        for signal in [
            json!({"is_error":true}),
            json!({"status":"interrupted"}),
            json!({"status":"denied"}),
            json!({"status":"INTERRUPTED"}),
            json!({"status":"DENIED"}),
        ] {
            let (event, mut payload) = fixture(host, false);
            payload
                .as_object_mut()
                .unwrap()
                .extend(signal.as_object().unwrap().clone());
            assert!(matches!(
                extract(host, event, &payload),
                Err(Error::UnsupportedPayload("failed tool result"))
            ));
        }
        let (event, mut payload) = fixture(host, false);
        payload["is_error"] = json!(false);
        payload["status"] = json!("success");
        assert_eq!(extract(host, event, &payload).unwrap().text, "hi\n");
    }
}

#[test]
fn json_hook_failure_signal_types_are_not_coerced() {
    for host in [Host::Qoder, Host::Codex, Host::QwenCode, Host::Cosh] {
        for bad in [Value::Null, json!("false"), json!(0), json!([]), json!({})] {
            let (event, mut payload) = fixture(host, false);
            payload["is_error"] = bad;
            assert!(matches!(
                extract(host, event, &payload),
                Err(Error::UnsupportedPayload("is_error"))
            ));
        }
        for bad in [Value::Null, json!(false), json!(0), json!([]), json!({})] {
            let (event, mut payload) = fixture(host, false);
            payload["status"] = bad;
            assert!(matches!(
                extract(host, event, &payload),
                Err(Error::UnsupportedPayload("status"))
            ));
        }
    }
}

#[test]
fn event_identity_and_local_callback_containers_are_enforced() {
    for host in HOSTS {
        let (event, mut payload) = fixture(host, true);
        assert!(extract(host, "PostToolUseFailure", &payload).is_err());
        assert!(extract(host, event, &json!([])).is_err());
        if matches!(host, Host::Hermes | Host::OpenClaw) {
            let flattened = payload["event"].clone();
            assert!(extract(host, event, &flattened).is_err());
            payload["context"] = Value::Null;
        } else {
            payload["hook_event_name"] = json!("PostToolUse");
        }
        assert!(extract(host, event, &payload).is_err());
    }
}

#[test]
fn cosh_reads_only_the_model_visible_slot() {
    let (event, mut payload) = fixture(Host::Cosh, false);
    payload["tool_response"] =
        json!({"llmContent":"model-visible","returnDisplay":{"text":"display-only"}});
    let before = payload.clone();
    assert_eq!(
        extract(Host::Cosh, event, &payload).unwrap().text,
        "model-visible"
    );
    assert_eq!(payload, before);
    for invalid in [
        json!("legacy flattened output"),
        json!({"returnDisplay":"display-only"}),
        json!({"llmContent":null,"returnDisplay":"display-only"}),
        json!({"llmContent":[{"type":"text","text":"one"}]}),
    ] {
        payload["tool_response"] = invalid;
        assert!(extract(Host::Cosh, event, &payload).is_err());
    }
}

#[test]
fn native_turn_observations_are_exact_and_consistent() {
    let (event, mut payload) = fixture(Host::OpenClaw, true);
    payload["event"]["runId"] = json!("other-run");
    assert!(matches!(
        extract(Host::OpenClaw, event, &payload),
        Err(Error::IdentityMismatch("runId"))
    ));
    payload["event"]["runId"] = json!("turn-a");
    assert_eq!(
        extract(Host::OpenClaw, event, &payload)
            .unwrap()
            .turn_id
            .as_deref(),
        Some("turn-a")
    );
    payload["context"]["runId"] = json!(7);
    assert!(matches!(
        extract(Host::OpenClaw, event, &payload),
        Err(Error::UnsupportedPayload("runId"))
    ));

    let (event, mut payload) = fixture(Host::Codex, true);
    payload["turn_id"] = json!(7);
    assert!(matches!(
        extract(Host::Codex, event, &payload),
        Err(Error::UnsupportedPayload("turn_id"))
    ));
    payload.as_object_mut().unwrap().remove("turn_id");
    payload["run_id"] = json!("unrelated-run");
    assert!(extract(Host::Codex, event, &payload)
        .unwrap()
        .turn_id
        .is_none());

    for host in [Host::Qoder, Host::QwenCode, Host::Cosh, Host::Hermes] {
        let (event, mut payload) = fixture(host, true);
        let context = if matches!(host, Host::Hermes) {
            &mut payload["context"]
        } else {
            &mut payload
        };
        context["run_id"] = json!("unrelated-run");
        context["turn_id"] = json!("unverified-turn");
        assert!(extract(host, event, &payload).unwrap().turn_id.is_none());
    }
}

#[test]
fn cosh_retains_its_explicit_shell_alias() {
    let (event, mut payload) = fixture(Host::Cosh, true);
    payload["tool_name"] = json!("shell");
    assert!(extract(Host::Cosh, event, &payload).is_ok());
}
