//! Tool response reservations survive failure, cancellation and configuration changes.

use aw_contracts::{
    events::{EventName, Notification},
    tool_response::ToolResponse,
};
use aw_core::{
    journal::FileJournal,
    ports::{Cancellation, HostError, NeverCancel},
    tool_response::{ToolResponseCommand, ToolResponseHost},
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
                "result-test-{}-{}",
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
        event: EventName::ToolAfter,
        source: "native_callback".into(),
        native_event: Some("PostToolUse".into()),
        runtime_id: "r1".into(),
        runtime_generation: 1,
        session_id: Some("s1".into()),
        session_epoch: Some(1),
        turn_id: None,
        turn_unknown_reason: Some("submission_does_not_prove_acceptance".into()),
        tool_call_id: Some("t1".into()),
        subagent_id: None,
        occurrence_id: "result-1".into(),
        config_revision: "c1".into(),
        payload: json!({"tool_response":"private source"}),
    }
}

struct Host {
    calls: usize,
    output: Value,
}
impl ToolResponseHost for Host {
    fn respond(
        &mut self,
        _: &str,
        request: &Value,
        remaining_ms: u64,
        _: &dyn Cancellation,
    ) -> Result<Value, HostError> {
        self.calls += 1;
        assert!(remaining_ms > 0 && remaining_ms <= 2000);
        assert_eq!(request["scope"], "tool.after.respond");
        assert_eq!(request["source"]["text"], "private source");
        Ok(self.output.clone())
    }
}
fn command() -> ToolResponseCommand<'static> {
    ToolResponseCommand {
        id: "projector",
        source: "private source",
        deadline: Instant::now() + Duration::from_secs(2),
    }
}
fn replacement() -> Value {
    json!({"format":1,"decision":"replace","source_digest":aw_contracts::canonical::digest(b"private source"),"text":"private candidate"})
}

#[test]
fn source_binding_and_exact_output_contract_reject_mixed_authority() {
    let digest = aw_contracts::canonical::digest(b"private source");
    assert_eq!(
        ToolResponse::parse(&replacement(), &digest).unwrap(),
        ToolResponse::Replace("private candidate".into())
    );
    assert_eq!(
        ToolResponse::parse(&json!({"format":1,"decision":"preserve"}), &digest).unwrap(),
        ToolResponse::Preserve
    );
    for (key, value) in [
        ("source_digest", json!("other")),
        ("text", json!("")),
        ("text", json!("x".repeat(65537))),
        ("text", json!("x\0y")),
        ("format", json!(2)),
        ("decision", json!("deny")),
        ("continue", json!(false)),
        ("updatedToolOutput", json!("x")),
    ] {
        let mut output = replacement();
        output[key] = value;
        assert!(ToolResponse::parse(&output, &digest).is_err());
    }
}

#[test]
fn failed_claim_cannot_be_reacquired_with_changed_source_or_config() {
    let dir = Directory::new();
    let core = Core::new().unwrap();
    let mut journal = FileJournal::new(&dir.0).unwrap();
    let mut host = Host {
        calls: 0,
        output: json!({}),
    };
    assert!(core
        .respond_to_tool(&event(), &command(), &mut host, &mut journal, &NeverCancel)
        .unwrap()
        .is_none());
    let mut changed = event();
    changed.config_revision = "new".into();
    let mut route = command();
    route.source = "different source";
    assert!(core
        .respond_to_tool(&changed, &route, &mut host, &mut journal, &NeverCancel)
        .is_err());
    assert_eq!(host.calls, 1);
}

#[test]
fn expired_deadline_claims_without_running_the_host() {
    let dir = Directory::new();
    let core = Core::new().unwrap();
    let mut host = Host {
        calls: 0,
        output: replacement(),
    };
    let mut journal = FileJournal::new(&dir.0).unwrap();
    let mut route = command();
    route.deadline = Instant::now() - Duration::from_millis(1);
    assert!(core
        .respond_to_tool(&event(), &route, &mut host, &mut journal, &NeverCancel)
        .unwrap()
        .is_none());
    assert_eq!(host.calls, 0);
    assert!(core
        .respond_to_tool(&event(), &command(), &mut host, &mut journal, &NeverCancel)
        .is_err());
}

#[test]
fn wrong_event_or_missing_tool_identity_never_invokes_host() {
    let dir = Directory::new();
    let core = Core::new().unwrap();
    let mut host = Host {
        calls: 0,
        output: replacement(),
    };
    for case in 0..3 {
        let mut e = event();
        match case {
            0 => e.event = EventName::ToolBefore,
            1 => e.tool_call_id = None,
            _ => e.subagent_id = Some("child".into()),
        }
        assert!(core
            .respond_to_tool(
                &e,
                &command(),
                &mut host,
                &mut FileJournal::new(&dir.0).unwrap(),
                &NeverCancel
            )
            .is_err());
    }
    assert_eq!(host.calls, 0);
}

#[test]
fn unacknowledged_or_cancelled_completion_never_releases_candidate() {
    use aw_core::ports::{Journal, JournalError};
    use std::sync::atomic::AtomicBool;
    struct CancellingJournal<'a> {
        inner: FileJournal,
        flag: &'a AtomicBool,
        bad_ack: bool,
    }
    impl Journal for CancellingJournal<'_> {
        fn claim(&mut self, key: &str, plan: &Value) -> Result<Value, JournalError> {
            self.inner.claim(key, plan)
        }
        fn append(&mut self, key: &str, record: &Value) -> Result<Value, JournalError> {
            let ack = self.inner.append(key, record)?;
            if record["kind"] == "tool_response_completed" {
                if self.bad_ack {
                    return Ok(json!({}));
                }
                self.flag.store(true, Ordering::Relaxed);
            }
            Ok(ack)
        }
        fn release(&mut self, key: &str) {
            self.inner.release(key);
        }
    }
    struct Cancel<'a>(&'a AtomicBool);
    impl Cancellation for Cancel<'_> {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }
    for bad_ack in [false, true] {
        let dir = Directory::new();
        let flag = AtomicBool::new(false);
        let mut journal = CancellingJournal {
            inner: FileJournal::new(&dir.0).unwrap(),
            flag: &flag,
            bad_ack,
        };
        let mut host = Host {
            calls: 0,
            output: replacement(),
        };
        let result = Core::new().unwrap().respond_to_tool(
            &event(),
            &command(),
            &mut host,
            &mut journal,
            &Cancel(&flag),
        );
        assert!(if bad_ack {
            result.is_err()
        } else {
            result.unwrap().is_none()
        });
        assert_eq!(host.calls, 1);
    }
}
