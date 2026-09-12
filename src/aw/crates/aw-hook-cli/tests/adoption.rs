//! Observed Qoder history facts using retained candidates and synthetic native rows.

mod common;
use aw_hook_cli::{mark_returned, observe, project, query};
use common::{projection_config, Directory};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

fn prepared(directory: &Directory) -> (PathBuf, Value, Value, String) {
    let (config, native, candidate) = projection_config(directory, "ok");
    let result = project(
        serde_json::from_value(config.clone()).unwrap(),
        native.clone(),
    )
    .unwrap();
    assert!(result.projected);
    let initial = query(&result.record_path).unwrap();
    assert_eq!(initial["prepared"], true);
    assert_eq!(initial["returned"], false);
    assert_eq!(initial["observation_status"], "unverified");
    // Simulate the explicit embedding application's completed transport boundary.
    let mut transport = Vec::new();
    transport
        .write_all(&serde_json::to_vec(&result.response).unwrap())
        .unwrap();
    transport.flush().unwrap();
    mark_returned(&result.record_path).unwrap();
    (result.record_path, config, native, candidate)
}

fn rows(native: &Value, output: &str) -> [Value; 2] {
    [
        json!({"type":"assistant","sessionId":native["session_id"],"cwd":native["cwd"],"isSidechain":false,
        "message":{"content":[{"type":"tool_use","id":native["tool_use_id"],"name":"Bash","input":native["tool_input"]}]}}),
        json!({"type":"user","sessionId":native["session_id"],"cwd":native["cwd"],"isSidechain":false,
        "message":{"content":[{"type":"tool_result","tool_use_id":native["tool_use_id"],"is_error":false,"content":output}]}}),
    ]
}

fn append(path: &Path, rows: &[Value]) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    for row in rows {
        writeln!(file, "{row}").unwrap();
    }
    file.sync_all().unwrap();
}

#[test]
fn native_history_distinguishes_adopted_preserved_and_overridden() {
    for status in ["adopted", "preserved", "overridden"] {
        let dir = Directory::new();
        let (record, config, native, candidate) = prepared(&dir);
        let source = native["tool_response"]["stdout"].as_str().unwrap();
        let text = match status {
            "adopted" => candidate.as_str(),
            "preserved" => source,
            _ => "native result 再次变更🙂",
        };
        let history = Path::new(config["history_path"].as_str().unwrap());
        append(history, &rows(&native, text));
        assert_eq!(
            query(&record).unwrap()["observation_status"],
            "unverified",
            "query must not observe live history"
        );
        observe(&record).unwrap();
        let view = query(&record).unwrap();
        assert_eq!(view["observation_status"], status);
        assert_eq!(view["returned"], true);
        assert_eq!(view["proof_boundary"], "local_history");
        assert_eq!(view["observation"]["effective_bytes"], text.len());
        if status == "adopted" {
            assert_eq!(view["saved_bytes"], source.len() - candidate.len());
        } else {
            assert_eq!(view["saved_bytes"], 0);
        }
        assert!(
            observe(&record).is_err(),
            "an observation must be recorded only once"
        );
        fs::remove_file(history).unwrap();
        assert_eq!(
            query(&record).unwrap(),
            view,
            "recorded snapshots do not depend on live history"
        );
    }
}

#[test]
fn missing_history_result_remains_unverified_and_can_be_observed_later() {
    let dir = Directory::new();
    let (record, config, native, candidate) = prepared(&dir);
    observe(&record).unwrap();
    let missing = query(&record).unwrap();
    assert_eq!(missing["observation_status"], "unverified");
    assert!(missing["saved_bytes"].is_null());
    assert!(!record.with_extension("observation.json").exists());
    append(
        Path::new(config["history_path"].as_str().unwrap()),
        &rows(&native, &candidate),
    );
    observe(&record).unwrap();
    assert_eq!(query(&record).unwrap()["observation_status"], "adopted");
}

