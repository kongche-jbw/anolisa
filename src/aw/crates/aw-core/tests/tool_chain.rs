//! Candidate ordering, final check and cancellation authority regressions.
use aw_contracts::events::{EventName, Notification};
use aw_core::{
    journal::FileJournal,
    ports::{Cancellation, HostError, NeverCancel},
    tool_chain::{ToolChain, ToolHost},
    Core,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!(
                "tool-chain-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn event() -> Notification {
    Notification {
        format: 1,
        event: EventName::ToolBefore,
        source: "native_callback".into(),
        native_event: Some("PreToolUse".into()),
        runtime_id: "runtime".into(),
        runtime_generation: 1,
        session_id: Some("session".into()),
        session_epoch: Some(1),
        turn_id: None,
        turn_unknown_reason: Some("unknown".into()),
        tool_call_id: Some("tool".into()),
        subagent_id: None,
        occurrence_id: "one".into(),
        config_revision: "config".into(),
        payload: json!({}),
    }
}
fn candidate() -> Value {
    json!({"tool_name":"Bash","tool_input":{"command":"original-private"},"cwd":"/workspace"})
}
struct Host {
    seen: Vec<String>,
    cancel: std::sync::Arc<AtomicBool>,
    cancel_guard: bool,
}
// Use a separate cancellation owner so the mutable Host cannot alter its verdict.
struct Cancel(std::sync::Arc<AtomicBool>);
impl Cancellation for Cancel {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
impl ToolHost for Host {
    fn admit(&self, _: &ToolChain) -> Result<(), HostError> {
        Ok(())
    }
    fn validate(&self, _: &Value) -> Result<(), HostError> {
        Ok(())
    }
    fn transform(
        &mut self,
        id: &str,
        c: &Value,
        _: u64,
        _: &dyn Cancellation,
    ) -> Result<Value, HostError> {
        self.seen.push(id.into());
        let mut next = c.clone();
        match id {
            "first" => next["tool_input"]["command"] = json!("transformed-private"),
            "second" => {
                assert_eq!(c["tool_input"]["command"], "transformed-private");
                next["tool_input"]["command"] = json!("forbidden-private");
            }
            "context" => next["cwd"] = json!("/changed"),
            _ => unreachable!(),
        }
        Ok(next)
    }
    fn guard(
        &mut self,
        _: &str,
        c: &Value,
        _: u64,
        _: &dyn Cancellation,
    ) -> Result<bool, HostError> {
        self.seen.push("guard".into());
        if self.cancel_guard {
            self.cancel.store(true, Ordering::Relaxed);
        }
        Ok(c["tool_input"]["command"] != "forbidden-private")
    }
}
fn host() -> Host {
    Host {
        seen: vec![],
        cancel: std::sync::Arc::new(AtomicBool::new(false)),
        cancel_guard: false,
    }
}
#[test]
fn guard_sees_the_last_candidate_and_denial_returns_no_content() {
    let dir = Dir::new();
    let mut h = host();
    let chain = ToolChain {
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
        transforms: vec!["first".into(), "second".into()],
        guard: "guard".into(),
    };
    let result = Core::new()
        .unwrap()
        .check_tool(
            &event(),
            candidate(),
            &chain,
            &mut h,
            &mut FileJournal::new(&dir.0).unwrap(),
            &NeverCancel,
        )
        .unwrap();
    assert!(result.candidate().is_none());
    assert_eq!(h.seen, ["first", "second", "guard"]);
    assert_eq!(result.record()["reason"], "policy_denied");
    assert!(!result.record().to_string().contains("private"));
}
#[test]
fn context_mutation_stops_before_guard_and_claim_cannot_replay() {
    let dir = Dir::new();
    let mut h = host();
    let chain = ToolChain {
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
        transforms: vec!["context".into()],
        guard: "guard".into(),
    };
    let core = Core::new().unwrap();
    let mut journal = FileJournal::new(&dir.0).unwrap();
    assert!(core
        .check_tool(
            &event(),
            candidate(),
            &chain,
            &mut h,
            &mut journal,
            &NeverCancel
        )
        .unwrap()
        .candidate()
        .is_none());
    assert_eq!(h.seen, ["context"]);
    assert!(core
        .check_tool(
            &event(),
            candidate(),
            &chain,
            &mut h,
            &mut journal,
            &NeverCancel
        )
        .is_err());
    assert_eq!(h.seen, ["context"]);
}
#[test]
fn preexisting_cancellation_never_calls_a_transform_or_guard() {
    let dir = Dir::new();
    let mut h = host();
    let chain = ToolChain {
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
        transforms: vec!["first".into()],
        guard: "guard".into(),
    };
    let cancelled = std::sync::Arc::new(AtomicBool::new(true));
    let result = Core::new()
        .unwrap()
        .check_tool(
            &event(),
            candidate(),
            &chain,
            &mut h,
            &mut FileJournal::new(&dir.0).unwrap(),
            &Cancel(cancelled),
        )
        .unwrap();
    assert!(result.candidate().is_none());
    assert!(h.seen.is_empty());
}
#[test]
fn approval_returns_only_the_candidate_checked_by_guard() {
    let dir = Dir::new();
    let mut h = host();
    let chain = ToolChain {
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
        transforms: vec!["first".into()],
        guard: "guard".into(),
    };
    let result = Core::new()
        .unwrap()
        .check_tool(
            &event(),
            candidate(),
            &chain,
            &mut h,
            &mut FileJournal::new(&dir.0).unwrap(),
            &NeverCancel,
        )
        .unwrap();
    assert_eq!(
        result.candidate().unwrap()["tool_input"]["command"],
        "transformed-private"
    );
    assert_eq!(h.seen, ["first", "guard"]);
}

#[test]
fn cancellation_during_guard_discards_even_a_successful_verdict() {
    let dir = Dir::new();
    let mut h = host();
    h.cancel_guard = true;
    let cancel = Cancel(std::sync::Arc::clone(&h.cancel));
    let chain = ToolChain {
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
        transforms: vec![],
        guard: "guard".into(),
    };
    let result = Core::new()
        .unwrap()
        .check_tool(
            &event(),
            candidate(),
            &chain,
            &mut h,
            &mut FileJournal::new(&dir.0).unwrap(),
            &cancel,
        )
        .unwrap();
    assert!(result.candidate().is_none());
    assert_eq!(h.seen, ["guard"]);
}

#[test]
fn exhausted_parent_budget_is_not_restarted_at_guard_entry() {
    let dir = Dir::new();
    let mut h = host();
    let chain = ToolChain {
        deadline: std::time::Instant::now(),
        transforms: vec!["first".into()],
        guard: "guard".into(),
    };
    let result = Core::new()
        .unwrap()
        .check_tool(
            &event(),
            candidate(),
            &chain,
            &mut h,
            &mut FileJournal::new(&dir.0).unwrap(),
            &NeverCancel,
        )
        .unwrap();
    assert!(result.candidate().is_none());
    assert!(h.seen.is_empty());
}
