//! Input response reservations survive failure, cancellation and configuration changes.

use aw_contracts::{
    events::{EventName, Notification},
    input_response::InputResponse,
};
use aw_core::{
    input_response::{InputResponseCommand, InputResponseHost},
    journal::FileJournal,
    ports::{Cancellation, HostError, NeverCancel},
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
                "input-test-{}-{}",
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
        event: EventName::InputSubmit,
        source: "native_callback".into(),
        native_event: Some("UserPromptSubmit".into()),
        runtime_id: "r1".into(),
        runtime_generation: 1,
        session_id: Some("s1".into()),
        session_epoch: Some(1),
        turn_id: None,
        turn_unknown_reason: Some("submission_does_not_prove_acceptance".into()),
        tool_call_id: None,
        subagent_id: None,
        occurrence_id: "input-1".into(),
        config_revision: "c1".into(),
        payload: json!({"prompt":"private prompt"}),
    }
}
struct Host {
    calls: usize,
    output: Value,
}
impl InputResponseHost for Host {
    fn respond(
        &mut self,
        _: &str,
        request: &Value,
        remaining_ms: u64,
        _: &dyn Cancellation,
    ) -> Result<Value, HostError> {
        assert!(remaining_ms > 0 && remaining_ms <= 2000);
        assert_eq!(request["scope"], "input.submit.respond");
        self.calls += 1;
        Ok(self.output.clone())
    }
}
fn command() -> InputResponseCommand<'static> {
    InputResponseCommand {
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
        output: json!({"decision":"continue"}),
    };
    let mut journal = FileJournal::new(&dir.0).unwrap();
    assert!(core
        .respond_to_input(&event, &command(), &mut host, &mut journal, &NeverCancel)
        .unwrap()
        .is_none());
    event.config_revision = "new-config".into();
    host.output = json!({"format":1,"decision":"continue"});
    assert!(core
        .respond_to_input(&event, &command(), &mut host, &mut journal, &NeverCancel)
        .is_err());
    assert_eq!(host.calls, 1);
    event.occurrence_id = "input-2".into();
    assert_eq!(
        core.respond_to_input(&event, &command(), &mut host, &mut journal, &NeverCancel)
            .unwrap(),
        Some(InputResponse::Continue(None))
    );
    assert_eq!(host.calls, 2);
}
#[test]
fn expired_deadline_claims_without_dispatching() {
    let dir = Directory::new();
    let core = Core::new().unwrap();
    let mut host = Host {
        calls: 0,
        output: json!({"format":1,"decision":"continue"}),
    };
    let route = InputResponseCommand {
        id: "response",
        deadline: Instant::now() - Duration::from_millis(1),
    };
    let mut journal = FileJournal::new(&dir.0).unwrap();
    assert!(core
        .respond_to_input(&event(), &route, &mut host, &mut journal, &NeverCancel)
        .unwrap()
        .is_none());
    assert_eq!(host.calls, 0);
    assert!(core
        .respond_to_input(&event(), &command(), &mut host, &mut journal, &NeverCancel)
        .is_err());
}
#[test]
fn other_events_cannot_invoke_the_input_authority() {
    let dir = Directory::new();
    let core = Core::new().unwrap();
    let mut host = Host {
        calls: 0,
        output: json!({}),
    };
    let mut event = event();
    event.event = EventName::ToolBefore;
    assert!(core
        .respond_to_input(
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
fn contract_rejects_oversized_empty_and_native_response_fields() {
    for output in [
        json!({"format":2,"decision":"continue"}),
        json!({"format":1,"decision":"reject"}),
        json!({"format":1,"decision":"reject","reason":" "}),
        json!({"format":1,"decision":"continue","additional_context":"x".repeat(16385)}),
        json!({"format":1,"decision":"continue","additional_context":"a\0b"}),
        json!({"format":1,"decision":"continue","continue":true}),
        json!({"format":1,"decision":"allow"}),
    ] {
        assert!(InputResponse::parse(&output).is_err());
    }
    assert!(InputResponse::parse(
        &json!({"format":1,"decision":"continue","additional_context":"x".repeat(16384)})
    )
    .is_ok());
}
