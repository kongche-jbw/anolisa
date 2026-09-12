//! Explicit native-history observation and read-only queries of recorded facts.

use crate::{
    history::Proof,
    records::{self, Record},
    Error,
};
use aw_contracts::{canonical, validation::AdoptionCheck, Registry};
use aw_core::{journal::FileJournal, ports::Journal};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

const KIND: &str = "qoder_local_history_observation/v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fact {
    format: u32,
    context_digest: String,
    proof: Proof,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    format: u32,
    event_key: String,
    journal_ack: Value,
    fact: Fact,
}

/// Observes one Qoder history snapshot and durably records a verified result.
///
/// Missing or late history stays unverified and may be observed again explicitly.
/// A committed observation is immutable; duplicate or interrupted claims fail.
/// The native history is trusted local data, not proof of model billing or an
/// attestation against a process that can rewrite private records and evidence.
///
/// # Errors
/// Rejects invalid execution, unsafe storage, ambiguous history or failed writes.
pub fn observe(path: &Path) -> Result<Value, Error> {
    let (record, digest) = records::load(path)?;
    let stored_path = records::sibling(path, "observation.json");
    if records::exists(&stored_path)? {
        return Err(Error::Execution);
    }
    let Some(call) = record.projection() else {
        return query(path);
    };
    let Some(proof) = record.history.observe()? else {
        return query(path);
    };
    let completed = call.receipt["completed_at_ms"]
        .as_u64()
        .ok_or(Error::Execution)?;
    if proof.observed_at_ms < completed {
        return Err(Error::Execution);
    }
    if proof.observed_at_ms - completed > record.history.max_observation_delay_ms {
        return query(path);
    }
    let fact = Fact {
        format: 1,
        context_digest: digest.clone(),
        proof,
    };
    // Validate the independently read result before reserving immutable storage.
    observation(&record, &fact, None)?;
    let binding = binding(&digest);
    let event_key = canonical::document_digest(&binding).map_err(|_| Error::Execution)?;
    let value = serde_json::to_value(&fact).map_err(|_| Error::Execution)?;
    let fact_digest = canonical::document_digest(&value).map_err(|_| Error::Execution)?;
    let mut journal = FileJournal::new(&record.journal).map_err(|_| Error::Execution)?;
    journal
        .claim(&event_key, &binding)
        .map_err(|_| Error::Execution)?;
    // Only metadata enters Core storage; explicitly retained rows stay private.
    let acknowledged = journal.append(
        &event_key,
        &json!({"kind":KIND,"context_digest":digest,"fact_digest":fact_digest}),
    );
    journal.release(&event_key);
    let journal_ack = acknowledged.map_err(|_| Error::Execution)?;
    records::write_new(
        &stored_path,
        &serde_json::to_value(Stored {
            format: 1,
            event_key,
            journal_ack,
            fact,
        })
        .map_err(|_| Error::Execution)?,
    )?;
    query(path)
}

