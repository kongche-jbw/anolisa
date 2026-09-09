#![forbid(unsafe_code)]
//! Opt-in native post-tool observation; never emits a tool permit or adoption.

use aw_adapters::{Adapter, CaptureRequest, Host, NativeContext, StepOptions};
use aw_contracts::{canonical, Registry};
use aw_core::{
    journal::FileJournal,
    ports::{Clock, NeverCancel, ProviderHost},
};
use aw_sec_host::{Config, Limits, SecHost};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    error::Error,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MAX_INPUT: usize = 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    runtime: Value,
    scope: Value,
    agent_pid: u32,
    agent_start_ticks: u64,
    // A launcher-owned single turn is allowed only for Qoder's bounded smoke.
    qoder_single_turn_id: Option<String>,
    journal: PathBuf,
    evidence: PathBuf,
    provider: ProviderSettings,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderSettings {
    provider_id: String,
    provider_version: String,
    program: PathBuf,
    args: Vec<String>,
    environment: BTreeMap<String, String>,
}
struct WallClock;
impl Clock for WallClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn bounded(mut reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((MAX_INPUT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_INPUT {
        return Err(invalid("hook input exceeds limit").into());
    }
    Ok(bytes)
}
fn process(pid: u32) -> Result<(u32, u64)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .ok_or_else(|| invalid("invalid process stat"))?
        .1
        .split_whitespace()
        .collect();
    Ok((
        fields
            .get(1)
            .ok_or_else(|| invalid("missing process parent"))?
            .parse()?,
        fields
            .get(19)
            .ok_or_else(|| invalid("missing process start"))?
            .parse()?,
    ))
}
fn verify_owner(settings: &Settings) -> Result<()> {
    let mut pid = std::process::id();
    for _ in 0..128 {
        let (parent, started) = process(pid)?;
        if pid == settings.agent_pid {
            if started != settings.agent_start_ticks {
                return Err(invalid("agent incarnation changed").into());
            }
            return Ok(());
        }
        if parent == 0 || parent == pid {
            break;
        }
        pid = parent;
    }
    Err(invalid("hook is not a descendant of the configured agent").into())
}
fn identity(payload: &Value, field: &str) -> Result<String> {
    payload[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| invalid("native identity missing").into())
}
fn main() {
    if run().is_err() {
        // Provider stderr and native payloads may contain secrets; do not echo them.
        eprintln!("aw-hook: inspection unavailable; no enforcement or adoption asserted");
        println!(
            "{}",
            json!({"systemMessage":"AW inspection unavailable; original tool result retained."})
        );
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let host = match args.next().as_deref() {
        Some("qoder") => Host::Qoder,
        Some("codex") => Host::Codex,
        _ => return Err(invalid("usage: aw-hook-cli <qoder|codex> SETTINGS.json").into()),
    };
    let path = args.next().ok_or_else(|| invalid("missing settings"))?;
    if args.next().is_some() {
        return Err(invalid("unexpected argument").into());
    }
    let settings: Settings = serde_json::from_slice(&bounded(fs::File::open(path)?)?)?;
    if !settings.scope.is_object() {
        return Err(invalid("scope must be an object").into());
    }
    verify_owner(&settings)?;
    let payload: Value = serde_json::from_slice(&bounded(io::stdin().lock())?)?;
    if payload["hook_event_name"] != "PostToolUse" {
        return Err(invalid("only PostToolUse is supported").into());
    }
    let mut scope = settings.scope.clone();
    for field in ["session_id", "tool_use_id"] {
        let observed = identity(&payload, field)?;
        if scope.get(field).is_some() && scope[field] != observed {
            return Err(invalid("configured native identity differs").into());
        }
        scope[field] = json!(observed);
    }
    let turn = if host == Host::Codex {
        identity(&payload, "turn_id")?
    } else {
        settings
            .qoder_single_turn_id
            .clone()
            .ok_or_else(|| invalid("Qoder requires an explicitly bounded launcher turn"))?
    };
    if scope.get("turn_id").is_some() && scope["turn_id"] != turn {
        return Err(invalid("configured turn differs").into());
    }
    scope["turn_id"] = json!(turn);
    let event_id = canonical::document_digest(
        &json!({"scope":scope,"host":host.as_str(),"event":"PostToolUse"}),
    )?;
    let adapter = Adapter::new(host)?;
    let captured = adapter.capture(CaptureRequest {
        native_event: "PostToolUse".into(),
        payload,
        context: NativeContext {
            scope,
            runtime: settings.runtime,
            event_id,
        },
    })?;
    let source_digest = captured.artifact()["digest"].clone();
    let source_bytes = captured.artifact()["content"]
        .as_str()
        .ok_or_else(|| invalid("missing captured text"))?
        .len();
    let mut provider = SecHost::new(Config {
        provider_id: settings.provider.provider_id.clone(),
        provider_version: settings.provider.provider_version,
        program: settings.provider.program,
        args: settings.provider.args,
        environment: settings.provider.environment,
        limits: Limits {
            timeout_ms: 10_000,
            input_bytes: MAX_INPUT,
            output_bytes: 128 * 1024,
            stderr_bytes: 16 * 1024,
        },
    })?;
    let descriptor = provider
        .descriptor(&settings.provider.provider_id)
        .ok_or_else(|| invalid("missing provider descriptor"))?;
    let registry = Registry::new()?;
    let input_schema = registry.reference("security-content-inspect-input-v2")?;
    let output_schema = registry.reference("security-content-inspect-output-v2")?;
    let plan = json!({
        "plan_id":captured.event_id(), "revision":1,"event_id":captured.event_id(),"scope":captured.scope(),
        "boundary_id":captured.boundary()["boundary_id"],"boundary_revision":captured.boundary()["revision"],
        "boundary":"post_tool","policy_revision":1,"source_digest":source_digest,
        "steps":[{"step_id":"inspect","capability":"security.content.inspect/v2","input_schema":input_schema,
          "output_schema":output_schema,"selection":"exactly_one","providers":[{
            "provider_id":descriptor["provider_id"],"provider_version":descriptor["provider_version"],"manifest_digest":descriptor["manifest_digest"]
          }],"required":true,"on_failure":"reject_plan","input_source":"boundary_source"}]
    });
    let now = WallClock.now_ms();
    let prepared = adapter.prepare(captured, plan, BTreeMap::from([("inspect".into(), StepOptions {
        constraints:json!({"include_low_confidence":false}),
        budget:json!({"input_bytes":MAX_INPUT,"output_bytes":128*1024,"wall_time_ms":10_000}),
        deadline_at_ms:now+15_000,
    })]), &provider, now)?;
    let key = prepared.event_key().to_owned();
    let mut journal = FileJournal::new(&settings.journal)?;
    let result = adapter.execute(
        prepared,
        &mut provider,
        &mut journal,
        &WallClock,
        &NeverCancel,
    )?;
    let execution = result.execution();
    journal.read_verified(&key, execution.journal_ack())?;
    let calls: Vec<_> = execution
        .calls()
        .iter()
        .map(|c| json!({"receipt":c.result().receipt,"output":c.result().output}))
        .collect();
    fs::create_dir_all(&settings.evidence)?;
    let evidence = json!({"host":host.as_str(),"event_key":key,"source_digest":source_digest,"source_bytes":source_bytes,
        "execution":execution.record(),"journal_ack":execution.journal_ack(),"calls":calls,
        "native_payload_preserved":true,"adoption":"not_observed","enforcement":"not_attempted"});
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(settings.evidence.join(format!("{key}.json")))?;
    file.write_all(&serde_json::to_vec_pretty(&evidence)?)?;
    file.sync_all()?;
    fs::File::open(&settings.evidence)?.sync_all()?;
    let inspection = execution
        .calls()
        .first()
        .and_then(|c| c.result().output.as_ref());
    match inspection.and_then(|o| o["inspection"]["verdict"].as_str()) {
        Some("clean") => println!("{{}}"),
        Some(_) => println!(
            "{}",
            json!({"systemMessage":"AW SecCore found sensitive or suspicious content. Observation only; original tool result retained."})
        ),
        None if execution
            .calls()
            .first()
            .is_some_and(|c| c.result().receipt["disposition"] == "bypassed") =>
        {
            println!(
                "{}",
                json!({"systemMessage":"AW SecCore skipped this inspection. Original tool result retained."})
            )
        }
        None => println!(
            "{}",
            json!({"systemMessage":"AW SecCore inspection failed. Original tool result retained."})
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_input_limit_is_enforced_before_decoding() {
        assert_eq!(bounded(&b"{}"[..]).unwrap(), b"{}");
        assert!(bounded(&vec![b'x'; MAX_INPUT + 1][..]).is_err());
    }

    #[test]
    fn native_identity_must_be_present_and_textual() {
        for value in [
            json!({}),
            json!({"turn_id":null}),
            json!({"turn_id":2}),
            json!({"turn_id":""}),
        ] {
            assert!(identity(&value, "turn_id").is_err());
        }
        assert_eq!(
            identity(&json!({"turn_id":"actual-turn"}), "turn_id").unwrap(),
            "actual-turn"
        );
    }

    #[test]
    fn runtime_owner_requires_the_same_live_incarnation() {
        let pid = std::process::id();
        let (_, start) = process(pid).unwrap();
        let mut settings:Settings=serde_json::from_value(json!({
            "runtime":{},"scope":{},"agent_pid":pid,"agent_start_ticks":start,
            "journal":"unused","evidence":"unused","provider":{
              "provider_id":"unused","provider_version":"unused","program":"/unused","args":[],"environment":{}
            }
        })).unwrap();
        verify_owner(&settings).unwrap();
        settings.agent_start_ticks += 1;
        assert!(verify_owner(&settings).is_err());
        settings.agent_pid = 0;
        assert!(verify_owner(&settings).is_err());
    }
}
