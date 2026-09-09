//! Synthetic Host tests exercise public Core boundaries, not deployed providers.

use aw_contracts::{canonical, Registry};
use aw_core::{
    ports::{
        Cancellation, Clock, HostError, Journal, JournalError, NeverCancel, ProviderHost,
        ProviderResult,
    },
    Core, Error, PrepareRequest, StepInput,
};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

fn fixtures() -> Value {
    canonical::parse(include_bytes!("../../../tests/fixtures/contracts.json")).unwrap()
}

struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

struct CancelFlag(Rc<Cell<bool>>);
impl Cancellation for CancelFlag {
    fn is_cancelled(&self) -> bool {
        self.0.get()
    }
}

#[derive(Default)]
struct MemoryJournal {
    claims: BTreeSet<String>,
    records: Vec<Value>,
    fail_claim: bool,
    fail_append: Option<usize>,
}
impl Journal for MemoryJournal {
    fn claim(&mut self, event_key: &str, plan: &Value) -> Result<Value, JournalError> {
        if self.fail_claim {
            return Err(JournalError::InvalidRecord);
        }
        if !self.claims.insert(event_key.to_owned()) {
            return Err(JournalError::AlreadyClaimed);
        }
        Ok(ack(plan, "claim"))
    }
    fn append(&mut self, event_key: &str, record: &Value) -> Result<Value, JournalError> {
        if !self.claims.contains(event_key) || self.fail_append == Some(self.records.len()) {
            return Err(JournalError::InvalidRecord);
        }
        self.records.push(record.clone());
        Ok(ack(record, &format!("record-{}", self.records.len())))
    }
}
fn ack(record: &Value, id: &str) -> Value {
    json!({"source_id":"synthetic-journal", "record_id":id,
        "digest":canonical::document_digest(record).unwrap()})
}

#[derive(Clone, Copy)]
enum Reply {
    Allow,
    Deny,
    Warn,
    Failed,
    Transport,
    WrongReceipt,
    WrongInput,
    WrongOutput,
    LateReceipt,
}
struct Host {
    descriptors: BTreeMap<String, Value>,
    invoked: Vec<Value>,
    replies: Vec<Reply>,
    cancel_after_call: Option<Rc<Cell<bool>>>,
}
impl Host {
    fn new() -> Self {
        Self {
            descriptors: BTreeMap::from([(
                "fixture-provider".into(),
                fixtures()["provider-descriptor-v1"].clone(),
            )]),
            invoked: vec![],
            replies: vec![],
            cancel_after_call: None,
        }
    }
    fn add_provider(&mut self, id: &str) {
        let mut descriptor = fixtures()["provider-descriptor-v1"].clone();
        descriptor["provider_id"] = json!(id);
        self.descriptors.insert(id.into(), descriptor);
    }
}
impl ProviderHost for Host {
    fn descriptor(&self, id: &str) -> Option<&Value> {
        self.descriptors.get(id)
    }
    fn invoke(&mut self, invocation: &Value) -> Result<ProviderResult, HostError> {
        let reply = self
            .replies
            .get(self.invoked.len())
            .copied()
            .unwrap_or(Reply::Allow);
        self.invoked.push(invocation.clone());
        if let Some(flag) = &self.cancel_after_call {
            flag.set(true);
        }
        if matches!(reply, Reply::Transport) {
            return Err(HostError {
                code: "synthetic_transport_failure".into(),
            });
        }
        let f = fixtures();
        let profile = match invocation["capability"].as_str().unwrap() {
            "security.command.inspect/v2" => "security-command-inspect",
            "security.content.inspect/v2" => "security-content-inspect",
            "context.projection.prepare/v2" => "context-projection-prepare",
            other => panic!("unexpected synthetic capability: {other}"),
        };
        let mut output = f[format!("{profile}-output-v2")].clone();
        if matches!(reply, Reply::Deny | Reply::Warn) {
            output["decision"]["verdict"] = json!(if matches!(reply, Reply::Deny) {
                "deny"
            } else {
                "warn"
            });
            output["decision"]["findings"] = json!([{"rule_id":"fixture-rule", "category":"dangerous_pattern", "severity":"high", "confidence":"high", "count":1}]);
            output["decision"]["reasons"] = json!(["policy.synthetic"]);
        }
        let mut receipt = f["provider-receipt-v1"].clone();
        for field in [
            "invocation_id",
            "provider_id",
            "provider_version",
            "manifest_digest",
            "capability",
            "scope",
            "input_schema",
            "input_digest",
            "plan_ref",
        ] {
            receipt[field] = invocation[field].clone();
        }
        receipt["started_at_ms"] = json!(1100);
        receipt["completed_at_ms"] = json!(1100);
        receipt["output"] = json!({"schema":invocation["output_schema"], "digest":canonical::document_digest(&output).unwrap(), "bytes":canonical::bytes(&output).unwrap().len()});
        match reply {
            Reply::Failed => {
                receipt["disposition"] = json!("failed");
                receipt["error_code"] = json!("synthetic_failure");
                receipt.as_object_mut().unwrap().remove("output");
                return Ok(ProviderResult {
                    receipt,
                    output: None,
                });
            }
            Reply::WrongReceipt => receipt["invocation_id"] = json!("another-call"),
            Reply::WrongInput => receipt["input_digest"] = json!("0".repeat(64)),
            Reply::WrongOutput => output["inspection"]["verdict"] = json!("tampered"),
            Reply::LateReceipt => receipt["completed_at_ms"] = json!(1101),
            _ => {}
        }
        Ok(ProviderResult {
            receipt,
            output: Some(output),
        })
    }
}

