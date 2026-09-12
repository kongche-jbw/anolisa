//! Native history fixtures exercise capture, append observation and offline replay.

use super::*;
use serde_json::json;
use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    directory: PathBuf,
    settings: ProjectionSettings,
    payload: Value,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!(
                "history-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("session-1.jsonl");
        fs::write(&path, []).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let config = json!({"provider_id":"test","provider_version":"test","program":"/test",
            "cwd":directory,"args":[],"environment":{},"program_sha256":"a".repeat(64),
            "pins":[],"limits":{"timeout_ms":1000,"input_bytes":1024,"output_bytes":1024,"stderr_bytes":1024}});
        let settings = serde_json::from_value(json!({"hook":{"runtime":{},
            "scope":{"session_id":"session-1","tool_use_id":"tool-1","turn_id":"turn-1"},
            "agent_pid":1,"agent_start_ticks":1,"qoder_single_turn_id":"turn-1",
            "journal":directory.join("journal"),"provider":config,"include_low_confidence":false},
            "tokenless":config,"record_directory":directory.join("records"),"history_path":path,
            "history_profile":PROFILE,"retention":"source_and_candidate","max_observation_delay_ms":1000,
            "accepted_reversibility":["unrecoverable"],"allow_text_reencoding":false})).unwrap();
        let payload = json!({"session_id":"session-1","tool_use_id":"tool-1","cwd":directory,
            "tool_name":"Bash","tool_input":{"command":"printf source"},"transcript_path":path});
        Self {
            directory,
            settings,
            payload,
        }
    }

    fn rows(&self) -> [Value; 2] {
        let row = json!({"sessionId":"session-1","cwd":self.directory,"isSidechain":false});
        let mut tool = row.clone();
        tool["type"] = json!("assistant");
        tool["message"] = json!({"content":[{"type":"tool_use","id":"tool-1",
            "name":"Bash","input":{"command":"printf source"}}]});
        let mut result = row;
        result["type"] = json!("user");
        result["message"] = json!({"content":[{"type":"tool_result","tool_use_id":"tool-1",
            "is_error":false,"content":"候选🙂\n\t"}]});
        [tool, result]
    }

    fn append(&self, rows: &[Value]) {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.settings.history_path)
            .unwrap();
        for row in rows {
            writeln!(file, "{row}").unwrap();
        }
    }

    fn capture(&self) -> Binding {
        Binding::capture(&self.settings, &self.payload).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn appended_result_has_exact_text_and_replays_after_history_is_removed() {
    let f = Fixture::new();
    let binding = f.capture();
    assert!(binding.observe().unwrap().is_none());
    f.append(&f.rows());
    let proof = binding.observe().unwrap().unwrap();
    let encoded = serde_json::to_value(&proof).unwrap();
    let proof: Proof = serde_json::from_value(encoded).unwrap();
    fs::remove_file(&f.settings.history_path).unwrap();
    assert_eq!(binding.validate(&proof).unwrap(), "候选🙂\n\t");
    assert!(binding.observe().unwrap().is_none());
    assert!(Binding::capture(&f.settings, &f.payload).is_err());
}

#[test]
fn existing_tool_use_can_be_bound_but_existing_result_cannot() {
    let f = Fixture::new();
    let rows = f.rows();
    f.append(&rows[..1]);
    let binding = f.capture();
    assert!(binding.observe().unwrap().is_none());
    f.append(&rows[1..]);
    assert!(binding.observe().unwrap().is_some());
    assert!(Binding::capture(&f.settings, &f.payload).is_err());
}

#[test]
fn native_success_may_omit_is_error_but_not_sidechain_identity() {
    let f = Fixture::new();
    let binding = f.capture();
    let mut rows = f.rows();
    rows[1]["message"]["content"][0]
        .as_object_mut()
        .unwrap()
        .remove("is_error");
    f.append(&rows);
    let proof = binding.observe().unwrap().unwrap();
    assert_eq!(binding.validate(&proof).unwrap(), "候选🙂\n\t");
}

#[test]
fn capture_rejects_ambiguous_encoded_input_and_accepts_equivalent_objects() {
    let mut f = Fixture::new();
    f.payload["tool_input"] = json!(r#"{"command":"printf source"}"#);
    let binding = f.capture();
    f.append(&f.rows());
    assert!(binding.observe().unwrap().is_some());
    f.payload["tool_input"] = json!(r#"{"command":"first","command":"printf source"}"#);
    assert!(Binding::capture(&f.settings, &f.payload).is_err());
}

#[test]
fn changed_prefix_truncation_and_replacement_inode_are_rejected() {
    for mode in ["prefix", "truncate", "inode"] {
        let f = Fixture::new();
        f.append(&f.rows()[..1]);
        let binding = f.capture();
        match mode {
            "prefix" => {
                let bytes = fs::read_to_string(&f.settings.history_path)
                    .unwrap()
                    .replace("printf source", "printf forged");
                fs::write(&f.settings.history_path, bytes).unwrap();
            }
            "truncate" => fs::write(&f.settings.history_path, []).unwrap(),
            _ => {
                fs::rename(&f.settings.history_path, f.directory.join("original")).unwrap();
                fs::write(&f.settings.history_path, []).unwrap();
                f.append(&f.rows()[..1]);
            }
        }
        f.append(&f.rows()[1..]);
        assert!(binding.observe().is_err(), "{mode}");
    }
}

#[test]
fn wrong_result_binding_error_and_nontext_content_are_rejected() {
    for (field, value) in [
        ("sessionId", json!("other")),
        ("cwd", json!("/other")),
        ("isSidechain", json!(true)),
        ("isSidechain", Value::Null),
        ("type", json!("assistant")),
        ("is_error", json!(true)),
        ("is_error", Value::Null),
        ("content", json!([])),
    ] {
        let f = Fixture::new();
        let binding = f.capture();
        let mut rows = f.rows();
        if field == "is_error" || field == "content" {
            rows[1]["message"]["content"][0][field] = value;
        } else {
            rows[1][field] = value;
        }
        f.append(&rows);
        assert!(binding.observe().is_err(), "{field}");
    }
}

#[test]
fn duplicate_out_of_order_or_mismatched_tool_records_are_rejected() {
    for mode in [
        "duplicate_tool",
        "duplicate_result",
        "reverse",
        "input",
        "name",
    ] {
        let f = Fixture::new();
        let binding = f.capture();
        let mut rows = f.rows().to_vec();
        match mode {
            "duplicate_tool" => rows.insert(1, rows[0].clone()),
            "duplicate_result" => rows.push(rows[1].clone()),
            "reverse" => rows.reverse(),
            "input" => rows[0]["message"]["content"][0]["input"] = json!({"command":"other"}),
            _ => rows[0]["message"]["content"][0]["name"] = json!("Read"),
        }
        f.append(&rows);
        assert!(binding.observe().is_err(), "{mode}");
    }
}

#[test]
fn damaged_partial_or_duplicate_key_jsonl_is_never_missing_evidence() {
    for bytes in [
        b"{invalid}\n".as_slice(),
        b"{}",
        b"{\"type\":1,\"type\":2}\n",
    ] {
        let f = Fixture::new();
        let binding = f.capture();
        fs::write(&f.settings.history_path, bytes).unwrap();
        assert!(binding.observe().is_err());
        assert!(Binding::capture(&f.settings, &f.payload).is_err());
    }
}

#[test]
fn history_open_rejects_symlink_and_oversized_sparse_file() {
    let f = Fixture::new();
    fs::rename(&f.settings.history_path, f.directory.join("original")).unwrap();
    symlink(f.directory.join("original"), &f.settings.history_path).unwrap();
    assert!(Binding::capture(&f.settings, &f.payload).is_err());
    fs::remove_file(&f.settings.history_path).unwrap();
    fs::File::create(&f.settings.history_path)
        .unwrap()
        .set_len(MAX_HISTORY_BYTES as u64 + 1)
        .unwrap();
    assert!(Binding::capture(&f.settings, &f.payload).is_err());
}

#[test]
fn replay_rejects_tampered_digest_identity_offsets_and_clock() {
    let f = Fixture::new();
    let binding = f.capture();
    f.append(&f.rows());
    let proof = binding.observe().unwrap().unwrap();
    for mode in ["digest", "binding", "offset", "clock", "line"] {
        let mut proof = proof.clone();
        match mode {
            "digest" => proof.result.digest = "0".repeat(64),
            "binding" => proof.binding_digest = "0".repeat(64),
            "offset" => proof.result.offset = 0,
            "clock" => proof.observed_at_ms = 0,
            _ => proof.result.line = proof.tool.line,
        }
        assert!(binding.validate(&proof).is_err(), "{mode}");
    }
}
