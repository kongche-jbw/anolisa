//! Real Core integration with synthetic Host responses and an in-memory journal.
//! Passing these tests does not establish native registration or enforcement.

use aw_adapters::{
    Adapter, CaptureRequest, CapturedEvent, Error, Host, NativeContext, StepOptions,
};
use aw_contracts::{canonical, Registry};
use aw_core::ports::{
    Clock, HostError, Journal, JournalError, NeverCancel, ProviderHost, ProviderResult,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const HOSTS: [Host; 6] = [
    Host::Qoder,
    Host::Codex,
    Host::QwenCode,
    Host::Hermes,
    Host::OpenClaw,
    Host::Cosh,
];

fn fixture() -> Value {
    canonical::parse(include_bytes!("../../../tests/fixtures/contracts.json")).unwrap()
}

fn request(host: Host, pre: bool) -> CaptureRequest {
    let f = fixture();
    let phase = if pre { "PreToolUse" } else { "PostToolUse" };
    let (event, mut payload) = match host {
        Host::Hermes => (
            if pre {
                "pre_tool_call"
            } else {
                "post_tool_call"
            },
            json!({"event":{"tool_name":"terminal","args":{"command":"printf hello"},"result":"hello\n"},"context":{"session_id":"session-1","tool_call_id":"tool-1"}}),
        ),
        Host::OpenClaw => (
            if pre {
                "before_tool_call"
            } else {
                "after_tool_call"
            },
            json!({"event":{"toolName":"exec","params":{"command":"printf hello"},"result":"hello\n","toolCallId":"tool-1"},"context":{"sessionId":"session-1"}}),
        ),
        _ => (
            phase,
            json!({"hook_event_name":phase,"tool_name":if matches!(host, Host::Qoder | Host::Codex) {"Bash"} else {"run_shell_command"},"tool_input":{"command":"printf hello"},"tool_response":"hello\n","session_id":"session-1","tool_use_id":"tool-1"}),
        ),
    };
    if matches!(host, Host::Cosh) && !pre {
        payload["tool_response"] =
            json!({"llmContent":"hello\n","returnDisplay":{"summary":"native display"}});
    }
    if matches!(host, Host::Codex) {
        payload["turn_id"] = json!("turn-1");
    } else if matches!(host, Host::OpenClaw) {
        payload["context"]["runId"] = json!("turn-1");
    }
    payload["additional_metadata"] = json!({"opaque":[1,true,null],"native_only":"kept"});
    // Raw native JSON is not an AW canonical document: these keys remain legal.
    payload["未解释的键"] = json!({"任意字段":"保留原文"});
    CaptureRequest {
        native_event: event.into(),
        payload,
        context: NativeContext {
            scope: f["capability-plan-v1"]["scope"].clone(),
            runtime: f["runtime-binding-v1"].clone(),
            event_id: "native-event-1".into(),
        },
    }
}

fn plan(captured: &CapturedEvent) -> Value {
    let f = fixture();
    let registry = Registry::new().unwrap();
    let mut plan = f["capability-plan-v1"].clone();
    plan["scope"] = captured.scope().clone();
    plan["event_id"] = json!(captured.event_id());
    plan["source_digest"] = captured.artifact()["digest"].clone();
    for (target, source) in [
        ("boundary_id", "boundary_id"),
        ("boundary_revision", "revision"),
        ("boundary", "boundary"),
    ] {
        plan[target] = captured.boundary()[source].clone();
    }
    let step = &mut plan["steps"][0];
    step["step_id"] = json!("inspect-1");
    step["capability"] = json!("security.content.inspect/v2");
    step["input_schema"] = registry
        .reference("security-content-inspect-input-v2")
        .unwrap();
    step["output_schema"] = registry
        .reference("security-content-inspect-output-v2")
        .unwrap();
    plan
}

fn options() -> BTreeMap<String, StepOptions> {
    BTreeMap::from([(
        "inspect-1".into(),
        StepOptions {
            constraints: json!({"include_low_confidence":false}),
            budget: fixture()["capability-invocation-v1"]["budget"].clone(),
            deadline_at_ms: 2000,
        },
    )])
}

struct FixedClock;
impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        1100
    }
}

#[derive(Default)]
struct MemoryJournal {
    claims: BTreeSet<String>,
    active: BTreeSet<String>,
    records: Vec<Value>,
}
impl Journal for MemoryJournal {
    fn release(&mut self, key: &str) {
        self.active.remove(key);
    }

