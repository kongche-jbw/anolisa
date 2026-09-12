//! Real TokenlessHost transport remains subordinate to Core safety and journaling.

mod common;

use aw_contracts::{canonical, Registry};
use aw_core::{
    journal::FileJournal,
    ports::{Clock, HostError, NeverCancel, ProviderHost, ProviderResult},
    Core, PrepareRequest, StepInput,
};
use aw_tokenless_host::TokenlessHost;
use common::{invocation, Fixture};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

const SOURCE: &str = "original tool output with substantial repeated padding\n";

struct WallClock;
impl Clock for WallClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }
}

fn fixtures() -> Value {
    canonical::parse(include_bytes!("../../../tests/fixtures/contracts.json")).unwrap()
}

fn request(host: &TokenlessHost) -> PrepareRequest {
    let fixture = fixtures();
    let call = invocation(host, SOURCE);
    let mut plan = fixture["capability-plan-v1"].clone();
    let selected = &mut plan["steps"][0]["providers"][0];
    for key in ["provider_id", "provider_version", "manifest_digest"] {
        selected[key] = call[key].clone();
    }
    plan["source_digest"] = call["input"]["artifact"]["digest"].clone();
    let mut boundary = fixture["boundary-descriptor-v1"].clone();
    boundary["reversibility"] = json!(["unrecoverable"]);
    PrepareRequest {
        plan,
        boundary,
        runtime: fixture["runtime-binding-v1"].clone(),
        inputs: BTreeMap::from([(
            "project-1".into(),
            StepInput {
                input: call["input"].clone(),
                budget: call["budget"].clone(),
                deadline_at_ms: call["deadline_at_ms"].as_u64().unwrap(),
            },
        )]),
    }
}

#[test]
fn real_host_candidate_is_journaled_without_adoption() {
    let fixture = Fixture::new("applied");
    let mut host = TokenlessHost::new(fixture.config.clone()).unwrap();
    let core = Core::new().unwrap();
    let prepared = core
        .prepare(request(&host), &host, WallClock.now_ms())
        .unwrap();
    let key = prepared.event_key().to_owned();
    let mut journal = FileJournal::new(fixture.path.join("journal")).unwrap();
    let execution = core
        .execute(prepared, &mut host, &mut journal, &WallClock, &NeverCancel)
        .unwrap();
    assert!(fixture.called());
    assert_eq!(execution.record()["decision"], "proceed");
    assert_eq!(
        execution.calls()[0].invocation()["input"]["artifact"]["content"],
        SOURCE
    );
    let output = execution.calls()[0].result().output.as_ref().unwrap();
    assert_eq!(output["candidate"]["content"], "done\n");
    assert_eq!(output["candidate"]["recovery"], json!({"mode":"none"}));
    assert!(execution.record().get("adoption").is_none());
    let stored = journal
        .read_verified(&key, execution.journal_ack())
        .unwrap();
    assert_eq!(
        stored.last().unwrap()["record"]["execution"],
        *execution.record()
    );
}

#[test]
fn failed_or_no_savings_projection_preserves_original_in_core() {
    for (mode, disposition) in [
        ("failure", "failed"),
        ("passthrough", "bypassed"),
        ("no_savings", "bypassed"),
    ] {
        let fixture = Fixture::new(mode);
        let mut host = TokenlessHost::new(fixture.config.clone()).unwrap();
        let core = Core::new().unwrap();
        let prepared = core
            .prepare(request(&host), &host, WallClock.now_ms())
            .unwrap();
        let key = prepared.event_key().to_owned();
        let mut journal = FileJournal::new(fixture.path.join("journal")).unwrap();
        let execution = core
            .execute(prepared, &mut host, &mut journal, &WallClock, &NeverCancel)
            .unwrap();
        assert!(fixture.called());
        assert_eq!(execution.record()["decision"], "preserve", "{mode}");
        let call = &execution.calls()[0];
        assert_eq!(call.result().receipt["disposition"], disposition);
        assert!(call.result().output.is_none());
        assert_eq!(call.invocation()["input"]["artifact"]["content"], SOURCE);
        journal
            .read_verified(&key, execution.journal_ack())
            .unwrap();
    }
}

struct SecurityFirst {
    projection: TokenlessHost,
    security: Value,
    inspections: usize,
}
impl ProviderHost for SecurityFirst {
    fn descriptor(&self, id: &str) -> Option<&Value> {
        if self.security["provider_id"] == id {
            Some(&self.security)
        } else {
            self.projection.descriptor(id)
        }
    }
    fn invoke(&mut self, invocation: &Value) -> Result<ProviderResult, HostError> {
        if invocation["provider_id"] != self.security["provider_id"] {
            return self.projection.invoke(invocation);
        }
        self.inspections += 1;
        let now = WallClock.now_ms();
        let mut receipt = json!({"disposition":"failed","error_code":"fixture_security_unavailable",
            "started_at_ms":now,"completed_at_ms":now,"meters":[],"evidence":[]});
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
        Ok(ProviderResult {
            receipt,
            output: None,
        })
    }
}

#[test]
fn required_security_failure_skips_the_native_projection() {
    let fixture = Fixture::new("applied");
    let projection = TokenlessHost::new(fixture.config.clone()).unwrap();
    let mut request = request(&projection);
    let mut security = fixtures()["provider-descriptor-v1"].clone();
    security["provider_id"] = json!("security-first");
    let mut step = request.plan["steps"][0].clone();
    step["step_id"] = json!("inspect-first");
    step["capability"] = json!("security.content.inspect/v2");
    let registry = Registry::new().unwrap();
    step["input_schema"] = registry
        .reference("security-content-inspect-input-v2")
        .unwrap();
    step["output_schema"] = registry
        .reference("security-content-inspect-output-v2")
        .unwrap();
    for key in ["provider_id", "provider_version", "manifest_digest"] {
        step["providers"][0][key] = security[key].clone();
    }
    request.plan["steps"]
        .as_array_mut()
        .unwrap()
        .insert(0, step);
    let projection_input = &request.inputs["project-1"];
    let mut input = fixtures()["security-content-inspect-input-v2"].clone();
    input["artifact"] = projection_input.input["artifact"].clone();
    request.inputs.insert(
        "inspect-first".into(),
        StepInput {
            input,
            budget: projection_input.budget.clone(),
            deadline_at_ms: projection_input.deadline_at_ms,
        },
    );
    let mut host = SecurityFirst {
        projection,
        security,
        inspections: 0,
    };
    let core = Core::new().unwrap();
    let prepared = core.prepare(request, &host, WallClock.now_ms()).unwrap();
    let key = prepared.event_key().to_owned();
    let mut journal = FileJournal::new(fixture.path.join("journal")).unwrap();
    let execution = core
        .execute(prepared, &mut host, &mut journal, &WallClock, &NeverCancel)
        .unwrap();
    assert_eq!(host.inspections, 1);
    assert!(!fixture.called());
    assert_eq!(execution.calls().len(), 1);
    assert_eq!(execution.record()["decision"], "preserve");
    assert_eq!(execution.record()["steps"][1]["outcome"], "skipped");
    journal
        .read_verified(&key, execution.journal_ack())
        .unwrap();
}
