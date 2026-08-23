use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

fn run_hook(input: &[u8]) -> (std::process::ExitStatus, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-memory-cosh-hook"))
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
    let input = serde_json::json!({
        "session_id": "session-1",
        "run_id": "run-1",
        "cwd": env!("CARGO_MANIFEST_DIR"),
        "hook_event_name": "UserPromptSubmit",
        "timestamp": "2026-08-23T00:00:00Z",
        "transcript_path": "/unused/cosh-transcript.jsonl",
        "prompt": "continue the task"
    });
    let (status, response) = run_hook(input.to_string().as_bytes());

    assert!(status.success());
    assert_eq!(response["continue"], true);
    assert!(response.get("hookSpecificOutput").is_none());
}

#[test]
fn malformed_and_oversized_frames_fail_open() {
    for input in [b"not-json".to_vec(), vec![b'x'; 1024 * 1024 + 1]] {
        let (status, response) = run_hook(&input);
        assert!(status.success());
        assert_eq!(response, serde_json::json!({"continue": true}));
    }
}