fn request(pre: bool) -> PrepareRequest {
    let f = fixtures();
    let registry = Registry::new().unwrap();
    let mut plan = f["capability-plan-v1"].clone();
    let mut boundary = f["boundary-descriptor-v1"].clone();
    if pre {
        plan["boundary"] = json!("pre_tool");
        plan["source_digest"] = f["security-command-inspect-input-v2"]["command"]["digest"].clone();
        plan["os_requirement"] = json!({"policy_digest":f["execution-intent-v1"]["protection_policy_digest"], "required_controls":["filesystem.access/v1"]});
        boundary["boundary"] = json!("pre_tool");
        boundary["can_replace_text"] = json!(false);
        boundary["can_deny_dispatch"] = json!(true);
        boundary["has_final_input_guard"] = json!(true);
        boundary["composition"] = json!({"input_finality":"revalidate_at_dispatch", "gate":"required_final_guard", "result_finality":"final"});
    }
    let profiles = if pre {
        ["security-command-inspect", "security-command-inspect"]
    } else {
        ["security-content-inspect", "context-projection-prepare"]
    };
    let mut inputs = BTreeMap::new();
    plan["steps"] = json!([]);
    for (i, profile) in profiles.into_iter().enumerate() {
        let mut step = f["capability-plan-v1"]["steps"][0].clone();
        let id = format!("step-{i}");
        step["step_id"] = json!(id);
        step["capability"] = json!(match profile {
            "security-command-inspect" => "security.command.inspect/v2",
            "security-content-inspect" => "security.content.inspect/v2",
            _ => "context.projection.prepare/v2",
        });
        step["input_schema"] = registry.reference(&format!("{profile}-input-v2")).unwrap();
        step["output_schema"] = registry.reference(&format!("{profile}-output-v2")).unwrap();
        step["on_failure"] = json!(if pre { "deny_dispatch" } else { "reject_plan" });
        plan["steps"].as_array_mut().unwrap().push(step);
        inputs.insert(
            id,
            StepInput {
                input: f[format!("{profile}-input-v2")].clone(),
                budget: f["capability-invocation-v1"]["budget"].clone(),
                deadline_at_ms: 2000,
            },
        );
    }
    PrepareRequest {
        plan,
        boundary,
        runtime: f["runtime-binding-v1"].clone(),
        inputs,
    }
}
fn second_provider(request: &mut PrepareRequest, host: &mut Host) {
    host.add_provider("second-provider");
    let mut selected = request.plan["steps"][0]["providers"][0].clone();
    selected["provider_id"] = json!("second-provider");
    request.plan["steps"][0]["selection"] = json!("all_distinct_providers");
    request.plan["steps"][0]["providers"]
        .as_array_mut()
        .unwrap()
        .push(selected);
}

