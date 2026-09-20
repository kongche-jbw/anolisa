//! Stop response reservations survive failure, cancellation and configuration changes.

use aw_contracts::{
    events::{EventName, Notification},
    stop_response::StopResponse,
};
use aw_core::{
    journal::FileJournal,
    ports::{Cancellation, HostError, NeverCancel},
    stop_response::{StopResponseCommand, StopResponseHost},
    Core,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!(
                "stop-test-{}-{}",
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
        event: EventName::TurnStop,
        source: "native_callback".into(),
        native_event: Some("Stop".into()),
        runtime_id: "r1".into(),
        runtime_generation: 1,
        session_id: Some("s1".into()),
        session_epoch: Some(1),
        turn_id: None,
        turn_unknown_reason: Some("submission_does_not_prove_acceptance".into()),
        tool_call_id: None,
        subagent_id: None,
        occurrence_id: "stop-1".into(),
        config_revision: "c1".into(),
        payload: json!({"stop_hook_active":false,"last_assistant_message":"private answer"}),
    }
}
struct Host {
    calls: usize,
    output: Value,
}
impl StopResponseHost for Host {
    fn respond(
        &mut self,
        _: &str,
        request: &Value,
        remaining_ms: u64,
        _: &dyn Cancellation,
    ) -> Result<Value, HostError> {
        assert!(remaining_ms > 0 && remaining_ms <= 2000);
        assert_eq!(request["scope"], "turn.stop.respond");
        self.calls += 1;
        Ok(self.output.clone())
    }
}
fn command() -> StopResponseCommand<'static> {
    StopResponseCommand {
        id: "response",
        deadline: Instant::now() + Duration::from_secs(2),
    }
}
#[test]
fn failed_response_claim_cannot_be_reacquired_by_a_new_config() {
    let dir = Directory::new();
    let core = Core::new().unwrap();
    let mut event = event();
    let mut host = Host {
        calls: 0,
        output: json!({"decision":"allow_stop"}),
    };
    let mut journal = FileJournal::new(&dir.0).unwrap();
    assert!(core
        .respond_to_stop(&event, &command(), &mut host, &mut journal, &NeverCancel)
        .unwrap()
        .is_none());
    event.config_revision = "new-config".into();
    host.output = json!({"format":1,"decision":"allow_stop"});
    assert!(core
        .respond_to_stop(&event, &command(), &mut host, &mut journal, &NeverCancel)
        .is_err());
    assert_eq!(host.calls, 1);
    event.occurrence_id = "stop-2".into();
    assert_eq!(
        core.respond_to_stop(&event, &command(), &mut host, &mut journal, &NeverCancel)
            .unwrap(),
        Some(StopResponse::AllowStop)
    );
    assert_eq!(host.calls, 2);
}
#[test]
fn expired_deadline_claims_without_dispatching() {
    let dir = Directory::new();
    let core = Core::new().unwrap();
    let mut host = Host {
        calls: 0,
        output: json!({"format":1,"decision":"allow_stop"}),
    };
    let route = StopResponseCommand {
        id: "response",
        deadline: Instant::now() - Duration::from_millis(1),
    };
    let mut journal = FileJournal::new(&dir.0).unwrap();
    assert!(core
        .respond_to_stop(&event(), &route, &mut host, &mut journal, &NeverCancel)
        .unwrap()
        .is_none());
    assert_eq!(host.calls, 0);
    assert!(core
        .respond_to_stop(&event(), &command(), &mut host, &mut journal, &NeverCancel)
        .is_err());
}
#[test]
fn other_events_cannot_invoke_the_stop_authority() {
    let dir = Directory::new();
    let core = Core::new().unwrap();
    let mut host = Host {
        calls: 0,
        output: json!({}),
    };
    let mut event = event();
    event.event = EventName::ToolBefore;
    assert!(core
        .respond_to_stop(
            &event,
            &command(),
            &mut host,
            &mut FileJournal::new(&dir.0).unwrap(),
            &NeverCancel
        )
        .is_err());
    assert_eq!(host.calls, 0);
}
#[test]
fn contract_accepts_only_stop_or_bounded_continuation() {
    for output in [
        json!({"format":2,"decision":"allow_stop"}),
        json!({"format":1,"decision":"continue"}),
        json!({"format":1,"decision":"continue","reason":" "}),
        json!({"format":1,"decision":"continue","reason":"x".repeat(16385)}),
        json!({"format":1,"decision":"continue","reason":"a\0b"}),
        json!({"format":1,"decision":"allow_stop","reason":"mixed"}),
        json!({"format":1,"decision":"allow_stop","continue":true}),
        json!({"format":1,"decision":"deny"}),
    ] {
        assert!(StopResponse::parse(&output).is_err(), "{output}");
    }
    assert_eq!(
        StopResponse::parse(&json!({"format":1,"decision":"allow_stop"})).unwrap(),
        StopResponse::AllowStop
    );
    assert_eq!(
        StopResponse::parse(&json!({"format":1,"decision":"continue","reason":"x".repeat(16384)}))
            .unwrap(),
        StopResponse::Continue("x".repeat(16384))
    );
}

#[test]
fn active_stop_check_claims_and_skips_without_false_success() {
    let dir = Directory::new();
    let core = Core::new().unwrap();
    let mut event = event();
    event.payload["stop_hook_active"] = json!(true);
    let mut host = Host {
        calls: 0,
        output: json!({"format":1,"decision":"continue","reason":"again"}),
    };
    let mut journal = FileJournal::new(&dir.0).unwrap();
    assert!(core
        .respond_to_stop(&event, &command(), &mut host, &mut journal, &NeverCancel)
        .unwrap()
        .is_none());
    assert!(core
        .respond_to_stop(&event, &command(), &mut host, &mut journal, &NeverCancel)
        .is_err());
    assert_eq!(host.calls, 0);
    let records = fs::read_dir(&dir.0)
        .unwrap()
        .map(|p| fs::read_to_string(p.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(records.contains("stop_response_skipped"));
    assert!(!records.contains("stop_response_completed"));
    assert!(!records.contains("private answer"));
}

#[test]
fn cancellation_after_command_withholds_continuation() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct CancelHost<'a>(&'a AtomicBool);
    impl StopResponseHost for CancelHost<'_> {
        fn respond(
            &mut self,
            _: &str,
            _: &Value,
            _: u64,
            _: &dyn Cancellation,
        ) -> Result<Value, HostError> {
            self.0.store(true, Ordering::Relaxed);
            Ok(json!({"format":1,"decision":"continue","reason":"must not escape"}))
        }
    }
    struct Cancel<'a>(&'a AtomicBool);
    impl Cancellation for Cancel<'_> {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }
    let dir = Directory::new();
    let flag = AtomicBool::new(false);
    assert!(Core::new()
        .unwrap()
        .respond_to_stop(
            &event(),
            &command(),
            &mut CancelHost(&flag),
            &mut FileJournal::new(&dir.0).unwrap(),
            &Cancel(&flag)
        )
        .unwrap()
        .is_none());
}