    fn claim(&mut self, key: &str, plan: &Value) -> Result<Value, JournalError> {
        if !self.claims.insert(key.into()) {
            return Err(JournalError::AlreadyClaimed);
        }
        self.active.insert(key.into());
        Ok(
            json!({"source_id":"synthetic-memory-journal","record_id":"claim","digest":canonical::document_digest(plan).unwrap()}),
        )
    }
    fn append(&mut self, key: &str, record: &Value) -> Result<Value, JournalError> {
        if !self.active.contains(key) {
            return Err(JournalError::InvalidRecord);
        }
        self.records.push(record.clone());
        Ok(
            json!({"source_id":"synthetic-memory-journal","record_id":format!("record-{}",self.records.len()),"digest":canonical::document_digest(record).unwrap()}),
        )
    }
}

#[derive(Clone, Copy)]
enum Reply {
    Produced,
    Failed,
    Transport,
    WrongInput,
}
struct SyntheticHost {
    descriptor: Value,
    calls: Vec<Value>,
    reply: Reply,
}
impl SyntheticHost {
    fn new(reply: Reply) -> Self {
        Self {
            descriptor: fixture()["provider-descriptor-v1"].clone(),
            calls: vec![],
            reply,
        }
    }
}
impl ProviderHost for SyntheticHost {
    fn descriptor(&self, id: &str) -> Option<&Value> {
        (self.descriptor["provider_id"] == id).then_some(&self.descriptor)
    }
    fn invoke(&mut self, invocation: &Value) -> Result<ProviderResult, HostError> {
        self.calls.push(invocation.clone());
        if matches!(self.reply, Reply::Transport) {
            return Err(HostError {
                code: "synthetic_transport_failure".into(),
            });
        }
        let f = fixture();
        let mut receipt = f["provider-receipt-v1"].clone();
        for key in [
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
            receipt[key] = invocation[key].clone();
        }
        receipt["started_at_ms"] = json!(1100);
        receipt["completed_at_ms"] = json!(1100);
        if matches!(self.reply, Reply::Failed) {
            receipt["disposition"] = json!("failed");
            receipt["error_code"] = json!("synthetic_failure");
            receipt.as_object_mut().unwrap().remove("output");
            return Ok(ProviderResult {
                receipt,
                output: None,
            });
        }
        let schema = if invocation["capability"] == "security.code.inspect/v2" {
            "security-code-inspect-output-v2"
        } else {
            "security-content-inspect-output-v2"
        };
        let mut output = f[schema].clone();
        let artifact = &invocation["input"]["artifact"];
        let bytes = artifact["content"].as_str().unwrap().len();
        let coverage = &mut output["inspection"]["coverage"];
        coverage["input_digest"] = artifact["digest"].clone();
        coverage["input_bytes"] = json!(bytes);
        coverage["scanned_bytes"] = json!(bytes);
        if invocation["capability"] == "security.code.inspect/v2"
            && invocation["input"]["constraints"]["language"] != "auto"
        {
            coverage["languages"] = json!([invocation["input"]["constraints"]["language"]]);
        }
        if matches!(self.reply, Reply::WrongInput) {
            coverage["input_digest"] = json!("0".repeat(64));
        }
        receipt["output"] = json!({"schema":invocation["output_schema"],"digest":canonical::document_digest(&output).unwrap(),"bytes":canonical::bytes(&output).unwrap().len()});
        Ok(ProviderResult {
            receipt,
            output: Some(output),
        })
    }
}

#[test]
fn all_six_hosts_invoke_real_core_once_and_keep_native_payload() {
    for host_id in HOSTS {
        let adapter = Adapter::new(host_id).unwrap();
        let request = request(host_id, false);
        let original = request.payload.clone();
        let captured = adapter.capture(request).unwrap();
        assert_eq!(captured.native_payload(), &original);
        let mut host = SyntheticHost::new(Reply::Produced);
        let selected = plan(&captured);
        let prepared = adapter
            .prepare(captured, selected, options(), &host, 1000)
            .unwrap();
        let mut journal = MemoryJournal::default();
        let result = adapter
            .execute(prepared, &mut host, &mut journal, &FixedClock, &NeverCancel)
            .unwrap();
        assert_eq!(result.host(), host_id);
        assert_eq!(result.native_payload(), &original);
        assert_eq!(host.calls.len(), 1);
        assert_eq!(result.execution().calls().len(), 1);
        assert_eq!(result.execution().record()["decision"], "proceed");
        let call = &result.execution().calls()[0];
        assert_eq!(
            call.result().receipt["input_digest"],
            call.invocation()["input_digest"]
        );
        assert_eq!(call.invocation()["input"]["artifact"]["content"], "hello\n");
        assert_eq!(
            result.execution().journal_ack()["source_id"],
            "synthetic-memory-journal"
        );
    }
}

