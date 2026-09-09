#![forbid(unsafe_code)]
//! Independently observe Qoder local history; hook output is never adoption proof.

use aw_adapters::{Adapter, CaptureRequest, Host, NativeContext};
use aw_contracts::{
    canonical, orchestration::InvocationEvidence, validation::AdoptionCheck, Registry,
};
use aw_core::{journal::FileJournal, ports::Journal};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    error::Error,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    runtime: Value,
    scope: Value,
    agent_pid: u32,
    agent_start_ticks: u64,
    started_at_ms: u64,
    workspace: PathBuf,
    journal: PathBuf,
    evidence: PathBuf,
    native_event: PathBuf,
    adoption_journal: PathBuf,
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
fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .ok_or_else(|| invalid("missing text field"))
}
fn read(path: &Path, limit: usize, native: bool) -> Result<Value> {
    require(!path.is_symlink(), "symlinks are unsupported")?;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    require(bytes.len() <= limit, "input exceeds limit")?;
    if native {
        Ok(serde_json::from_slice(&bytes)?)
    } else {
        Ok(canonical::parse(&bytes)?)
    }
}
fn now_ms() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
}

// Qoder records UTC RFC3339 timestamps. Other timezone forms are not inferred.
fn timestamp(value: &Value) -> Result<u64> {
    let s = value
        .as_str()
        .ok_or_else(|| invalid("missing history timestamp"))?;
    let b = s.as_bytes();
    require(
        b.len() >= 20
            && b[4] == b'-'
            && b[7] == b'-'
            && b[10] == b'T'
            && b[13] == b':'
            && b[16] == b':'
            && b.last() == Some(&b'Z'),
        "unsupported history timestamp",
    )?;
    let number = |start: usize, end: usize| -> Result<i64> {
        require(
            b[start..end].iter().all(u8::is_ascii_digit),
            "invalid timestamp digits",
        )?;
        Ok(std::str::from_utf8(&b[start..end])?.parse()?)
    };
    let (year, month, day, hour, minute, second) = (
        number(0, 4)?,
        number(5, 7)?,
        number(8, 10)?,
        number(11, 13)?,
        number(14, 16)?,
        number(17, 19)?,
    );
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    require(
        (1970..=9999).contains(&year)
            && day >= 1
            && day <= month_days
            && hour < 24
            && minute < 60
            && second < 60,
        "invalid timestamp date",
    )?;
    let ms = if b.len() == 20 {
        0
    } else {
        require(
            b[19] == b'.' && b.len() >= 22 && b.len() <= 29,
            "unsupported timestamp fraction",
        )?;
        let digits = &b[20..b.len() - 1];
        require(
            digits.iter().all(u8::is_ascii_digit),
            "invalid timestamp fraction",
        )?;
        digits
            .iter()
            .take(3)
            .fold(0u64, |sum, d| sum * 10 + u64::from(d - b'0'))
            * 10u64.pow(3 - digits.len().min(3) as u32)
    };
    let y = year - i64::from(month <= 2);
    let era = y / 400;
    let yoe = y - era * 400;
    let m = month + if month > 2 { -3 } else { 9 };
    let days =
        era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + (153 * m + 2) / 5 + day - 1 - 719468;
    Ok(u64::try_from(days * 86400 + hour * 3600 + minute * 60 + second)? * 1000 + ms)
}

struct History {
    effective: String,
    line: usize,
    digest: String,
    path: PathBuf,
    rows: Vec<Value>,
}

