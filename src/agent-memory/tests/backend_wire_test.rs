use std::io::Write;
use std::process::{Command, Stdio};

use agent_memory::protocol::{
    IdentityContext, MEMORY_PROTOCOL_VERSION, MemoryRequest, MemoryRequestEnvelope,
    MemoryWireResponse,
};

#[test]
fn jsonl_backend_preserves_request_ids_and_survives_invalid_frames() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-memory-backend"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("backend binary starts");
    let request = MemoryRequestEnvelope {
        protocol_version: MEMORY_PROTOCOL_VERSION,
        request_id: "request-1".to_string(),
        trace_id: "trace-1".to_string(),
        run_id: None,
        task_id: None,
        turn_id: None,
        deadline_at_ms: None,
        identity: IdentityContext {
            tenant_id: None,
            team_id: None,
            user_id: "unix:1000".to_string(),
            agent_id: "cosh-ng".to_string(),
            session_id: "session-1".to_string(),
            workspace_id: "workspace-1".to_string(),
        },
        request: MemoryRequest::Negotiate { required: vec![] },
    };
    {
        let stdin = child.stdin.as_mut().expect("stdin is piped");
        serde_json::to_writer(&mut *stdin, &request).expect("request serializes");
        stdin
            .write_all(b"\n{invalid-json}\n")
            .expect("frames write");
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("backend exits");
    assert!(output.status.success());
    let lines: Vec<&[u8]> = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 2);
    assert!(matches!(
        serde_json::from_slice::<MemoryWireResponse>(lines[0]).expect("first response is JSON"),
        MemoryWireResponse::Ok { request_id, .. } if request_id == "request-1"
    ));
    assert!(matches!(
        serde_json::from_slice::<MemoryWireResponse>(lines[1]).expect("error response is JSON"),
        MemoryWireResponse::Error { request_id, .. } if request_id == "unknown"
    ));
}

#[test]
fn schema_command_outputs_machine_readable_bundle() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-memory-backend"))
        .arg("--schema")
        .output()
        .expect("schema command runs");
    assert!(output.status.success());
    let schema: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("schema output is JSON");
    assert_eq!(schema["version"], MEMORY_PROTOCOL_VERSION);
    assert!(schema["request"].is_object());
    assert!(schema["response"].is_object());
}

#[test]
fn jsonl_backend_classifies_unknown_operation_and_preserves_request_id() {
    let input = serde_json::json!({
        "protocol_version": 1,
        "request_id": "future-operation-1",
        "trace_id": "trace-future-operation-1",
        "run_id": null,
        "task_id": null,
        "turn_id": null,
        "deadline_at_ms": null,
        "identity": {
            "tenant_id": null,
            "team_id": null,
            "user_id": "unix:1000",
            "agent_id": "cosh-ng",
            "session_id": "session-1",
            "workspace_id": "workspace-1"
        },
        "request": {"operation": "future_operation", "input": {}}
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-memory-backend"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("backend starts");
    let stdin = child.stdin.as_mut().expect("stdin is piped");
    serde_json::to_writer(&mut *stdin, &input).expect("request serializes");
    stdin.write_all(b"\n").expect("request delimiter writes");
    drop(child.stdin.take());
    let output = child
        .wait_with_output()
        .expect("backend handles future operation");
    assert!(output.status.success());
    assert!(matches!(
        serde_json::from_slice::<MemoryWireResponse>(&output.stdout)
            .expect("response is protocol JSON"),
        MemoryWireResponse::Error {
            request_id,
            error: agent_memory::protocol::ProtocolError {
                code: agent_memory::protocol::ProtocolErrorCode::UnsupportedCapability,
                ..
            },
            ..
        } if request_id == "future-operation-1"
    ));
}

#[test]
fn jsonl_backend_bounds_frames_and_recovers_at_newline() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-memory-backend"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("backend binary starts");
    let request = MemoryRequestEnvelope {
        protocol_version: MEMORY_PROTOCOL_VERSION,
        request_id: "after-oversize".to_string(),
        trace_id: "trace-after-oversize".to_string(),
        run_id: None,
        task_id: None,
        turn_id: None,
        deadline_at_ms: None,
        identity: IdentityContext {
            tenant_id: None,
            team_id: None,
            user_id: "unix:1000".to_string(),
            agent_id: "cosh-ng".to_string(),
            session_id: "session-1".to_string(),
            workspace_id: "workspace-1".to_string(),
        },
        request: MemoryRequest::Negotiate { required: vec![] },
    };
    {
        let stdin = child.stdin.as_mut().expect("stdin is piped");
        stdin
            .write_all(&vec![b'x'; 1024 * 1024 + 1])
            .expect("oversized frame writes");
        stdin.write_all(b"\n").expect("frame delimiter writes");
        serde_json::to_writer(&mut *stdin, &request).expect("request serializes");
        stdin.write_all(b"\n").expect("request delimiter writes");
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("backend exits");
    assert!(output.status.success());
    let responses: Vec<MemoryWireResponse> = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("response is JSON"))
        .collect();
    assert_eq!(responses.len(), 2);
    assert!(matches!(
        responses[0],
        MemoryWireResponse::Error {
            error: agent_memory::protocol::ProtocolError {
                code: agent_memory::protocol::ProtocolErrorCode::ResourceExhausted,
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        responses[1],
        MemoryWireResponse::Ok { ref request_id, .. } if request_id == "after-oversize"
    ));
}