#[test]
fn complete_plan_is_serial_and_candidate_is_not_adoption() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let mut journal = MemoryJournal::default();
    let prepared = core.prepare(request(false), &host, 1000).unwrap();
    let result = core
        .execute(
            prepared,
            &mut host,
            &mut journal,
            &FixedClock(1100),
            &NeverCancel,
        )
        .unwrap();
    assert_eq!(result.record()["decision"], "proceed");
    assert_eq!(result.calls().len(), 2);
    assert_eq!(host.invoked[0]["plan_ref"]["step_id"], "step-0");
    assert_eq!(host.invoked[1]["plan_ref"]["step_id"], "step-1");
    assert!(
        result.record()["steps"][0]["settled_sequence"]
            .as_u64()
            .unwrap()
            < result.record()["steps"][1]["started_sequence"]
                .as_u64()
                .unwrap()
    );
    assert!(result.calls()[1]
        .result()
        .output
        .as_ref()
        .unwrap()
        .get("candidate")
        .is_some());
    assert!(result.record().get("adoption").is_none());
    assert!(journal.records.iter().all(|r| r.get("adoption").is_none()));
    assert_eq!(journal.records.last().unwrap()["kind"], "execution_settled");
    assert_eq!(
        result.journal_ack()["digest"],
        canonical::document_digest(journal.records.last().unwrap()).unwrap()
    );
}

#[test]
fn all_distinct_providers_are_called_and_references_are_complete() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let mut req = request(false);
    second_provider(&mut req, &mut host);
    let prepared = core.prepare(req, &host, 1000).unwrap();
    let result = core
        .execute(
            prepared,
            &mut host,
            &mut MemoryJournal::default(),
            &FixedClock(1100),
            &NeverCancel,
        )
        .unwrap();
    assert_eq!(result.calls().len(), 3);
    assert_eq!(
        result.record()["steps"][0]["invocations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_ne!(
        host.invoked[0]["invocation_id"],
        host.invoked[1]["invocation_id"]
    );
    assert_eq!(host.invoked[1]["provider_id"], "second-provider");
}

#[test]
fn command_deny_or_warn_survives_later_allow_and_skips_next_step() {
    for verdict in [Reply::Deny, Reply::Warn] {
        let core = Core::new().unwrap();
        let mut host = Host::new();
        host.replies = vec![verdict, Reply::Allow];
        let mut req = request(true);
        second_provider(&mut req, &mut host);
        let prepared = core.prepare(req, &host, 1000).unwrap();
        let result = core
            .execute(
                prepared,
                &mut host,
                &mut MemoryJournal::default(),
                &FixedClock(1100),
                &NeverCancel,
            )
            .unwrap();
        assert_eq!(result.record()["decision"], "deny");
        assert_eq!(result.calls().len(), 2);
        assert_eq!(result.record()["steps"][1]["outcome"], "skipped");
        assert_eq!(
            result.calls()[1].result().output.as_ref().unwrap()["decision"]["verdict"],
            "allow"
        );
    }
}

#[test]
fn empty_optional_route_records_gap_but_required_route_preserves() {
    for required in [false, true] {
        let core = Core::new().unwrap();
        let mut host = Host::new();
        let mut req = request(false);
        req.plan["steps"][0]["providers"] = json!([]);
        req.plan["steps"][0]["required"] = json!(required);
        req.plan["steps"][0]["on_failure"] = json!(if required {
            "reject_plan"
        } else {
            "record_gap_and_continue"
        });
        let prepared = core.prepare(req, &host, 1000).unwrap();
        let result = core
            .execute(
                prepared,
                &mut host,
                &mut MemoryJournal::default(),
                &FixedClock(1100),
                &NeverCancel,
            )
            .unwrap();
        assert_eq!(result.record()["steps"][0]["outcome"], "gap");
        assert_eq!(result.record()["steps"][0]["invocations"], json!([]));
        assert_eq!(
            result.record()["decision"],
            if required { "preserve" } else { "proceed" }
        );
        assert_eq!(host.invoked.len(), usize::from(!required));
    }
}