fn history(native: &Value, binding: &Binding, now: u64, stored: Option<&Value>) -> Result<History> {
    let session = text(&binding.scope, "session_id")?;
    let tool = text(native, "tool_use_id")?;
    let path = PathBuf::from(text(native, "transcript_path")?);
    require(
        path.is_absolute()
            && !path.is_symlink()
            && path.file_stem().and_then(|s| s.to_str()) == Some(session),
        "history path is not bound to native session",
    )?;
    require(
        native["cwd"] == binding.workspace.to_string_lossy().as_ref(),
        "native workspace differs",
    )?;
    let rows = if let Some(stored) = stored {
        stored
            .as_array()
            .ok_or_else(|| invalid("missing stored history rows"))?
            .clone()
    } else {
        require(!path.is_symlink(), "history symlink unsupported")?;
        let mut raw = String::new();
        fs::File::open(&path)?
            .take(64 * 1024 * 1024 + 1)
            .read_to_string(&mut raw)?;
        require(raw.len() <= 64 * 1024 * 1024, "history exceeds limit")?;
        raw.split_inclusive('\n')
            .enumerate()
            .filter(|(_, line)| line.ends_with('\n'))
            .map(|(index, line)| json!({"line":index+1,"raw":line.trim_end_matches('\n')}))
            .collect()
    };
    let mut tool_seen = false;
    let mut tool_row = None;
    let mut results = Vec::new();
    let mut last_line = 0;
    for item in &rows {
        let index = item["line"]
            .as_u64()
            .ok_or_else(|| invalid("missing history line number"))?;
        require(index > last_line, "history lines are not ordered")?;
        last_line = index;
        let line = text(item, "raw")?;
        let row: Value = serde_json::from_str(line)?;
        if row["sessionId"] != session || row["isSidechain"] == true {
            continue;
        }
        let Some(blocks) = row["message"]["content"].as_array() else {
            continue;
        };
        for block in blocks {
            let is_tool =
                row["type"] == "assistant" && block["type"] == "tool_use" && block["id"] == tool;
            let is_result = row["type"] == "user"
                && block["type"] == "tool_result"
                && block["tool_use_id"] == tool;
            if !is_tool && !is_result {
                continue;
            }
            require(
                row["isSidechain"].is_null() || row["isSidechain"] == false,
                "invalid sidechain indicator",
            )?;
            require(
                row["cwd"] == binding.workspace.to_string_lossy().as_ref(),
                "history workspace differs",
            )?;
            let at = timestamp(&row["timestamp"])?;
            require(
                at >= binding.started_at_ms && at <= now,
                "history is outside bound launch",
            )?;
            if is_tool {
                require(
                    !tool_seen && results.is_empty(),
                    "duplicate native tool use",
                )?;
                require(
                    block["name"] == native["tool_name"] && block["input"] == native["tool_input"],
                    "history tool differs from captured input",
                )?;
                tool_seen = true;
                tool_row = Some(item.clone());
            } else {
                require(tool_seen, "tool result precedes tool use")?;
                require(
                    block["is_error"].is_null() || block["is_error"] == false,
                    "unsuccessful tool result",
                )?;
                results.push(History {
                    effective: text(block, "content")?.to_owned(),
                    line: usize::try_from(index)?,
                    digest: canonical::digest(line.as_bytes()),
                    path: path.clone(),
                    rows: vec![
                        tool_row
                            .clone()
                            .ok_or_else(|| invalid("missing tool row"))?,
                        item.clone(),
                    ],
                });
            }
        }
    }
    require(results.len() == 1, "no unique successful native result")?;
    results
        .pop()
        .ok_or_else(|| invalid("missing native result"))
}

fn ledger_body(observation: &Value, key: &str, binding: &Binding, history: &History) -> Value {
    let mut body = observation.clone();
    if let Some(object) = body.as_object_mut() {
        object.remove("ledger_status");
        object.remove("ledger_evidence");
    }
    json!({"kind":"context_adoption","event_key":key,"observation":body,
        "agent_pid":binding.agent_pid,"agent_start_ticks":binding.agent_start_ticks,
        "started_at_ms":binding.started_at_ms,"workspace":binding.workspace,
        "transcript_path":history.path,"transcript_line":history.line,"transcript_line_digest":history.digest,"history_rows":history.rows})
}

