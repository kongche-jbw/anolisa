//! Runs a synthetic in-process candidate through Core and a real Linux journal.
//! This reference Host is neither Tokenless nor SecCore and observes no adoption.

use aw_contracts::canonical;
use aw_core::{
    journal::FileJournal,
    ports::{Clock, HostError, NeverCancel, ProviderHost, ProviderResult},
    Core, PrepareRequest, StepInput,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

struct FixtureHost {
    descriptor: Value,
    output: Value,
    receipt: Value,
}

impl ProviderHost for FixtureHost {
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
        let digest = canonical::document_digest(&self.output).map_err(fixture_error)?;
        let bytes = canonical::bytes(&self.output).map_err(fixture_error)?.len();
        receipt["output"] = json!({
            "schema":invocation["output_schema"], "digest":digest, "bytes":bytes
        });
        receipt["started_at_ms"] = json!(1100);
        receipt["completed_at_ms"] = json!(1100);
        Ok(ProviderResult {
            receipt,
            output: Some(self.output.clone()),
        })
    }
}

fn fixture_error(_: aw_contracts::Error) -> HostError {
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
        let path = target.join(format!("pinned-plan-{}-{suffix}", std::process::id()));
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
    let fixture = canonical::parse(include_bytes!("../../../tests/fixtures/contracts.json"))?;
    let core = Core::new()?;
    let mut host = FixtureHost {
        descriptor: fixture["provider-descriptor-v1"].clone(),
        output: fixture["context-projection-prepare-output-v2"].clone(),
        receipt: fixture["provider-receipt-v1"].clone(),
    };
    let prepared = core.prepare(
        PrepareRequest {
            plan: fixture["capability-plan-v1"].clone(),
            boundary: fixture["boundary-descriptor-v1"].clone(),
            runtime: fixture["runtime-binding-v1"].clone(),
            inputs: BTreeMap::from([(
                "project-1".into(),
                StepInput {
                    input: fixture["context-projection-prepare-input-v2"].clone(),
                    budget: fixture["capability-invocation-v1"]["budget"].clone(),
                    deadline_at_ms: 2000,
                },
            )]),
        },
        &host,
        1000,
    )?;
    let event_key = prepared.event_key().to_owned();
    let mut directory = TemporaryJournal::create()?;
    let mut journal = FileJournal::new(directory.path()?)?;
    let execution = core.execute(
        prepared,
        &mut host,
        &mut journal,
        &FixtureClock,
        &NeverCancel,
    )?;
    // Reopen the persisted file and compare its chain tip to the returned evidence.
    drop(journal);
    let reopened = FileJournal::new(directory.path()?)?;
    let records = reopened.read_verified(&event_key, execution.journal_ack())?;
    println!("provider=synthetic_fixture (not Tokenless or SecCore)");
    println!("plan_decision={}", execution.record()["decision"]);
    println!("calls={}", execution.calls().len());
    println!("journal_verified=true records={}", records.len());
    println!("adoption=not_observed");
    drop(reopened);
    directory.remove()?;
    println!("temporary_journal_cleanup=complete");
    Ok(())
}