#[test]
fn later_malformed_selection_or_input_fails_before_any_call() {
    for provider in [false, true] {
        let core = Core::new().unwrap();
        let host = Host::new();
        let mut req = request(false);
        if provider {
            req.plan["steps"][1]["providers"][0]["provider_version"] = json!("unknown");
        } else {
            req.inputs.get_mut("step-1").unwrap().input["artifact"]["content"] =
                json!("changed source");
        }
        assert!(core.prepare(req, &host, 1000).is_err());
        assert!(host.invoked.is_empty());
    }
}

#[test]
fn named_unavailable_provider_is_not_an_optional_empty_route() {
    let core = Core::new().unwrap();
    let host = Host::new();
    let mut req = request(false);
    req.plan["steps"][0]["required"] = json!(false);
    req.plan["steps"][0]["on_failure"] = json!("record_gap_and_continue");
    req.plan["steps"][0]["providers"][0]["provider_id"] = json!("missing-provider");
    assert!(matches!(
        core.prepare(req, &host, 1000),
        Err(Error::ProviderUnavailable)
    ));
    assert!(host.invoked.is_empty());
}

#[test]
fn changed_descriptor_is_rejected_before_dispatch() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let prepared = core.prepare(request(false), &host, 1000).unwrap();
    host.descriptors.get_mut("fixture-provider").unwrap()["provider_version"] = json!("2");
    assert!(matches!(
        core.execute(
            prepared,
            &mut host,
            &mut MemoryJournal::default(),
            &FixedClock(1100),
            &NeverCancel
        ),
        Err(Error::ProviderChanged)
    ));
    assert!(host.invoked.is_empty());
}

#[test]
fn mismatched_receipt_input_or_output_stops_without_terminal_success() {
    for reply in [Reply::WrongReceipt, Reply::WrongInput, Reply::WrongOutput] {
        let core = Core::new().unwrap();
        let mut host = Host::new();
        host.replies = vec![reply];
        let mut journal = MemoryJournal::default();
        let prepared = core.prepare(request(false), &host, 1000).unwrap();
        assert!(matches!(
            core.execute(
                prepared,
                &mut host,
                &mut journal,
                &FixedClock(1100),
                &NeverCancel
            ),
            Err(Error::Contract(_))
        ));
        assert_eq!(host.invoked.len(), 1);
        assert!(journal
            .records
            .iter()
            .all(|r| r["kind"] != "execution_settled"));
        assert_eq!(journal.claims.len(), 1);
    }
}

#[test]
fn expired_dispatch_deadline_prevents_host_call() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let prepared = core.prepare(request(false), &host, 1000).unwrap();
    assert!(core
        .execute(
            prepared,
            &mut host,
            &mut MemoryJournal::default(),
            &FixedClock(2001),
            &NeverCancel
        )
        .is_err());
    assert!(host.invoked.is_empty());
}

#[test]
fn receipt_cannot_claim_time_outside_observed_call() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    host.replies = vec![Reply::LateReceipt];
    let prepared = core.prepare(request(false), &host, 1000).unwrap();
    assert!(matches!(
        core.execute(
            prepared,
            &mut host,
            &mut MemoryJournal::default(),
            &FixedClock(1100),
            &NeverCancel
        ),
        Err(Error::HostTime)
    ));
    assert_eq!(host.invoked.len(), 1);
}

#[test]
fn cancellation_before_first_call_has_no_receipts() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let prepared = core.prepare(request(false), &host, 1000).unwrap();
    let result = core
        .execute(
            prepared,
            &mut host,
            &mut MemoryJournal::default(),
            &FixedClock(1100),
            &CancelFlag(Rc::new(Cell::new(true))),
        )
        .unwrap();
    assert_eq!(result.record()["decision"], "cancelled");
    assert_eq!(result.record()["steps"][1]["outcome"], "skipped");
    assert!(result.calls().is_empty());
}

