use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

struct TestEnvironment {
    root: tempfile::TempDir,
    database: PathBuf,
    workspace: PathBuf,
    bin: PathBuf,
}

impl TestEnvironment {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("test root");
        let database = root.path().join("state").join("memory.sqlite3");
        let workspace = root.path().join("workspace");
        let bin = root.path().join("bin");
        fs::create_dir(&workspace).expect("workspace");
        fs::create_dir(&bin).expect("bin directory");
        git2::Repository::init(&workspace).expect("workspace repository");
        Self {
            root,
            database,
            workspace,
            bin,
        }
    }

    fn install_stub(&self, name: &str) {
        self.install_script(name, "#!/bin/sh\nexit 0\n");
    }

    fn install_script(&self, name: &str, script: &str) {
        let path = self.bin.join(name);
        fs::write(&path, script).expect("stub command");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("stub permissions");
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.run_from(&self.workspace, arguments)
    }

    fn run_from(&self, directory: &Path, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_agent-memory-ctl"))
            .args(arguments)
            .env("ANOLISA_MEMORY_DB", &self.database)
            .env("HOME", self.root.path())
            .env("XDG_STATE_HOME", self.root.path().join("xdg-state"))
            .env("PATH", &self.bin)
            .current_dir(directory)
            .output()
            .expect("run agent-memory-ctl")
    }
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON stdout: {error}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn check<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .expect("doctor checks")
        .iter()
        .find(|check| check["name"] == name)
        .unwrap_or_else(|| panic!("missing doctor check {name}"))
}

fn assert_output_hides_path(output: &Output, path: &Path) {
    let path = path.to_string_lossy();
    assert!(!String::from_utf8_lossy(&output.stdout).contains(path.as_ref()));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(path.as_ref()));
}