#[test]
fn stale_native_session_and_tool_ids_are_rejected() {
    for host in HOSTS {
        for tool in [false, true] {
            let adapter = Adapter::new(host).unwrap();
            let mut req = request(host, false);
            match (host, tool) {
                (Host::Hermes, false) => {
                    req.payload["context"]["session_id"] = json!("old-session")
                }
                (Host::Hermes, true) => req.payload["context"]["tool_call_id"] = json!("old-tool"),
                (Host::OpenClaw, false) => {
                    req.payload["context"]["sessionId"] = json!("old-session")
                }
                (Host::OpenClaw, true) => req.payload["event"]["toolCallId"] = json!("old-tool"),
                (_, false) => req.payload["session_id"] = json!("old-session"),
                (_, true) => req.payload["tool_use_id"] = json!("old-tool"),
            }
            assert!(matches!(
                adapter.capture(req),
                Err(Error::IdentityMismatch(_))
            ));
        }
    }
}

#[test]
fn observable_native_turn_cannot_be_relabelled_by_context() {
    for host in [Host::Codex, Host::OpenClaw] {
        let adapter = Adapter::new(host).unwrap();
        let mut req = request(host, false);
        req.context.scope["turn_id"] = json!("another-turn");
        assert!(matches!(
            adapter.capture(req),
            Err(Error::IdentityMismatch(_))
        ));
    }
}

#[test]
fn changed_runtime_incarnation_is_rejected_during_capture() {
    for (key, value) in [
        ("generation", json!(2)),
        ("binding_revision", json!(2)),
        ("state", json!("exited")),
        ("session_id", json!("old-session")),
    ] {
        let adapter = Adapter::new(Host::Qoder).unwrap();
        let mut req = request(Host::Qoder, false);
        req.context.runtime[key] = value;
        assert!(adapter.capture(req).is_err(), "{key}");
    }
}

#[test]
fn changed_plan_identity_or_source_never_invokes_provider() {
    let adapter = Adapter::new(Host::Qoder).unwrap();
    let host = SyntheticHost::new(Reply::Produced);
    for (pointer, value) in [
        ("/source_digest", json!("0".repeat(64))),
        ("/scope/session_id", json!("other")),
        ("/event_id", json!("other")),
        ("/boundary_id", json!("other")),
        ("/boundary_revision", json!(1)),
        ("/boundary", json!("pre_tool")),
    ] {
        let captured = adapter.capture(request(Host::Qoder, false)).unwrap();
        let mut plan = plan(&captured);
        *plan.pointer_mut(pointer).unwrap() = value;
        assert!(
            adapter
                .prepare(captured, plan, options(), &host, 1000)
                .is_err(),
            "{pointer}"
        );
        assert!(host.calls.is_empty());
    }
}

#[test]
fn snapshot_and_prepared_execution_cannot_cross_hosts() {
    let qoder = Adapter::new(Host::Qoder).unwrap();
    let codex = Adapter::new(Host::Codex).unwrap();
    let mut host = SyntheticHost::new(Reply::Produced);
    let captured = qoder.capture(request(Host::Qoder, false)).unwrap();
    let selected = plan(&captured);
    assert!(matches!(
        codex.prepare(captured, selected, options(), &host, 1000),
        Err(Error::IdentityMismatch("adapter host"))
    ));
    let captured = qoder.capture(request(Host::Qoder, false)).unwrap();
    let selected = plan(&captured);
    let prepared = qoder
        .prepare(captured, selected, options(), &host, 1000)
        .unwrap();
    let mut journal = MemoryJournal::default();
    assert!(matches!(
        codex.execute(prepared, &mut host, &mut journal, &FixedClock, &NeverCancel),
        Err(Error::IdentityMismatch("adapter host"))
    ));
    assert!(host.calls.is_empty());
    assert!(journal.claims.is_empty());
}

