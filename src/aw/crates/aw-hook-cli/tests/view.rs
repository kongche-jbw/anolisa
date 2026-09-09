//! View counters require journal-bound records and an explicit native session.

use aw_contracts::canonical;
use aw_core::{journal::FileJournal, ports::Journal};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    binding: Value,
    event: Value,
    key: String,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/view-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        let f = canonical::parse(include_bytes!("../../../tests/fixtures/contracts.json")).unwrap();
        let key = canonical::digest(root.as_os_str().as_encoded_bytes());
        let mut journal = FileJournal::new(root.join("journal")).unwrap();
        journal.claim(&key, &f["capability-plan-v1"]).unwrap();
        journal
            .append(
                &key,
                &json!({"kind":"invocation_settled","receipt":f["provider-receipt-v1"]}),
            )
            .unwrap();
        let ack = journal
            .append(
                &key,
                &json!({"kind":"execution_settled","execution":f["plan-execution-v1"]}),
            )
            .unwrap();
        let event = json!({"event_key":key,"source_digest":f["capability-plan-v1"]["source_digest"],"execution":f["plan-execution-v1"],"journal_ack":ack,"calls":[{"receipt":f["provider-receipt-v1"],"output":f["context-projection-prepare-output-v2"]}],"adoption":"adopted","saved_bytes":999999});
        let stat = fs::read_to_string(format!("/proc/{}/stat", std::process::id())).unwrap();
        let ticks: u64 = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap()
            .parse()
            .unwrap();
        let binding = json!({"runtime":f["runtime-binding-v1"],"scope":f["capability-plan-v1"]["scope"],"agent_pid":std::process::id(),"agent_start_ticks":ticks,"journal":root.join("journal"),"evidence":root.join("evidence")});
        fs::create_dir(root.join("evidence")).unwrap();
        Self {
            root,
            binding,
            event,
            key,
        }
    }
    fn run(&self) -> Output {
        fs::write(
            self.root.join("binding.json"),
            serde_json::to_vec(&self.binding).unwrap(),
        )
        .unwrap();
        fs::write(
            self.root
                .join("evidence")
                .join(format!("{}.json", self.key)),
            serde_json::to_vec(&self.event).unwrap(),
        )
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_aw-view-cli"))
            .arg(self.root.join("binding.json"))
            .output()
            .unwrap()
    }
    fn value(&self) -> Value {
        let out = self.run();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn candidate_and_flat_adoption_claim_do_not_count_as_saved() {
    let f = Fixture::new();
    let view = f.value();
    assert_eq!(view["providers"][0]["calls"], 1);
    assert_eq!(view["providers"][0]["candidates"], 1);
    assert_eq!(view["providers"][0]["adopted"], 0);
    assert_eq!(view["providers"][0]["saved_bytes"], 0);
    assert_eq!(view["runtime_alive"], true);
}
#[test]
fn another_native_session_has_no_counters() {
    let mut f = Fixture::new();
    f.binding["scope"]["session_id"] = json!("fresh-session");
    f.binding["runtime"]["session_id"] = json!("fresh-session");
    assert_eq!(f.value()["providers"], json!([]));
}
#[test]
fn mismatched_runtime_generation_is_rejected() {
    let mut f = Fixture::new();
    f.binding["scope"]["runtime_generation"] = json!(2);
    assert!(!f.run().status.success());
}
#[test]
fn receipt_and_output_tampering_are_rejected() {
    let mut f = Fixture::new();
    f.event["calls"][0]["output"]["candidate"]["content"] = json!("changed");
    assert!(!f.run().status.success());
    let mut f = Fixture::new();
    f.event["calls"][0]["receipt"]["provider_id"] = json!("other");
    assert!(!f.run().status.success());
}
#[test]
fn intact_journal_prefix_is_insufficient_without_expected_tip() {
    let f = Fixture::new();
    let p = f.root.join("journal").join(format!("{}.jsonl", f.key));
    let text = fs::read_to_string(&p).unwrap();
    let prefix = text.lines().take(2).collect::<Vec<_>>().join("\n") + "\n";
    fs::write(p, prefix).unwrap();
    assert!(!f.run().status.success());
}
#[test]
fn pid_reuse_does_not_report_live_runtime() {
    let mut f = Fixture::new();
    f.binding["agent_start_ticks"] = json!(0);
    assert_eq!(f.value()["runtime_alive"], false);
}
#[test]
fn missing_native_session_never_falls_back_to_runtime_totals() {
    let mut f = Fixture::new();
    f.binding["scope"]
        .as_object_mut()
        .unwrap()
        .remove("session_id");
    assert!(!f.run().status.success());
}

#[test]
fn adoption_bindings_are_selected_by_event_key() {
    let mut f = Fixture::new();
    f.binding["adoption_bindings"] = json!({"another-event": f.root.join("absent-binding.json")});
    assert_eq!(f.value()["providers"][0]["adopted"], 0);
    f.binding["adoption_bindings"] = json!({f.key.clone(): f.root.join("absent-binding.json")});
    assert!(!f.run().status.success());
}

#[test]
fn unreaped_exited_agent_is_not_alive() {
    let mut child = Command::new("/bin/true").spawn().unwrap();
    let path = format!("/proc/{}/stat", child.id());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let ticks = loop {
        let stat = fs::read_to_string(&path).unwrap();
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        if fields[0] == "Z" {
            break fields[19].parse::<u64>().unwrap();
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let mut f = Fixture::new();
    f.binding["agent_pid"] = json!(child.id());
    f.binding["agent_start_ticks"] = json!(ticks);
    let view = f.value();
    child.wait().unwrap();
    assert_eq!(view["runtime_alive"], false);
}

#[test]
fn interrupted_current_event_is_not_reported_as_zero_calls() {
    let f = Fixture::new();
    let fixtures =
        canonical::parse(include_bytes!("../../../tests/fixtures/contracts.json")).unwrap();
    let mut journal = FileJournal::new(f.root.join("journal")).unwrap();
    journal
        .claim(
            &canonical::digest(b"interrupted-event"),
            &fixtures["capability-plan-v1"],
        )
        .unwrap();
    assert!(!f.run().status.success());
}