/// Revalidates retained execution and a previously acknowledged history snapshot.
///
/// Performs no provider execution, directory creation, synchronization or live
/// history read. Saved bytes refer only to the recorded `local_history` boundary;
/// a later history rewrite or model request is outside this query's evidence.
/// No candidate or missing observation is not proof that the source was retained.
///
/// # Errors
/// Rejects corrupt, inconsistent, unsafe or incomplete retained evidence.
pub fn query(path: &Path) -> Result<Value, Error> {
    let (record, digest) = records::load(path)?;
    let returned = records::returned(path, &digest)?;
    let stored_path = records::sibling(path, "observation.json");
    let (observation, saved_bytes) = if records::exists(&stored_path)? {
        let stored: Stored = serde_json::from_value(records::read_json(&stored_path)?)
            .map_err(|_| Error::Execution)?;
        let binding = binding(&digest);
        if stored.format != 1
            || stored.fact.format != 1
            || stored.fact.context_digest != digest
            || stored.event_key
                != canonical::document_digest(&binding).map_err(|_| Error::Execution)?
        {
            return Err(Error::Execution);
        }
        let fact_digest = canonical::document_digest(
            &serde_json::to_value(&stored.fact).map_err(|_| Error::Execution)?,
        )
        .map_err(|_| Error::Execution)?;
        let journal = FileJournal::open_read_only(&record.journal).map_err(|_| Error::Execution)?;
        let entries = journal
            .read_verified(&stored.event_key, &stored.journal_ack)
            .map_err(|_| Error::Execution)?;
        if entries.len() != 2
            || entries[0]["record"]["plan"] != binding
            || entries[1]["record"]
                != json!({"kind":KIND,"context_digest":digest,"fact_digest":fact_digest})
        {
            return Err(Error::Execution);
        }
        observation(&record, &stored.fact, Some(&stored.journal_ack))?
    } else {
        (Value::Null, None)
    };
    Ok(json!({
        "event_key":record.event_key,"execution_decision":record.execution["decision"],
        "prepared":record.projection().is_some_and(|call| call.output.is_some()),
        "returned":returned,"observation_status":observation.get("decision").cloned().unwrap_or(json!("unverified")),
        "proof_boundary":observation.pointer("/proof/boundary"),
        "observation_kind":if observation.is_null() { Value::Null } else { json!("recorded_snapshot") },
        "saved_bytes":saved_bytes,"observation":observation
    }))
}

fn binding(digest: &str) -> Value {
    json!({"kind":KIND,"context_digest":digest})
}

fn observation(
    record: &Record,
    fact: &Fact,
    ack: Option<&Value>,
) -> Result<(Value, Option<u64>), Error> {
    let call = record.projection().ok_or(Error::Execution)?;
    let text = record.history.validate(&fact.proof)?;
    let completed = call.receipt["completed_at_ms"]
        .as_u64()
        .ok_or(Error::Execution)?;
    if fact.proof.observed_at_ms < completed
        || fact.proof.observed_at_ms - completed > record.history.max_observation_delay_ms
    {
        return Err(Error::Execution);
    }
    let source = &call.invocation["input"]["artifact"];
    let candidate = call
        .output
        .as_ref()
        .and_then(|o| o["candidate"]["content"].as_str());
    let (decision, reason) = if candidate == Some(text.as_str()) {
        ("adopted", "candidate_selected")
    } else if source["content"].as_str() == Some(text.as_str()) {
        (
            "preserved",
            if call.output.is_none() {
                "no_candidate"
            } else {
                "environment_rejected"
            },
        )
    } else {
        ("overridden", "later_transform")
    };
    let mut value = json!({
        "invocation_id":call.invocation["invocation_id"],"scope":call.invocation["scope"],
        "boundary_id":call.invocation["boundary_id"],"boundary_revision":call.invocation["boundary_revision"],
        "receipt_digest":canonical::document_digest(&call.receipt).map_err(|_| Error::Execution)?,
        "source_digest":source["digest"],"decision":decision,"reason":reason,
        "effective_digest":canonical::digest(text.as_bytes()),"effective_bytes":text.len(),
        "proof":{"boundary":"local_history","representation_id":crate::history::PROFILE,"revision":1,
            "evidence":fact.proof.evidence(),
            "observed_at_ms":fact.proof.observed_at_ms},
        "ledger_status":if ack.is_some() { "committed" } else { "unavailable" }
    });
    if call.output.is_some() {
        value["candidate_digest"] = call.receipt["output"]["digest"].clone();
    }
    if let Some(ack) = ack {
        value["ledger_evidence"] = ack.clone();
    }
    let registry = Registry::new().map_err(|_| Error::Execution)?;
    let savings = registry
        .validate_plan_adoption(
            &record.plan,
            &record.execution,
            &record.evidence(),
            AdoptionCheck {
                invocation: &call.invocation,
                receipt: &call.receipt,
                output: call.output.as_ref(),
                observation: &value,
                boundary: &record.boundary,
                effective_text: Some(&text),
                recovered_text: None,
                now_ms: crate::wall_time()?,
            },
        )
        .map_err(|_| Error::Execution)?;
    Ok((value, savings))
}
