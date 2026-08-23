use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

fn run_hook(input: &[u8], state: &TempDir) -> (std::process::ExitStatus, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-memory-cosh-hook"))
        .env(
            "ANOLISA_MEMORY_DB",
            state.path().join("private").join("memory-v1.sqlite3"),
        )
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
    let (status, response) = run_hook(input.to_string().as_bytes(), &state);

    assert!(status.success());
    assert_eq!(response["continue"], true);
    assert!(response.get("hookSpecificOutput").is_none());
}

#[test]
fn malformed_and_oversized_frames_fail_open() {
    let state = tempfile::tempdir().expect("state directory");
    for input in [b"not-json".to_vec(), vec![b'x'; 1024 * 1024 + 1]] {
        let (status, response) = run_hook(&input, &state);
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
    let (capture_status, capture_response) = run_hook(captured.to_string().as_bytes(), &state);
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
    let (recall_status, recall_response) = run_hook(recalled.to_string().as_bytes(), &state);

    assert!(recall_status.success());
    let context = recall_response["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("recalled context missing from {recall_response}"));
    assert!(context.contains("PIPESTATUS preserved every pipeline status"));
    assert!(context.contains("authority=\"Candidate\""));
}