#[test]
fn cancellation_inside_multi_provider_step_preserves_partial_evidence() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let flag = Rc::new(Cell::new(false));
    host.cancel_after_call = Some(flag.clone());
    let mut req = request(false);
    second_provider(&mut req, &mut host);
    let prepared = core.prepare(req, &host, 1000).unwrap();
    let result = core
        .execute(
            prepared,
            &mut host,
            &mut MemoryJournal::default(),
            &FixedClock(1100),
            &CancelFlag(flag),
        )
        .unwrap();
    assert_eq!(result.record()["decision"], "cancelled");
    assert_eq!(result.record()["steps"][0]["outcome"], "cancelled");
    assert_eq!(
        result.record()["steps"][0]["invocations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(result.calls().len(), 1);
    assert_eq!(host.invoked.len(), 1);
}

#[test]
fn journal_claim_or_pre_dispatch_append_failure_prevents_call() {
    for (fail_claim, fail_append) in [(true, None), (false, Some(0)), (false, Some(1))] {
        let core = Core::new().unwrap();
        let mut host = Host::new();
        let mut journal = MemoryJournal {
            fail_claim,
            fail_append,
            ..MemoryJournal::default()
        };
        let prepared = core.prepare(request(false), &host, 1000).unwrap();
        assert!(matches!(
            core.execute(
                prepared,
                &mut host,
                &mut journal,
                &FixedClock(1100),
                &NeverCancel
            ),
            Err(Error::Journal(_))
        ));
        assert!(host.invoked.is_empty());
    }
}

#[test]
fn settlement_write_failure_stops_later_calls_and_keeps_claim() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let mut journal = MemoryJournal {
        fail_append: Some(2),
        ..MemoryJournal::default()
    };
    let prepared = core.prepare(request(false), &host, 1000).unwrap();
    assert!(matches!(
        core.execute(
            prepared,
            &mut host,
            &mut journal,
            &FixedClock(1100),
            &NeverCancel
        ),
        Err(Error::Journal(_))
    ));
    assert_eq!(host.invoked.len(), 1);
    assert_eq!(journal.claims.len(), 1);
    let retry = core.prepare(request(false), &host, 1000).unwrap();
    assert!(matches!(
        core.execute(
            retry,
            &mut host,
            &mut journal,
            &FixedClock(1100),
            &NeverCancel
        ),
        Err(Error::Journal(JournalError::AlreadyClaimed))
    ));
    assert_eq!(host.invoked.len(), 1);
}

#[test]
fn completed_event_cannot_be_reexecuted_with_a_changed_plan() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let mut journal = MemoryJournal::default();
    let first = core.prepare(request(false), &host, 1000).unwrap();
    core.execute(
        first,
        &mut host,
        &mut journal,
        &FixedClock(1100),
        &NeverCancel,
    )
    .unwrap();
    let mut req = request(false);
    req.plan["revision"] = json!(2);
    let second = core.prepare(req, &host, 1000).unwrap();
    assert!(matches!(
        core.execute(
            second,
            &mut host,
            &mut journal,
            &FixedClock(1100),
            &NeverCancel
        ),
        Err(Error::Journal(JournalError::AlreadyClaimed))
    ));
    assert_eq!(host.invoked.len(), 2);
}

#[test]
fn scopes_have_separate_claims_and_wrong_runtime_binding_is_rejected() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let mut journal = MemoryJournal::default();
    let first = core.prepare(request(false), &host, 1000).unwrap();
    let first_key = first.event_key().to_owned();
    core.execute(
        first,
        &mut host,
        &mut journal,
        &FixedClock(1100),
        &NeverCancel,
    )
    .unwrap();
    let mut req = request(false);
    req.plan["scope"]["session_id"] = json!("other-session");
    assert!(core.prepare(req, &host, 1000).is_err());
    let mut req = request(false);
    req.plan["scope"]["session_id"] = json!("other-session");
    req.runtime["session_id"] = json!("other-session");
    let second = core.prepare(req, &host, 1000).unwrap();
    assert_ne!(first_key, second.event_key());
    let result = core
        .execute(
            second,
            &mut host,
            &mut journal,
            &FixedClock(1100),
            &NeverCancel,
        )
        .unwrap();
    assert_eq!(result.record()["scope"]["session_id"], "other-session");
    assert!(result
        .calls()
        .iter()
        .all(|call| call.result().receipt["scope"]["session_id"] == "other-session"));
    assert_eq!(journal.claims.len(), 2);
}