fn run(mode: &str, binding: Binding, key: &str) -> Result<Value> {
    require(
        matches!(mode, "record" | "verify"),
        "expected record or verify",
    )?;
    require(
        key.len() == 64
            && key
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid event key",
    )?;
    for path in [
        &binding.workspace,
        &binding.journal,
        &binding.evidence,
        &binding.native_event,
        &binding.adoption_journal,
    ] {
        require(path.is_absolute(), "absolute paths required")?;
    }
    require(
        binding.agent_pid > 0 && binding.agent_start_ticks > 0 && binding.started_at_ms > 0,
        "missing launch identity",
    )?;
    let now = now_ms()?;
    let registry = Registry::new()?;
    let native = read(&binding.native_event, canonical::MAX_DOCUMENT_BYTES, true)?;
    let event = read(
        &binding.evidence.join(format!("{key}.json")),
        canonical::MAX_DOCUMENT_BYTES,
        false,
    )?;
    require(
        event["host"] == "qoder" && event["event_key"] == key,
        "event identity differs",
    )?;
    require(
        event["runtime"] == binding.runtime,
        "evidence runtime differs",
    )?;
    require(
        native["hook_event_name"] == "PostToolUse",
        "native event is not PostToolUse",
    )?;
    let core = FileJournal::new(&binding.journal)?;
    let records = core.read_verified(key, &event["journal_ack"])?;
    let plan = &records[0]["record"]["plan"];
    let scope = &plan["scope"];
    for field in [
        "environment_id",
        "execution_context_id",
        "actor_id",
        "runtime_id",
        "runtime_generation",
        "binding_revision",
        "session_id",
    ] {
        require(
            !binding.scope[field].is_null() && binding.scope[field] == scope[field],
            "launch scope differs",
        )?;
    }
    if let Some(turn) = binding.scope.get("turn_id") {
        require(turn == &scope["turn_id"], "launch turn differs")?;
    }
    require(
        native["session_id"] == scope["session_id"]
            && native["tool_use_id"] == scope["tool_use_id"],
        "native identity differs",
    )?;
    require(
        canonical::document_digest(&json!({"scope":scope,"event_id":plan["event_id"]}))? == key,
        "event key differs from plan",
    )?;
    require(
        records.last().map(|r| &r["record"]["execution"]) == Some(&event["execution"]),
        "execution differs from core journal",
    )?;
    let capture = Adapter::new(Host::Qoder)?.capture(CaptureRequest {
        native_event: "PostToolUse".into(),
        payload: native.clone(),
        context: NativeContext {
            scope: scope.clone(),
            runtime: binding.runtime.clone(),
            event_id: text(plan, "event_id")?.into(),
        },
    })?;
    require(
        capture.artifact()["digest"] == plan["source_digest"]
            && event["source_digest"] == plan["source_digest"],
        "captured source differs",
    )?;
    let calls = event["calls"]
        .as_array()
        .ok_or_else(|| invalid("missing calls"))?;
    let mut invocations = Vec::new();
    for call in calls {
        let mut invocation = call["invocation"].clone();
        require(
            invocation.is_object()
                && invocation["input"].is_object()
                && invocation["input"]["artifact"].is_object(),
            "missing reconstructable invocation",
        )?;
        require(
            invocation["input"]["artifact"].get("content").is_none(),
            "invocation must omit source content",
        )?;
        invocation["input"]["artifact"]["content"] = capture.artifact()["content"].clone();
        require(
            invocation["input"]["artifact"] == *capture.artifact(),
            "invocation artifact differs",
        )?;
        let digest = canonical::document_digest(&invocation)?;
        let settled: Vec<_> = records
            .iter()
            .filter(|r| {
                r["record"]["kind"] == "invocation_settled"
                    && r["record"]["receipt"]["invocation_id"] == invocation["invocation_id"]
            })
            .collect();
        require(
            settled.len() == 1
                && settled[0]["record"]["invocation_digest"] == digest
                && settled[0]["record"]["receipt"] == call["receipt"],
            "invocation differs from settled core record",
        )?;
        require(
            call["receipt"]["started_at_ms"]
                .as_u64()
                .is_some_and(|at| at >= binding.started_at_ms),
            "receipt predates bound launch",
        )?;
        invocations.push(invocation);
    }
    require(
        records
            .iter()
            .filter(|r| r["record"]["kind"] == "invocation_settled")
            .count()
            == calls.len(),
        "incomplete calls",
    )?;
    let evidence: Vec<_> = calls
        .iter()
        .zip(&invocations)
        .map(|(c, i)| InvocationEvidence {
            invocation: i,
            receipt: &c["receipt"],
            output: c.get("output").filter(|o| !o.is_null()),
        })
        .collect();
    let projections: Vec<_> = evidence
        .iter()
        .filter(|e| e.invocation["capability"] == "context.projection.prepare/v2")
        .collect();
    require(projections.len() == 1, "expected one settled projection")?;
    let projection = projections[0];
    let output_path = binding.evidence.join(format!("{key}.adoption.json"));
    let persisted = if mode == "verify" || output_path.try_exists()? {
        let saved = read(&output_path, canonical::MAX_DOCUMENT_BYTES, false)?;
        let ledger = FileJournal::new(&binding.adoption_journal)?;
        let records = ledger.read_verified(key, &saved["observation"]["ledger_evidence"])?;
        require(records.len() == 1, "unexpected adoption journal entries")?;
        Some((saved, records[0]["record"]["plan"].clone()))
    } else {
        None
    };
    let observed = history(
        &native,
        &binding,
        now,
        persisted.as_ref().map(|(_, body)| &body["history_rows"]),
    )?;
    let source = text(capture.artifact(), "content")?;
    let candidate = projection
        .output
        .and_then(|o| o["candidate"]["content"].as_str());
    let adopted = candidate == Some(observed.effective.as_str()) && candidate != Some(source);
    require(
        adopted || observed.effective == source,
        "history differs from candidate and source",
    )?;
    let reason = if adopted {
        "candidate_selected"
    } else if projection.output.is_none() {
        "no_candidate"
    } else {
        "environment_rejected"
    };
    let mut observation = json!({"invocation_id":projection.invocation["invocation_id"],"scope":scope,
        "boundary_id":projection.invocation["boundary_id"],"boundary_revision":projection.invocation["boundary_revision"],
        "receipt_digest":canonical::document_digest(projection.receipt)?,"source_digest":capture.artifact()["digest"],
        "decision":if adopted {"adopted"} else {"preserved"},"reason":reason,
        "effective_digest":canonical::digest(observed.effective.as_bytes()),"effective_bytes":observed.effective.len(),
        "proof":{"boundary":"local_history","representation_id":format!("qoder-{key}"),"revision":1,
            "evidence":{"source_id":"qoder-native-local-history","record_id":format!("{key}:{}",observed.line),"digest":observed.digest},"observed_at_ms":now},
        "ledger_status":"unavailable"});
    if projection.output.is_some() {
        observation["candidate_digest"] = projection.receipt["output"]["digest"].clone();
    }
    if let Some((existing, stored_body)) = persisted {
        require(existing["event_key"] == key, "adoption event mismatch")?;
        let old = &existing["observation"];
        let at = old["proof"]["observed_at_ms"]
            .as_u64()
            .ok_or_else(|| invalid("missing observation time"))?;
        require(
            at >= binding.started_at_ms && at <= now,
            "stored observation time differs",
        )?;
        observation["proof"]["observed_at_ms"] = json!(at);
        observation["ledger_status"] = json!("committed");
        observation["ledger_evidence"] = old["ledger_evidence"].clone();
        require(
            observation == *old,
            "stored observation differs from native history",
        )?;
        require(
            stored_body == ledger_body(old, key, &binding, &observed),
            "adoption journal differs",
        )?;
    } else {
        registry.validate_plan_adoption(
            plan,
            &event["execution"],
            &evidence,
            AdoptionCheck {
                invocation: projection.invocation,
                receipt: projection.receipt,
                output: projection.output,
                observation: &observation,
                boundary: capture.boundary(),
                effective_text: Some(&observed.effective),
                recovered_text: None,
                now_ms: now,
            },
        )?;
        let mut ledger = FileJournal::new(&binding.adoption_journal)?;
        let ack = ledger.claim(key, &ledger_body(&observation, key, &binding, &observed))?;
        observation["ledger_status"] = json!("committed");
        observation["ledger_evidence"] = ack;
    }
    let savings = registry.validate_plan_adoption(
        plan,
        &event["execution"],
        &evidence,
        AdoptionCheck {
            invocation: projection.invocation,
            receipt: projection.receipt,
            output: projection.output,
            observation: &observation,
            boundary: capture.boundary(),
            effective_text: Some(&observed.effective),
            recovered_text: None,
            now_ms: now,
        },
    )?;
    let result = json!({"format":1,"event_key":key,"provider_id":projection.receipt["provider_id"],"observation":observation,"saved_bytes":savings,"verification":"native_history_and_journals_verified"});
    if mode == "record" && !output_path.try_exists()? {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&output_path)?;
        file.write_all(&canonical::bytes(&result)?)?;
        file.sync_all()?;
        fs::File::open(&binding.evidence)?.sync_all()?;
    }
    Ok(result)
}

