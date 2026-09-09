//! Synthetic Qoder-shaped capture through real AW Core and a Linux file journal.
//! No Qoder process, native plugin, SecCore scanner or adoption is exercised.

use aw_adapters::{Adapter, CaptureRequest, Host, NativeContext, StepOptions};
use aw_contracts::{canonical, Registry};
use aw_core::{
    journal::FileJournal,
    ports::{Clock, HostError, NeverCancel, ProviderHost, ProviderResult},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

struct SyntheticHost {
    descriptor: Value,
    receipt: Value,
    output: Value,
}
impl ProviderHost for SyntheticHost {
    fn descriptor(&self, id: &str) -> Option<&Value> {
        (self.descriptor["provider_id"] == id).then_some(&self.descriptor)
    }
    fn invoke(&mut self, invocation: &Value) -> Result<ProviderResult, HostError> {
        let mut receipt = self.receipt.clone();
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
        let artifact = &invocation["input"]["artifact"];
        let text = artifact["content"].as_str().ok_or_else(|| HostError {
            code: "missing_synthetic_text".into(),
        })?;
        let mut output = self.output.clone();
        let coverage = &mut output["inspection"]["coverage"];
        coverage["input_digest"] = artifact["digest"].clone();
        coverage["input_bytes"] = json!(text.len());
        coverage["scanned_bytes"] = json!(text.len());
        // These are synthetic coverage assertions, not scanner observations.
        receipt["output"] = json!({"schema":invocation["output_schema"],"digest":canonical::document_digest(&output).map_err(synthetic_error)?,"bytes":canonical::bytes(&output).map_err(synthetic_error)?.len()});
        receipt["started_at_ms"] = json!(1100);
        receipt["completed_at_ms"] = json!(1100);
        Ok(ProviderResult {
            receipt,
            output: Some(output),
        })
    }
}
fn synthetic_error(_: aw_contracts::Error) -> HostError {
    HostError {
        code: "invalid_synthetic_fixture".into(),
    }
}

struct FixtureClock;
impl Clock for FixtureClock {
    fn now_ms(&self) -> u64 {
        1100
    }
}

struct TemporaryJournal(Option<PathBuf>);
impl TemporaryJournal {
    fn create() -> Result<Self, Box<dyn Error>> {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        fs::create_dir_all(&target)?;
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = target.join(format!("native-inspection-{}-{suffix}", std::process::id()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self(Some(path)))
    }
    fn path(&self) -> Result<&Path, io::Error> {
        self.0
            .as_deref()
            .ok_or_else(|| io::Error::other("temporary journal already removed"))
    }
    fn remove(&mut self) -> Result<(), io::Error> {
        if let Some(path) = &self.0 {
            fs::remove_dir_all(path)?;
            if path.try_exists()? {
                return Err(io::Error::other("temporary journal still exists"));
            }
            self.0 = None;
        }
        Ok(())
    }
}
impl Drop for TemporaryJournal {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            eprintln!("temporary journal cleanup failed: {error}");
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let f = canonical::parse(include_bytes!("../../../tests/fixtures/contracts.json"))?;
    let adapter = Adapter::new(Host::Qoder)?;
    let native_payload = json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_response":"synthetic output\n","session_id":"session-1","tool_use_id":"tool-1","native_metadata":{"opaque":true}});
    let captured = adapter.capture(CaptureRequest {
        native_event: "PostToolUse".into(),
        payload: native_payload.clone(),
        context: NativeContext {
            scope: f["capability-plan-v1"]["scope"].clone(),
            runtime: f["runtime-binding-v1"].clone(),
            event_id: "synthetic-native-event-1".into(),
        },
    })?;
    let registry = Registry::new()?;
    let mut plan = f["capability-plan-v1"].clone();
    plan["scope"] = captured.scope().clone();
    plan["event_id"] = json!(captured.event_id());
    plan["source_digest"] = captured.artifact()["digest"].clone();
    plan["boundary_id"] = captured.boundary()["boundary_id"].clone();
    plan["boundary_revision"] = captured.boundary()["revision"].clone();
    plan["boundary"] = captured.boundary()["boundary"].clone();
    let step = &mut plan["steps"][0];
    step["step_id"] = json!("inspect-1");
    step["capability"] = json!("security.content.inspect/v2");
    step["input_schema"] = registry.reference("security-content-inspect-input-v2")?;
    step["output_schema"] = registry.reference("security-content-inspect-output-v2")?;
    let mut host = SyntheticHost {
        descriptor: f["provider-descriptor-v1"].clone(),
        receipt: f["provider-receipt-v1"].clone(),
        output: f["security-content-inspect-output-v2"].clone(),
    };
    let prepared = adapter.prepare(
        captured,
        plan,
        BTreeMap::from([(
            "inspect-1".into(),
            StepOptions {
                constraints: json!({"include_low_confidence":false}),
                budget: f["capability-invocation-v1"]["budget"].clone(),
                deadline_at_ms: 2000,
            },
        )]),
        &host,
        1000,
    )?;
    let event_key = prepared.event_key().to_owned();
    let mut directory = TemporaryJournal::create()?;
    let mut journal = FileJournal::new(directory.path()?)?;
    let result = adapter.execute(
        prepared,
        &mut host,
        &mut journal,
        &FixtureClock,
        &NeverCancel,
    )?;
    drop(journal);
    let reopened = FileJournal::new(directory.path()?)?;
    let records = reopened.read_verified(&event_key, result.execution().journal_ack())?;
    if result.native_payload() != &native_payload {
        return Err(io::Error::other("native payload changed").into());
    }
    println!("native_host=qoder fixture_only=true");
    println!("provider=synthetic_fixture (not SecCore)");
    println!(
        "calls={} decision={}",
        result.execution().calls().len(),
        result.execution().record()["decision"]
    );
    println!(
        "native_payload_preserved=true journal_verified=true records={}",
        records.len()
    );
    println!("native_registration=not_exercised enforcement=not_observed adoption=not_observed");
    drop(reopened);
    directory.remove()?;
    println!("temporary_journal_cleanup=complete");
    Ok(())
}