#[test]
fn failed_receipt_is_a_visible_gap_and_transport_error_is_interrupted() {
    for reply in [Reply::Failed, Reply::Transport] {
        let core = Core::new().unwrap();
        let mut host = Host::new();
        host.replies = vec![reply];
        let mut journal = MemoryJournal::default();
        let prepared = core.prepare(request(false), &host, 1000).unwrap();
        let result = core.execute(
            prepared,
            &mut host,
            &mut journal,
            &FixedClock(1100),
            &NeverCancel,
        );
        if matches!(reply, Reply::Failed) {
            let result = result.unwrap();
            assert_eq!(result.record()["decision"], "preserve");
            assert_eq!(result.record()["steps"][0]["outcome"], "gap");
            assert!(result.calls()[0].result().output.is_none());
        } else {
            assert!(matches!(result, Err(Error::Host(_))));
            assert!(journal
                .records
                .iter()
                .all(|r| r["kind"] != "execution_settled"));
        }
        assert_eq!(host.invoked.len(), 1);
    }
}

#[test]
fn terminal_journal_failure_never_returns_a_usable_execution() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let mut journal = MemoryJournal {
        fail_append: Some(8),
        ..MemoryJournal::default()
    };
    let prepared = core.prepare(request(false), &host, 1000).unwrap();
    assert!(matches!(
        core.execute(
            prepared,
            &mut host,
            &mut journal,
            &FixedClock(1100),
            &NeverCancel
        ),
        Err(Error::Journal(_))
    ));
    assert_eq!(host.invoked.len(), 2);
    assert_eq!(journal.records.last().unwrap()["kind"], "step_settled");
    assert_eq!(journal.claims.len(), 1);
}

#[test]
fn cancellation_during_final_host_call_cannot_return_proceed() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let flag = Rc::new(Cell::new(false));
    host.cancel_after_call = Some(flag.clone());
    let mut req = request(false);
    req.plan["steps"].as_array_mut().unwrap().remove(0);
    req.inputs.remove("step-0");
    let prepared = core.prepare(req, &host, 1000).unwrap();
    let result = core
        .execute(
            prepared,
            &mut host,
            &mut MemoryJournal::default(),
            &FixedClock(1100),
            &CancelFlag(flag),
        )
        .unwrap();
    assert_eq!(result.record()["decision"], "cancelled");
    assert_eq!(result.record()["steps"][0]["outcome"], "cancelled");
    assert_eq!(result.calls().len(), 1);
    assert!(result.calls()[0]
        .result()
        .output
        .as_ref()
        .unwrap()
        .get("candidate")
        .is_some());
}

struct SharedClock(Rc<Cell<u64>>);
impl Clock for SharedClock {
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
}

#[derive(Default)]
struct InterleavingJournal {
    inner: MemoryJournal,
    advance_on_start: Option<(Rc<Cell<u64>>, u64)>,
    cancel_on_start: Option<Rc<Cell<bool>>>,
}
impl Journal for InterleavingJournal {
    fn claim(&mut self, event_key: &str, plan: &Value) -> Result<Value, JournalError> {
        self.inner.claim(event_key, plan)
    }

    fn append(&mut self, event_key: &str, record: &Value) -> Result<Value, JournalError> {
        let evidence = self.inner.append(event_key, record)?;
        if record["kind"] == "invocation_started" {
            // Simulate an acknowledged fsync overlapping time or cancellation.
            if let Some((clock, now)) = self.advance_on_start.take() {
                clock.set(now);
            }
            if let Some(cancelled) = self.cancel_on_start.take() {
                cancelled.set(true);
            }
        }
        Ok(evidence)
    }
}

