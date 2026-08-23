use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

fn run_hook(
    input: &[u8],
    state: &TempDir,
    mant: Option<&Path>,
) -> (std::process::ExitStatus, Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-memory-cosh-hook"));
    command.env(
        "ANOLISA_MEMORY_DB",
        state.path().join("private").join("memory-v1.sqlite3"),
    );
    if let Some(mant) = mant {
        command
            .env("ANOLISA_MEMORY_MANT", "on")
            .env("ANOLISA_MANT_PATH", mant)
            .env("ANOLISA_MEMORY_MANT_DOCUMENT", "bash");
    } else {
        command.env("ANOLISA_MEMORY_MANT", "off");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input)
        .expect("write hook input");
    let output = child.wait_with_output().expect("wait for hook");
    assert!(output.stderr.is_empty());
    let response = serde_json::from_slice(&output.stdout).expect("hook JSON response");
    (output.status, response)
}

#[test]
fn one_shot_hook_accepts_current_cosh_shape() {
    let state = tempfile::tempdir().expect("state directory");
    let input = serde_json::json!({
        "session_id": "session-1",
        "run_id": "run-1",
        "cwd": env!("CARGO_MANIFEST_DIR"),
        "hook_event_name": "UserPromptSubmit",
        "timestamp": "2026-08-23T00:00:00Z",
        "transcript_path": "/unused/cosh-transcript.jsonl",
        "prompt": "continue the task"
    });
    let (status, response) = run_hook(input.to_string().as_bytes(), &state, None);

    assert!(status.success());
    assert_eq!(response["continue"], true);
    assert!(response.get("hookSpecificOutput").is_none());
}

#[test]
fn malformed_and_oversized_frames_fail_open() {
    let state = tempfile::tempdir().expect("state directory");
    for input in [b"not-json".to_vec(), vec![b'x'; 1024 * 1024 + 1]] {
        let (status, response) = run_hook(&input, &state, None);
        assert!(status.success());
        assert_eq!(response, serde_json::json!({"continue": true}));
    }
}

#[test]
fn tool_evidence_is_recalled_by_a_new_hook_process() {
    let state = tempfile::tempdir().expect("state directory");
    let captured = serde_json::json!({
        "session_id": "capture-session",
        "run_id": "capture-run",
        "cwd": state.path(),
        "hook_event_name": "PostToolUse",
        "timestamp": "2026-08-24T00:00:00Z",
        "transcript_path": "/unused/cosh-transcript.jsonl",
        "tool_use_id": "call-1",
        "tool_name": "shell",
        "tool_input": {"command": "check pipeline status"},
        "tool_response": "PIPESTATUS preserved every pipeline status"
    });
    let (capture_status, capture_response) =
        run_hook(captured.to_string().as_bytes(), &state, None);
    assert!(capture_status.success());
    assert_eq!(capture_response, serde_json::json!({"continue": true}));

    let recalled = serde_json::json!({
        "session_id": "recall-session",
        "run_id": "recall-run",
        "cwd": state.path(),
        "hook_event_name": "UserPromptSubmit",
        "timestamp": "2026-08-24T00:00:01Z",
        "transcript_path": "/unused/cosh-transcript.jsonl",
        "prompt": "How did PIPESTATUS preserve pipeline status?"
    });
    let (recall_status, recall_response) = run_hook(recalled.to_string().as_bytes(), &state, None);

    assert!(recall_status.success());
    let context = recall_response["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("recalled context missing from {recall_response}"));
    assert!(context.contains("PIPESTATUS preserved every pipeline status"));
    assert!(context.contains("authority=\"Candidate\""));
}

#[test]
fn optional_mant_provider_adds_bounded_candidate_knowledge() {
    let state = tempfile::tempdir().expect("state directory");
    let mant = state.path().join("fake-mant");
    fs::write(
        &mant,
        r#"#!/bin/sh
if [ "$1" = "--protocol-version" ]; then
  printf '%s\n' '{"protocol":"mant.cli/v0.9","nativeApiVersion":"0.9","requestSchema":"mant.request/v0.9","excerptSchema":"mant.excerpt/v0.9","searchSchema":"mant.search/v0.9"}'
  exit 0
fi
cat >/dev/null
printf '%s\n' '{"schema":"mant.search/v0.9","label":"Bash manual","query":{"pattern":"PIPESTATUS","syntax":"literal","case":"insensitive","scope":"visible","word":false,"contextLines":1,"limit":4,"offset":0},"render":{"schema":"mant.markdown/v1","format":"markdown","scope":"full","lineBase":1,"columnBase":1,"lineCount":1},"total":1,"returned":1,"offset":0,"truncated":false,"matches":[{"ordinal":1,"outline":{"node":{"kind":"document-section","path":"1","id":"pipestatus","title":"PIPESTATUS"}},"occurrences":[{"matchedText":"PIPESTATUS","markdown":{"startByte":0,"endByte":10,"startLine":1,"startColumn":1,"endLine":1,"endColumn":11},"lineRanges":[{"line":1,"startByte":0,"endByte":10}]}],"occurrenceCount":1,"occurrencesTruncated":false,"preview":"PIPESTATUS is an array of pipeline exit statuses.","context":[{"line":1,"text":"PIPESTATUS is an array of pipeline exit statuses.","matched":true}]}]}'
"#,
    )
    .expect("fake ManT script");
    let mut permissions = fs::metadata(&mant)
        .expect("fake ManT metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&mant, permissions).expect("fake ManT executable");
    let request = serde_json::json!({
        "session_id": "mant-session",
        "run_id": "mant-run",
        "cwd": state.path(),
        "hook_event_name": "UserPromptSubmit",
        "timestamp": "2026-08-24T00:00:00Z",
        "transcript_path": "/unused/cosh-transcript.jsonl",
        "prompt": "How does PIPESTATUS work?"
    });

    let (status, response) = run_hook(request.to_string().as_bytes(), &state, Some(&mant));
    assert!(status.success());
    let context = response["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("ManT context missing from {response}"));
    assert!(context.contains("PIPESTATUS is an array"));
    assert!(context.contains("kind=\"Knowledge\""));
    assert!(context.contains("authority=\"Candidate\""));
    assert!(context.contains("knowledge://mant/bash/"));
}