#[test]
fn status_json_creates_only_the_typed_local_store() {
    let environment = TestEnvironment::new();
    let output = environment.run(&["status", "--json"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report = json_stdout(&output);
    assert_eq!(report["status"], "ready");
    assert_eq!(report["backend"], "local-sqlite-v1");
    assert_eq!(report["durability"], "durable");
    assert_eq!(report["sessions"], 0);
    assert_eq!(report["events"], 0);
    assert_eq!(report["tasks"], 0);
    assert_eq!(report["context_views"], 0);
    assert_eq!(report["context_view_retention_days"], 7);
    assert_eq!(report["closed_session_retention_days"], 30);
    assert_eq!(report["recall_sample_size"], 0);
    assert_eq!(report["diagnostic_recall_samples"], 0);
    assert!(environment.database.is_file());
    assert!(!environment.root.path().join(".anolisa").exists());
    assert_output_hides_path(&output, &environment.database);
}

#[test]
fn doctor_treats_mant_as_optional() {
    let environment = TestEnvironment::new();
    environment.install_stub("agent-memory-cosh-hook");
    environment.install_stub("cosh");
    let output = environment.run(&["doctor", "--json"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report = json_stdout(&output);
    assert_eq!(report["status"], "healthy");
    assert_eq!(check(&report, "local_store")["status"], "ok");
    assert_eq!(check(&report, "cosh_hook")["status"], "ok");
    assert_eq!(check(&report, "cosh_runtime")["status"], "available");
    assert_eq!(check(&report, "mant")["status"], "not_found");
    assert_eq!(check(&report, "mant")["required"], false);
    assert_output_hides_path(&output, &environment.database);
}

#[test]
fn doctor_probes_the_optional_mant_protocol() {
    let environment = TestEnvironment::new();
    environment.install_stub("agent-memory-cosh-hook");
    environment.install_script(
        "mant",
        "#!/bin/sh\nprintf '%s\\n' '{\"protocol\":\"mant.cli/v0.9\",\"nativeApiVersion\":\"0.9\",\"requestSchema\":\"mant.request/v0.9\",\"excerptSchema\":\"mant.excerpt/v0.9\",\"searchSchema\":\"mant.search/v0.9\"}'\n",
    );
    let output = environment.run(&["doctor", "--json"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = json_stdout(&output);
    assert_eq!(check(&report, "mant")["status"], "ok");
    assert!(
        check(&report, "mant")["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("mant.cli/v0.9"))
    );
    assert_output_hides_path(&output, &environment.database);
}

#[test]
fn doctor_fails_with_an_action_when_the_hook_is_missing() {
    let environment = TestEnvironment::new();
    environment.install_stub("cosh");
    let output = environment.run(&["doctor", "--json"]);

    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let report = json_stdout(&output);
    assert_eq!(report["status"], "unhealthy");
    let hook = check(&report, "cosh_hook");
    assert_eq!(hook["status"], "error");
    assert!(
        hook["action"]
            .as_str()
            .is_some_and(|action| action.contains("Reinstall"))
    );
    assert_output_hides_path(&output, &environment.database);
}

#[test]
fn demo_recalls_synthetic_evidence_after_cold_backend_reopen() {
    let environment = TestEnvironment::new();
    let demo = environment.run(&["demo", "--json"]);

    assert!(
        demo.status.success(),
        "{}",
        String::from_utf8_lossy(&demo.stderr)
    );
    assert!(demo.stderr.is_empty());
    let demo_report = json_stdout(&demo);
    assert_eq!(demo_report["status"], "ok");
    assert_eq!(demo_report["captured_events"], 1);
    assert!(
        demo_report["recalled_items"]
            .as_u64()
            .is_some_and(|count| count >= 1)
    );
    assert!(
        demo_report["recalled_candidate_evidence"]
            .as_u64()
            .is_some_and(|count| count >= 1)
    );
    assert_eq!(demo_report["outcome"], "useful");
    assert!(demo_report["cold_reopen_ms"].is_u64());
    assert!(!String::from_utf8_lossy(&demo.stdout).contains("demo verification"));

    let status = environment.run(&["status", "--json"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_report = json_stdout(&status);
    assert!(
        status_report["sessions"]
            .as_u64()
            .is_some_and(|count| count >= 2)
    );
    assert!(
        status_report["events"]
            .as_u64()
            .is_some_and(|count| count >= 1)
    );
    assert!(
        status_report["context_views"]
            .as_u64()
            .is_some_and(|count| count >= 1)
    );
    assert_eq!(status_report["recall_sample_size"], 0);
    assert_eq!(status_report["useful_outcomes"], 0);
    assert_eq!(status_report["diagnostic_recall_samples"], 1);
    assert_output_hides_path(&demo, &environment.database);
    assert_output_hides_path(&status, &environment.database);
}

#[test]
fn why_and_confirmed_forget_manage_only_the_demo_view() {
    let environment = TestEnvironment::new();
    let nested = environment.workspace.join("nested");
    fs::create_dir(&nested).expect("nested workspace directory");
    let demo = environment.run_from(&nested, &["demo", "--json"]);
    assert!(demo.status.success());
    let demo_report = json_stdout(&demo);
    let view_id = demo_report["context_view_id"]
        .as_str()
        .expect("demo context view id");

    let why = environment.run(&["why", view_id, "--json"]);
    assert!(
        why.status.success(),
        "{}",
        String::from_utf8_lossy(&why.stderr)
    );
    let explanation = json_stdout(&why);
    assert_eq!(explanation["status"], "explained");
    assert_eq!(explanation["context_view_id"], view_id);
    assert_eq!(explanation["outcome"], "useful");
    assert!(
        explanation["admitted"]
            .as_u64()
            .is_some_and(|count| count >= 1)
    );
    assert!(!String::from_utf8_lossy(&why.stdout).contains("demo verification"));

    let unconfirmed = environment.run(&["forget", "context-view", view_id, "--json"]);
    assert!(!unconfirmed.status.success());
    assert!(unconfirmed.stdout.is_empty());
    let error: Value = serde_json::from_slice(&unconfirmed.stderr).expect("JSON error");
    assert_eq!(error["code"], "confirmation_required");

    let forgotten = environment.run(&["forget", "context-view", view_id, "--yes", "--json"]);
    assert!(forgotten.status.success());
    let forgotten_report = json_stdout(&forgotten);
    assert_eq!(forgotten_report["status"], "forgotten");
    assert_eq!(forgotten_report["deleted"], true);

    let absent = environment.run(&["why", view_id, "--json"]);
    assert!(!absent.status.success());
    assert!(absent.stdout.is_empty());
    let error: Value = serde_json::from_slice(&absent.stderr).expect("JSON error");
    assert_eq!(error["status"], "error");
    assert_output_hides_path(&why, &environment.database);
    assert_output_hides_path(&forgotten, &environment.database);
    assert_output_hides_path(&absent, &environment.database);
}

#[test]
fn status_failure_is_actionable_and_does_not_disclose_the_database_path() {
    let environment = TestEnvironment::new();
    let parent = environment.database.parent().expect("database parent");
    fs::create_dir(parent).expect("database parent");
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).expect("unsafe mode");
    let output = environment.run(&["status", "--json"]);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).expect("JSON error");
    assert_eq!(error["status"], "error");
    assert_eq!(error["code"], "local_store_permissions");
    assert!(
        error["action"]
            .as_str()
            .is_some_and(|action| action.contains("0700"))
    );
    assert_output_hides_path(&output, &environment.database);
}