struct TimedHost {
    inner: Host,
    clock: Rc<Cell<u64>>,
    call_duration: u64,
}
impl ProviderHost for TimedHost {
    fn descriptor(&self, id: &str) -> Option<&Value> {
        self.inner.descriptor(id)
    }

    fn invoke(&mut self, invocation: &Value) -> Result<ProviderResult, HostError> {
        let start = self.clock.get();
        let mut result = self.inner.invoke(invocation)?;
        self.clock.set(start + self.call_duration);
        result.receipt["started_at_ms"] = json!(start);
        result.receipt["completed_at_ms"] = json!(self.clock.get());
        Ok(result)
    }
}

#[test]
fn journal_sync_crossing_deadline_prevents_actual_dispatch() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let clock = Rc::new(Cell::new(1100));
    let mut journal = InterleavingJournal {
        advance_on_start: Some((clock.clone(), 2001)),
        ..InterleavingJournal::default()
    };
    let prepared = core.prepare(request(false), &host, 1000).unwrap();
    assert!(matches!(
        core.execute(
            prepared,
            &mut host,
            &mut journal,
            &SharedClock(clock),
            &NeverCancel,
        ),
        Err(Error::Contract(_))
    ));
    assert!(host.invoked.is_empty());
    assert_eq!(journal.inner.claims.len(), 1);
    assert_eq!(
        journal.inner.records.last().unwrap()["kind"],
        "invocation_started"
    );
    assert!(journal
        .inner
        .records
        .iter()
        .all(|record| record["kind"] != "invocation_settled"));
}

#[test]
fn journal_sync_duration_does_not_consume_host_wall_time_budget() {
    let core = Core::new().unwrap();
    let clock = Rc::new(Cell::new(1100));
    let mut host = TimedHost {
        inner: Host::new(),
        clock: clock.clone(),
        call_duration: 5,
    };
    let mut journal = InterleavingJournal {
        advance_on_start: Some((clock.clone(), 1600)),
        ..InterleavingJournal::default()
    };
    let mut req = request(false);
    for input in req.inputs.values_mut() {
        input.budget["wall_time_ms"] = json!(50);
    }
    let prepared = core.prepare(req, &host, 1000).unwrap();
    let result = core
        .execute(
            prepared,
            &mut host,
            &mut journal,
            &SharedClock(clock.clone()),
            &NeverCancel,
        )
        .unwrap();
    assert_eq!(result.record()["decision"], "proceed");
    assert_eq!(host.inner.invoked.len(), 2);
    assert_eq!(result.calls()[0].result().receipt["started_at_ms"], 1600);
    assert_eq!(result.calls()[0].result().receipt["completed_at_ms"], 1605);
    assert_eq!(clock.get(), 1610);
}

#[test]
fn cancellation_during_start_sync_prevents_call_without_fabricating_receipt() {
    let core = Core::new().unwrap();
    let mut host = Host::new();
    let flag = Rc::new(Cell::new(false));
    let mut journal = InterleavingJournal {
        cancel_on_start: Some(flag.clone()),
        ..InterleavingJournal::default()
    };
    let prepared = core.prepare(request(false), &host, 1000).unwrap();
    let result = core
        .execute(
            prepared,
            &mut host,
            &mut journal,
            &FixedClock(1100),
            &CancelFlag(flag),
        )
        .unwrap();
    assert_eq!(result.record()["decision"], "cancelled");
    assert_eq!(result.record()["steps"][0]["outcome"], "cancelled");
    assert_eq!(result.record()["steps"][1]["outcome"], "skipped");
    assert_eq!(result.record()["steps"][0]["invocations"], json!([]));
    assert!(result.calls().is_empty());
    assert!(host.invoked.is_empty());
    assert!(journal
        .inner
        .records
        .iter()
        .any(|record| record["kind"] == "invocation_started"));
    assert!(journal
        .inner
        .records
        .iter()
        .all(|record| record["kind"] != "invocation_settled"));
}
