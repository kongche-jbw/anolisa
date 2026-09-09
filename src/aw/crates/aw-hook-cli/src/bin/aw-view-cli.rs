#![forbid(unsafe_code)]
//! Read a bound session's receipts through the Core journal before rendering counters.

use aw_contracts::{canonical, Registry};
use aw_core::journal::FileJournal;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const SCOPE_FIELDS: [&str; 7] = [
    "environment_id",
    "execution_context_id",
    "actor_id",
    "runtime_id",
    "runtime_generation",
    "binding_revision",
    "session_id",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    runtime: Value,
    scope: Value,
    agent_pid: u32,
    agent_start_ticks: u64,
    journal: PathBuf,
    evidence: PathBuf,
    #[serde(default)]
    adoption_bindings: BTreeMap<String, PathBuf>,
}

#[derive(Default, serde::Serialize)]
struct Counters {
    kind: String,
    calls: u64,
    candidates: u64,
    adopted: u64,
    saved_bytes: u64,
    failed: u64,
    bypassed: u64,
}

fn invalid(message: &str) -> Box<dyn Error> {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}
fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(invalid(message))
    }
}
fn read(path: &Path) -> Result<Value> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((canonical::MAX_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(canonical::parse(&bytes)?)
}
fn scope_matches(scope: &Value, binding: &Value) -> bool {
    SCOPE_FIELDS
        .iter()
        .all(|field| scope.get(field) == binding.get(field))
}
fn runtime_alive(binding: &Binding) -> bool {
    fs::read_to_string(format!("/proc/{}/stat", binding.agent_pid))
        .ok()
        .and_then(|s| s.rsplit_once(')').map(|(_, fields)| fields.to_owned()))
        .and_then(|s| {
            if matches!(s.split_whitespace().next(), Some("Z" | "X")) {
                return None;
            }
            s.split_whitespace()
                .nth(19)
                .and_then(|v| v.parse::<u64>().ok())
        })
        == Some(binding.agent_start_ticks)
}
fn verify_event(
    registry: &Registry,
    journal: &FileJournal,
    event: &Value,
    key: &str,
) -> Result<Vec<Value>> {
    require(
        event["event_key"] == key,
        "evidence filename differs from event key",
    )?;
    let records = journal.read_verified(key, &event["journal_ack"])?;
    let plan = &records[0]["record"]["plan"];
    registry.validate("capability-plan-v1", plan)?;
    let execution = &event["execution"];
    registry.validate("plan-execution-v1", execution)?;
    require(
        records.last().map(|r| &r["record"]["execution"]) == Some(execution),
        "execution differs from journal terminal record",
    )?;
    require(
        execution["scope"] == plan["scope"]
            && execution["event_id"] == plan["event_id"]
            && execution["plan_digest"] == canonical::document_digest(plan)?,
        "execution differs from reserved plan",
    )?;
    require(
        event["source_digest"] == plan["source_digest"],
        "source differs from reserved plan",
    )?;
    let receipts: Vec<_> = records
        .iter()
        .filter(|r| r["record"]["kind"] == "invocation_settled")
        .map(|r| r["record"]["receipt"].clone())
        .collect();
    let calls = event["calls"]
        .as_array()
        .ok_or_else(|| invalid("missing calls"))?;
    require(
        calls.len() == receipts.len(),
        "receipt count differs from journal",
    )?;
    let mut ids = BTreeSet::new();
    for (call, receipt) in calls.iter().zip(&receipts) {
        require(call["receipt"] == *receipt, "receipt differs from journal")?;
        registry.validate("provider-receipt-v1", receipt)?;
        require(
            receipt["scope"] == execution["scope"]
                && receipt["plan_ref"]["digest"] == execution["plan_digest"],
            "receipt scope or plan mismatch",
        )?;
        let id = receipt["invocation_id"]
            .as_str()
            .ok_or_else(|| invalid("missing invocation ID"))?;
        require(ids.insert(id), "duplicate invocation ID")?;
        for reference in receipt["evidence"].as_array().into_iter().flatten() {
            if reference["source_id"] == "tokenless-native-mapping/v1" {
                let mapping = &event["tokenless_mapping"];
                require(
                    mapping["invocation_id"] == id
                        && reference["record_id"] == id
                        && reference["digest"] == canonical::document_digest(mapping)?,
                    "native provider detail differs from receipt evidence",
                )?;
            }
        }
        let references: Vec<_> = execution["steps"]
            .as_array()
            .ok_or_else(|| invalid("missing steps"))?
            .iter()
            .flat_map(|s| s["invocations"].as_array().into_iter().flatten())
            .filter(|v| v["invocation_id"] == id)
            .collect();
        require(
            references.len() == 1
                && references[0]["receipt_digest"] == canonical::document_digest(receipt)?,
            "execution receipt reference mismatch",
        )?;
        if receipt["disposition"] == "produced" {
            let output = &call["output"];
            let schema = match receipt["capability"].as_str() {
                Some("security.content.inspect/v2") => "security-content-inspect-output-v2",
                Some("security.code.inspect/v2") => "security-code-inspect-output-v2",
                Some("context.projection.prepare/v2") => "context-projection-prepare-output-v2",
                _ => return Err(invalid("unsupported display capability")),
            };
            registry.validate(schema, output)?;
            require(
                receipt["output"]["schema"] == registry.reference(schema)?
                    && receipt["output"]["digest"] == canonical::document_digest(output)?
                    && receipt["output"]["bytes"].as_u64()
                        == Some(canonical::bytes(output)?.len() as u64),
                "output differs from receipt",
            )?;
        } else {
            require(call["output"].is_null(), "non-produced receipt has output")?;
        }
    }
    Ok(calls.clone())
}
fn verified_adoption(binding: &Binding, key: &str) -> Result<Option<Value>> {
    let Some(path) = binding.adoption_bindings.get(key) else {
        return Ok(None);
    };
    require(path.is_absolute(), "absolute adoption binding required")?;
    let trusted = read(path)?;
    require(
        scope_matches(&trusted["scope"], &binding.scope)
            && trusted["runtime"] == binding.runtime
            && trusted["agent_pid"] == binding.agent_pid
            && trusted["agent_start_ticks"] == binding.agent_start_ticks
            && trusted["journal"] == json!(binding.journal)
            && trusted["evidence"] == json!(binding.evidence),
        "adoption binding differs from current view",
    )?;
    if !binding
        .evidence
        .join(format!("{key}.adoption.json"))
        .try_exists()?
    {
        return Ok(None);
    }
    let binary = std::env::current_exe()?.with_file_name("aw-adoption-cli");
    let mut child = std::process::Command::new(binary)
        .args(["verify"])
        .arg(path)
        .arg(key)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(invalid("adoption verification timeout"));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    require(status.success(), "adoption verification failed")?;
    let mut raw = Vec::new();
    child
        .stdout
        .take()
        .ok_or_else(|| invalid("missing verifier output"))?
        .take(64 * 1024)
        .read_to_end(&mut raw)?;
    let result = canonical::parse(&raw)?;
    require(
        result["verification"] == "native_history_and_journals_verified"
            && result["event_key"] == key
            && scope_matches(&result["observation"]["scope"], &binding.scope),
        "adoption verifier identity mismatch",
    )?;
    Ok(Some(result))
}
fn view(binding: Binding) -> Result<Value> {
    let registry = Registry::new()?;
    registry.validate("runtime-binding-v1", &binding.runtime)?;
    require(
        binding.scope.is_object()
            && SCOPE_FIELDS
                .iter()
                .all(|k| binding.scope.get(k).is_some_and(|v| !v.is_null())),
        "explicit native session scope required",
    )?;
    require(
        binding.scope["session_id"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "native session required",
    )?;
    for key in ["runtime_id", "binding_revision", "environment_id"] {
        require(
            binding.scope[key] == binding.runtime[key],
            "runtime scope mismatch",
        )?;
    }
    require(
        binding.scope["runtime_generation"] == binding.runtime["generation"],
        "runtime generation mismatch",
    )?;
    if let Some(session) = binding.runtime.get("session_id") {
        require(
            session == &binding.scope["session_id"],
            "runtime session mismatch",
        )?;
    }
    require(
        binding.journal.is_absolute() && binding.evidence.is_absolute(),
        "absolute evidence paths required",
    )?;
    let mut providers: BTreeMap<String, Counters> = BTreeMap::new();
    let mut invocation_ids = BTreeSet::new();
    let mut history_verified = false;
    let mut verified_events = BTreeSet::new();
    require(
        binding.evidence.try_exists()? || !binding.journal.try_exists()?,
        "journal exists without event evidence",
    )?;
    if binding.evidence.try_exists()? {
        require(binding.journal.is_dir(), "journal directory missing")?;
        let journal = FileJournal::new(&binding.journal)?;
        let mut files = Vec::new();
        let mut settled_keys = BTreeSet::new();
        for entry in fs::read_dir(&binding.evidence)? {
            let entry = entry?;
            require(
                files.len() < 4096,
                "session evidence directory exceeds limit",
            )?;
            if entry.path().extension().is_some_and(|s| s == "json") {
                files.push(entry.path());
            }
        }
        files.sort();
        for path in files {
            require(!path.is_symlink(), "evidence symlink unsupported")?;
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.ends_with(".adoption.json"))
            {
                continue;
            }
            let event = read(&path)?;
            if !scope_matches(&event["execution"]["scope"], &binding.scope) {
                continue;
            }
            let key = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| invalid("invalid evidence filename"))?;
            let calls = verify_event(&registry, &journal, &event, key)?;
            verified_events.insert(key.to_owned());
            settled_keys.insert(key.to_owned());
            let adopted = verified_adoption(&binding, key)?;
            history_verified |= adopted.is_some();
            for call in calls {
                let receipt = &call["receipt"];
                let id = receipt["invocation_id"]
                    .as_str()
                    .ok_or_else(|| invalid("missing invocation"))?;
                require(
                    invocation_ids.insert(id.to_owned()),
                    "invocation counted twice",
                )?;
                let provider = receipt["provider_id"]
                    .as_str()
                    .ok_or_else(|| invalid("missing provider"))?;
                let counters = providers.entry(provider.to_owned()).or_default();
                let kind = if receipt["capability"] == "context.projection.prepare/v2" {
                    "projection"
                } else {
                    "security"
                };
                require(
                    counters.kind.is_empty() || counters.kind == kind,
                    "provider has mixed display roles",
                )?;
                counters.kind = kind.to_owned();
                if let Some(observed) = &adopted {
                    if observed["provider_id"] == provider
                        && observed["observation"]["invocation_id"] == id
                    {
                        let savings = observed["saved_bytes"]
                            .as_u64()
                            .ok_or_else(|| invalid("missing verified savings"))?;
                        if observed["observation"]["decision"] == "adopted" {
                            require(
                                receipt["disposition"] == "produced" && kind == "projection",
                                "adoption requires a produced projection",
                            )?;
                            counters.adopted += 1;
                            counters.saved_bytes = counters
                                .saved_bytes
                                .checked_add(savings)
                                .ok_or_else(|| invalid("savings overflow"))?;
                        } else {
                            require(savings == 0, "retained result claims savings")?;
                        }
                    }
                }
                counters.calls += 1;
                match receipt["disposition"].as_str() {
                    Some("failed") => counters.failed += 1,
                    Some("bypassed") => counters.bypassed += 1,
                    Some("produced")
                        if receipt["capability"] == "context.projection.prepare/v2" =>
                    {
                        counters.candidates += 1
                    }
                    _ => {}
                }
            }
        }
        for entry in fs::read_dir(&binding.journal)? {
            let path = entry?.path();
            if path.extension().is_none_or(|v| v != "jsonl") {
                continue;
            }
            let key = path
                .file_stem()
                .and_then(|v| v.to_str())
                .ok_or_else(|| invalid("invalid journal filename"))?;
            let records = journal.read(key)?;
            if scope_matches(&records[0]["record"]["plan"]["scope"], &binding.scope) {
                require(
                    settled_keys.contains(key),
                    "current session has unsettled or missing evidence",
                )?;
            }
        }
    }
    let rows: Vec<_> = providers
        .into_iter()
        .map(|(id, counts)| {
            let mut row = json!(counts);
            row["provider_id"] = json!(id);
            row
        })
        .collect();
    Ok(
        json!({"format":1,"scope":binding.scope,"runtime_alive":runtime_alive(&binding),"providers":rows,"events":verified_events,"verification":"journal_verified","adoption":if history_verified {"local_history"} else {"not_observed"}}),
    )
}
fn main() {
    let result = (|| -> Result<Value> {
        let mut args = std::env::args_os().skip(1);
        let path = args
            .next()
            .ok_or_else(|| invalid("usage: aw-view-cli BINDING.json"))?;
        require(args.next().is_none(), "unexpected argument")?;
        view(serde_json::from_value(read(Path::new(&path))?)?)
    })();
    match result {
        Ok(value) => println!("{value}"),
        Err(_) => {
            eprintln!("AW view unavailable: session binding or journal evidence failed validation");
            std::process::exit(1);
        }
    }
}