fn main() {
    let result = (|| -> Result<Value> {
        let mut args = std::env::args().skip(1);
        let mode = args.next().ok_or_else(|| {
            invalid("usage: aw-adoption-cli record|verify BINDING.json EVENT_KEY")
        })?;
        let path = args.next().ok_or_else(|| invalid("missing binding"))?;
        let key = args.next().ok_or_else(|| invalid("missing event key"))?;
        require(args.next().is_none(), "unexpected argument")?;
        run(
            &mode,
            serde_json::from_value(read(
                Path::new(&path),
                canonical::MAX_DOCUMENT_BYTES,
                false,
            )?)?,
            &key,
        )
    })();
    match result {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!("AW adoption unavailable: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario() -> (Binding, Value, Value) {
        let binding = Binding {
            runtime: json!({}),
            scope: json!({"session_id":"synthetic-session"}),
            agent_pid: 1,
            agent_start_ticks: 1,
            started_at_ms: 1,
            workspace: "/synthetic/work".into(),
            journal: "/synthetic/journal".into(),
            evidence: "/synthetic/evidence".into(),
            native_event: "/synthetic/native.json".into(),
            adoption_journal: "/synthetic/adoption".into(),
        };
        let native = json!({"session_id":"synthetic-session","tool_use_id":"synthetic-tool",
            "cwd":"/synthetic/work","tool_name":"Bash","tool_input":{"command":"printf synthetic"},
            "transcript_path":"/synthetic/synthetic-session.jsonl"});
        let tool = json!({"sessionId":"synthetic-session","isSidechain":false,"cwd":"/synthetic/work",
            "timestamp":"1970-01-01T00:00:01.000Z","type":"assistant","message":{"content":[{
                "type":"tool_use","id":"synthetic-tool","name":"Bash","input":{"command":"printf synthetic"}}]}});
        let result = json!({"sessionId":"synthetic-session","isSidechain":false,"cwd":"/synthetic/work",
            "timestamp":"1970-01-01T00:00:02.000Z","type":"user","message":{"content":[{
                "type":"tool_result","tool_use_id":"synthetic-tool","is_error":false,"content":"synthetic"}]}});
        (
            binding,
            native,
            json!([{"line":3,"raw":tool.to_string()},{"line":5,"raw":result.to_string()}]),
        )
    }

    fn change(rows: &mut Value, index: usize, edit: impl FnOnce(&mut Value)) {
        let mut row: Value = serde_json::from_str(rows[index]["raw"].as_str().unwrap()).unwrap();
        edit(&mut row);
        rows[index]["raw"] = json!(row.to_string());
    }

    #[test]
    fn stored_native_rows_preserve_exact_text_and_line_evidence() {
        let (binding, native, rows) = scenario();
        let found = history(&native, &binding, 3000, Some(&rows)).unwrap();
        assert_eq!(found.effective, "synthetic");
        assert_eq!(found.line, 5);
        assert_eq!(
            found.digest,
            canonical::digest(rows[1]["raw"].as_str().unwrap().as_bytes())
        );
        assert_eq!(found.rows, rows.as_array().unwrap().clone());
    }

    #[test]
    fn duplicate_result_or_tool_use_is_not_unique_adoption() {
        for duplicate in [0, 1] {
            let (binding, native, mut rows) = scenario();
            let mut row = rows[duplicate].clone();
            row["line"] = json!(8);
            rows.as_array_mut().unwrap().push(row);
            assert!(history(&native, &binding, 3000, Some(&rows)).is_err());
        }
    }

    #[test]
    fn wrong_session_workspace_sidechain_error_and_multimodal_are_rejected() {
        let changes = [
            ("sessionId", json!("other-session")),
            ("cwd", json!("/other/work")),
            ("isSidechain", json!(true)),
            ("isSidechain", json!("false")),
        ];
        for (field, value) in changes {
            let (binding, native, mut rows) = scenario();
            change(&mut rows, 1, |row| row[field] = value);
            assert!(history(&native, &binding, 3000, Some(&rows)).is_err());
        }
        for value in [json!(true), json!("false")] {
            let (binding, native, mut rows) = scenario();
            change(&mut rows, 1, |row| {
                row["message"]["content"][0]["is_error"] = value
            });
            assert!(history(&native, &binding, 3000, Some(&rows)).is_err());
        }
        let (binding, native, mut rows) = scenario();
        change(&mut rows, 1, |row| {
            row["message"]["content"][0]["content"] = json!([{"type":"image"}])
        });
        assert!(history(&native, &binding, 3000, Some(&rows)).is_err());
    }

    #[test]
    fn launch_time_tool_input_and_order_are_bound() {
        let (mut binding, native, rows) = scenario();
        binding.started_at_ms = 1500;
        assert!(history(&native, &binding, 3000, Some(&rows)).is_err());
        binding.started_at_ms = 1;
        assert!(history(&native, &binding, 1500, Some(&rows)).is_err());
        let mut reversed = rows.clone();
        reversed.as_array_mut().unwrap().reverse();
        assert!(history(&native, &binding, 3000, Some(&reversed)).is_err());
        let mut wrong = rows;
        change(&mut wrong, 0, |row| {
            row["message"]["content"][0]["input"]["command"] = json!("printf other")
        });
        assert!(history(&native, &binding, 3000, Some(&wrong)).is_err());
    }

    #[test]
    fn utc_timestamp_parser_checks_calendar_and_fraction() {
        assert_eq!(timestamp(&json!("1970-01-01T00:00:00Z")).unwrap(), 0);
        assert_eq!(timestamp(&json!("1970-01-01T00:00:01.12Z")).unwrap(), 1120);
        assert_eq!(
            timestamp(&json!("2000-03-01T00:00:00Z")).unwrap(),
            951868800000
        );
        for input in [
            "2025-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-01-01T24:00:00Z",
            "2026-01-01T00:00:00+08:00",
            "2026-01-01T00:00:00.Z",
            "2026-01-01T00:00:00.xyZ",
        ] {
            assert!(timestamp(&json!(input)).is_err(), "{input}");
        }
    }
}