#[test]
fn required_provider_failure_preserves_source_and_failed_receipt() {
    let adapter = Adapter::new(Host::Qoder).unwrap();
    let captured = adapter.capture(request(Host::Qoder, false)).unwrap();
    let original = captured.native_payload().clone();
    let selected = plan(&captured);
    let mut host = SyntheticHost::new(Reply::Failed);
    let prepared = adapter
        .prepare(captured, selected, options(), &host, 1000)
        .unwrap();
    let result = adapter
        .execute(
            prepared,
            &mut host,
            &mut MemoryJournal::default(),
            &FixedClock,
            &NeverCancel,
        )
        .unwrap();
    assert_eq!(result.execution().record()["decision"], "preserve");
    assert_eq!(result.native_payload(), &original);
    assert_eq!(
        result.execution().calls()[0].result().receipt["disposition"],
        "failed"
    );
    assert!(result.execution().calls()[0].result().output.is_none());
}

#[test]
fn provider_transport_error_never_settles_success() {
    let adapter = Adapter::new(Host::Qoder).unwrap();
    let captured = adapter.capture(request(Host::Qoder, false)).unwrap();
    let selected = plan(&captured);
    let mut host = SyntheticHost::new(Reply::Transport);
    let mut journal = MemoryJournal::default();
    let prepared = adapter
        .prepare(captured, selected, options(), &host, 1000)
        .unwrap();
    assert!(matches!(
        adapter.execute(prepared, &mut host, &mut journal, &FixedClock, &NeverCancel),
        Err(Error::Core(aw_core::Error::Host(_)))
    ));
    assert_eq!(host.calls.len(), 1);
    assert_eq!(journal.claims.len(), 1);
    assert!(journal
        .records
        .iter()
        .all(|r| r["kind"] != "execution_settled"));
}

#[test]
fn duplicate_native_occurrence_keeps_core_reservation() {
    let adapter = Adapter::new(Host::Qoder).unwrap();
    let mut host = SyntheticHost::new(Reply::Produced);
    let mut journal = MemoryJournal::default();
    for duplicate in [false, true] {
        let captured = adapter.capture(request(Host::Qoder, false)).unwrap();
        let selected = plan(&captured);
        let prepared = adapter
            .prepare(captured, selected, options(), &host, 1000)
            .unwrap();
        let result = adapter.execute(prepared, &mut host, &mut journal, &FixedClock, &NeverCancel);
        if duplicate {
            assert!(matches!(
                result,
                Err(Error::Core(aw_core::Error::Journal(
                    JournalError::AlreadyClaimed
                )))
            ));
        } else {
            assert!(result.is_ok());
        }
    }
    assert_eq!(host.calls.len(), 1);
}

#[test]
fn output_coverage_must_bind_to_captured_text() {
    let adapter = Adapter::new(Host::Qoder).unwrap();
    let captured = adapter.capture(request(Host::Qoder, false)).unwrap();
    let selected = plan(&captured);
    let mut host = SyntheticHost::new(Reply::WrongInput);
    let prepared = adapter
        .prepare(captured, selected, options(), &host, 1000)
        .unwrap();
    assert!(adapter
        .execute(
            prepared,
            &mut host,
            &mut MemoryJournal::default(),
            &FixedClock,
            &NeverCancel
        )
        .is_err());
    assert_eq!(host.calls.len(), 1);
}

#[test]
fn core_inspection_does_not_manufacture_native_adoption() {
    let adapter = Adapter::new(Host::Qoder).unwrap();
    let captured = adapter.capture(request(Host::Qoder, false)).unwrap();
    let selected = plan(&captured);
    let mut host = SyntheticHost::new(Reply::Produced);
    let mut journal = MemoryJournal::default();
    let prepared = adapter
        .prepare(captured, selected, options(), &host, 1000)
        .unwrap();
    let result = adapter
        .execute(prepared, &mut host, &mut journal, &FixedClock, &NeverCancel)
        .unwrap();
    // Admitting a proof boundary does not create evidence of an observation.
    assert_eq!(
        result.execution().boundary()["proof_boundaries"],
        json!(["local_history"])
    );
    assert!(result.execution().record().get("adoption").is_none());
    assert!(journal
        .records
        .iter()
        .all(|r| r["kind"].as_str().is_none_or(|k| !k.contains("adoption"))));
}