#[test]
fn ambiguous_or_mismatched_history_does_not_establish_adoption() {
    for failure in [
        "session",
        "cwd",
        "duplicate",
        "order",
        "error",
        "malformed",
        "anchor",
    ] {
        let dir = Directory::new();
        let (record, config, native, candidate) = prepared(&dir);
        let history = Path::new(config["history_path"].as_str().unwrap());
        let mut entries = rows(&native, &candidate).to_vec();
        match failure {
            "session" => entries[0]["sessionId"] = json!("wrong-session"),
            "cwd" => entries[1]["cwd"] = json!("/wrong-directory"),
            "duplicate" => entries.push(entries[1].clone()),
            "order" => entries.reverse(),
            "error" => entries[1]["message"]["content"][0]["is_error"] = json!(true),
            "anchor" => {
                entries[0]["message"]["content"][0]["input"]["command"] = json!("different command")
            }
            _ => {}
        }
        if failure == "malformed" {
            fs::write(history, b"{not-json}\n").unwrap();
        } else {
            append(history, &entries);
        }
        assert!(observe(&record).is_err(), "{failure}");
        assert_eq!(query(&record).unwrap()["observation_status"], "unverified");
        assert!(!record.with_extension("observation.json").exists());
    }
}

#[test]
fn query_rejects_changed_retained_material_or_missing_journal() {
    for failure in ["record", "journal", "observation"] {
        let dir = Directory::new();
        let (record, config, native, candidate) = prepared(&dir);
        append(
            Path::new(config["history_path"].as_str().unwrap()),
            &rows(&native, &candidate),
        );
        observe(&record).unwrap();
        match failure {
            "record" => {
                fs::write(&record, b"{}\n").unwrap();
            }
            "journal" => {
                fs::remove_dir_all(Path::new(config["hook"]["journal"].as_str().unwrap())).unwrap();
            }
            _ => {
                fs::write(record.with_extension("observation.json"), b"{}\n").unwrap();
            }
        }
        assert!(query(&record).is_err(), "{failure}");
    }
}

#[test]
fn missing_files_and_late_results_stay_unverified() {
    for missing_file in [true, false] {
        let dir = Directory::new();
        let (mut config, native, candidate) = projection_config(&dir, "ok");
        config["max_observation_delay_ms"] = json!(1);
        let result = project(
            serde_json::from_value(config.clone()).unwrap(),
            native.clone(),
        )
        .unwrap();
        mark_returned(&result.record_path).unwrap();
        let history = Path::new(config["history_path"].as_str().unwrap());
        if missing_file {
            fs::remove_file(history).unwrap();
        } else {
            append(history, &rows(&native, &candidate));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let view = observe(&result.record_path).unwrap();
        assert_eq!(view["observation_status"], "unverified");
        assert!(view["saved_bytes"].is_null());
        assert!(!result
            .record_path
            .with_extension("observation.json")
            .exists());
    }
}

#[test]
fn changed_history_prefix_invalidates_the_captured_anchor() {
    let dir = Directory::new();
    let (config, native, candidate) = projection_config(&dir, "ok");
    let history = Path::new(config["history_path"].as_str().unwrap());
    let mut entries = rows(&native, &candidate);
    entries[0]["metadata"] = json!("before");
    append(history, &entries[..1]);
    let result = project(serde_json::from_value(config.clone()).unwrap(), native).unwrap();
    mark_returned(&result.record_path).unwrap();
    append(history, &entries[1..]);
    let changed = fs::read_to_string(history)
        .unwrap()
        .replace("before", "edited");
    fs::write(history, changed).unwrap();
    assert!(observe(&result.record_path).is_err());
    assert_eq!(
        query(&result.record_path).unwrap()["observation_status"],
        "unverified"
    );
}

#[test]
fn no_candidate_does_not_claim_preservation_without_history() {
    for mode in ["no_savings", "security"] {
        let dir = Directory::new();
        let (config, native, _) = projection_config(&dir, mode);
        let result = project(
            serde_json::from_value(config.clone()).unwrap(),
            native.clone(),
        )
        .unwrap();
        assert!(!result.projected);
        let view = query(&result.record_path).unwrap();
        assert_eq!(view["prepared"], false);
        assert_eq!(view["observation_status"], "unverified");
        assert!(view["saved_bytes"].is_null());
        append(
            Path::new(config["history_path"].as_str().unwrap()),
            &rows(&native, native["tool_response"]["stdout"].as_str().unwrap()),
        );
        let observed = observe(&result.record_path).unwrap();
        if mode == "no_savings" {
            assert_eq!(observed["observation_status"], "preserved");
            assert_eq!(observed["saved_bytes"], 0);
        } else {
            assert_eq!(observed["observation_status"], "unverified");
            assert!(observed["saved_bytes"].is_null());
        }
    }
}
