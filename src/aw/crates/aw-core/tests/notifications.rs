//! Notification authority and durable reservation tests, including terminal facts.

use aw_contracts::events::{EventName, Notification};
use aw_core::{
    journal::FileJournal,
    notifications::NotificationHost,
    ports::{Cancellation, HostError, NeverCancel},
    Core,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!(
                "notify-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn event() -> Notification {
    Notification {
        format: 1,
        event: EventName::RuntimeExited,
        source: "runtime_owner".into(),
        native_event: None,
        runtime_id: "run-1".into(),
        runtime_generation: 1,
        session_id: None,
        session_epoch: None,
        turn_id: None,
        turn_unknown_reason: Some("process_exit_does_not_prove_task_completion".into()),
        tool_call_id: None,
        subagent_id: None,
        occurrence_id: "exit-1".into(),
        config_revision: "config-1".into(),
        payload: json!({"private":"never journal this"}),
    }
}

#[derive(Default)]
struct Host {
    called: Vec<String>,
}
impl NotificationHost for Host {
    fn notify(
        &mut self,
        command: &str,
        notification: &Notification,
        remaining_ms: u64,
        _: &dyn Cancellation,
    ) -> Result<Value, HostError> {
        assert!(remaining_ms > 0 && remaining_ms <= 2000);
        assert_eq!(notification.payload["private"], "never journal this");
        self.called.push(command.into());
        Ok(if command == "forged-response" {
            json!({"continue":false})
        } else {
            json!({"format":1,"observed":true})
        })
    }
}

#[test]
fn terminal_notifications_are_ordered_and_claimed_without_runtime_running() {
    let dir = Directory::new();
    let core = Core::new().unwrap();
    let event = event();
    let mut journal = FileJournal::new(&dir.0).unwrap();
    let mut host = Host::default();
    let commands = vec!["one".into(), "two".into()];
    let result = core
        .notify(&event, &commands, &mut host, &mut journal, &NeverCancel)
        .unwrap();
    assert_eq!(host.called, commands);
    assert_eq!(result["gap"], false);
    drop(journal);
    let mut journal = FileJournal::new(&dir.0).unwrap();
    let mut changed = event.clone();
    changed.config_revision = "config-2".into();
    assert!(core
        .notify(&changed, &commands, &mut host, &mut journal, &NeverCancel)
        .is_err());
    assert_eq!(host.called.len(), 2);
    assert!(!event
        .metadata()
        .unwrap()
        .to_string()
        .contains("never journal this"));
}

#[test]
fn a_notification_response_never_becomes_a_control_decision() {
    let dir = Directory::new();
    let mut host = Host::default();
    let result = Core::new()
        .unwrap()
        .notify(
            &event(),
            &["forged-response".into(), "next".into()],
            &mut host,
            &mut FileJournal::new(&dir.0).unwrap(),
            &NeverCancel,
        )
        .unwrap();
    assert_eq!(result["gap"], true);
    assert_eq!(result["commands"][0]["outcome"], "failed");
    assert_eq!(result["commands"][0]["reason"], "invalid_notification_ack");
    assert_eq!(result["commands"][1]["outcome"], "completed");
    assert!(result.get("continue").is_none());
}

struct Cancelled;
impl Cancellation for Cancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

#[test]
fn cancellation_does_not_release_the_occurrence_for_replay() {
    let dir = Directory::new();
    let mut journal = FileJournal::new(&dir.0).unwrap();
    let mut host = Host::default();
    let core = Core::new().unwrap();
    let report = core
        .notify(
            &event(),
            &["one".into()],
            &mut host,
            &mut journal,
            &Cancelled,
        )
        .unwrap();
    assert_eq!(report["commands"][0]["outcome"], "skipped");
    assert!(host.called.is_empty());
    assert!(core
        .notify(
            &event(),
            &["one".into()],
            &mut host,
            &mut journal,
            &NeverCancel
        )
        .is_err());
}

#[test]
fn malformed_envelopes_and_duplicate_commands_fail_before_dispatch() {
    let dir = Directory::new();
    let mut journal = FileJournal::new(&dir.0).unwrap();
    let mut host = Host::default();
    let core = Core::new().unwrap();
    let mut invalid = event();
    invalid.turn_id = Some("invented-turn".into());
    assert!(core
        .notify(
            &invalid,
            &["one".into()],
            &mut host,
            &mut journal,
            &NeverCancel
        )
        .is_err());
    assert!(core
        .notify(
            &event(),
            &["one".into(), "one".into()],
            &mut host,
            &mut journal,
            &NeverCancel
        )
        .is_err());
    assert!(host.called.is_empty());
    core.notify(
        &event(),
        &["one".into()],
        &mut host,
        &mut journal,
        &NeverCancel,
    )
    .unwrap();
}