#[test]
fn pre_tool_profiles_cannot_be_promoted_to_final_guards() {
    let registry = Registry::new().unwrap();
    for host_id in HOSTS {
        let adapter = Adapter::new(host_id).unwrap();
        let captured = adapter.capture(request(host_id, true)).unwrap();
        assert_eq!(captured.boundary()["has_final_input_guard"], false);
        let mut selected = plan(&captured);
        selected["os_requirement"] = json!({
            "policy_digest":fixture()["execution-intent-v1"]["protection_policy_digest"],
            "required_controls":["filesystem.access/v1"]
        });
        selected["steps"][0]["capability"] = json!("security.command.inspect/v2");
        selected["steps"][0]["input_schema"] = registry
            .reference("security-command-inspect-input-v2")
            .unwrap();
        selected["steps"][0]["output_schema"] = registry
            .reference("security-command-inspect-output-v2")
            .unwrap();
        selected["steps"][0]["on_failure"] = json!("deny_dispatch");
        // An otherwise well-formed command plan still lacks native finality.
        registry.validate("capability-plan-v1", &selected).unwrap();
        let host = SyntheticHost::new(Reply::Produced);
        assert!(matches!(
            adapter.prepare(captured, selected, options(), &host, 1000),
            Err(Error::Contract(aw_contracts::Error::Invariant(
                "pre-tool plan requires a non-bypassable command gate"
            )))
        ));
        assert!(host.calls.is_empty());
    }
}

#[test]
fn options_must_cover_exactly_the_pinned_plan() {
    let adapter = Adapter::new(Host::Qoder).unwrap();
    let host = SyntheticHost::new(Reply::Produced);
    for extra in [false, true] {
        let captured = adapter.capture(request(Host::Qoder, false)).unwrap();
        let selected = plan(&captured);
        let mut opts = options();
        if extra {
            opts.insert("unplanned".into(), options().remove("inspect-1").unwrap());
        } else {
            opts.clear();
        }
        assert!(adapter
            .prepare(captured, selected, opts, &host, 1000)
            .is_err());
        assert!(host.calls.is_empty());
    }
}

#[test]
fn code_inspection_binds_language_and_exact_source_coverage() {
    let adapter = Adapter::new(Host::Qoder).unwrap();
    let registry = Registry::new().unwrap();
    for reply in [Reply::Produced, Reply::WrongInput] {
        let mut req = request(Host::Qoder, false);
        let text = "print('hello')\n";
        req.payload["tool_name"] = json!("Read");
        req.payload["tool_response"] = json!(text);
        let captured = adapter.capture(req).unwrap();
        let mut selected = plan(&captured);
        let step = &mut selected["steps"][0];
        step["capability"] = json!("security.code.inspect/v2");
        step["input_schema"] = registry
            .reference("security-code-inspect-input-v2")
            .unwrap();
        step["output_schema"] = registry
            .reference("security-code-inspect-output-v2")
            .unwrap();
        let mut opts = options();
        opts.get_mut("inspect-1").unwrap().constraints = json!({"language":"python"});
        let mut host = SyntheticHost::new(reply);
        let prepared = adapter
            .prepare(captured, selected, opts, &host, 1000)
            .unwrap();
        let result = adapter.execute(
            prepared,
            &mut host,
            &mut MemoryJournal::default(),
            &FixedClock,
            &NeverCancel,
        );
        assert_eq!(host.calls.len(), 1);
        assert_eq!(
            host.calls[0]["input"]["constraints"],
            json!({"language":"python"})
        );
        assert_eq!(host.calls[0]["input"]["artifact"]["content"], text);
        if matches!(reply, Reply::WrongInput) {
            assert!(result.is_err());
        } else {
            let result = result.unwrap();
            let output = result.execution().calls()[0]
                .result()
                .output
                .as_ref()
                .unwrap();
            let coverage = &output["inspection"]["coverage"];
            assert_eq!(coverage["input_digest"], canonical::digest(text.as_bytes()));
            assert_eq!(coverage["input_bytes"], text.len());
            assert_eq!(coverage["scanned_bytes"], text.len());
            assert_eq!(coverage["languages"], json!(["python"]));
        }
    }
}
