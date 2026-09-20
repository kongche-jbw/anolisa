//! Ordered candidate transformations followed by one mandatory read-only check.

use crate::{
    execute::JournalClaim,
    ports::{Cancellation, HostError, Journal},
    Core, Error,
};
use aw_contracts::{
    canonical,
    events::{EventName, Notification},
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

/// Trusted commands selected by configuration, never by tool arguments.
pub struct ToolChain {
    /// Caller-owned monotonic deadline, capped by Core at two seconds.
    pub deadline: Instant,
    /// At most three transformations, applied in this exact order.
    pub transforms: Vec<String>,
    /// Final read-only policy check; it cannot transform the candidate.
    pub guard: String,
}

/// Host boundary for a native tool check, without final-consumption authority.
pub trait ToolHost {
    /// Validates the entire route before any transformation can run.
    fn admit(&self, chain: &ToolChain) -> Result<(), HostError>;
    /// Validates the candidate against the native tool's writable field contract.
    fn validate(&self, candidate: &Value) -> Result<(), HostError>;
    /// Returns a new candidate; the Host enforces remaining time and cancellation.
    fn transform(
        &mut self,
        command: &str,
        candidate: &Value,
        remaining_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<Value, HostError>;
    /// Checks the exact final candidate, returning true only for a valid allow.
    fn guard(
        &mut self,
        command: &str,
        candidate: &Value,
        remaining_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<bool, HostError>;
}

/// A local decision, never proof of native execution or actual blocking.
pub struct ToolDecision {
    candidate: Option<Value>,
    record: Value,
}
impl ToolDecision {
    /// The approved immutable candidate; absent on every denied or failed check.
    pub fn candidate(&self) -> Option<&Value> {
        self.candidate.as_ref()
    }
    /// Metadata-only completion record; includes no candidate or scanner output.
    pub fn record(&self) -> &Value {
        &self.record
    }
}

impl Core {
    /// Joins synchronous transformations before the final check under one budget.
    ///
    /// The embedding authenticates the tool occurrence and must consume the same
    /// candidate or state its weaker native guarantee. Hosts must bound every call;
    /// Core cannot preempt a synchronous Host. No legacy final capability is granted.
    ///
    /// # Errors
    /// Invalid admission, journal failure or duplicate claims return no candidate.
    /// Once claimed, command errors are recorded as rejection, never as fallback.
    pub fn check_tool(
        &self,
        event: &Notification,
        candidate: Value,
        chain: &ToolChain,
        host: &mut impl ToolHost,
        journal: &mut impl Journal,
        cancellation: &dyn Cancellation,
    ) -> Result<ToolDecision, Error> {
        let deadline = chain.deadline.min(Instant::now() + Duration::from_secs(2));
        let metadata = event.metadata()?;
        if event.event != EventName::ToolBefore
            || event.tool_call_id.is_none()
            || event.session_id.is_none()
            || event.session_epoch.is_none()
            || chain.transforms.len() > 3
        {
            return Err(Error::Preparation("invalid tool check"));
        }
        let ids: Vec<_> = chain
            .transforms
            .iter()
            .chain(std::iter::once(&chain.guard))
            .collect();
        if ids.iter().any(|s| s.is_empty() || s.len() > 256)
            || ids.iter().collect::<HashSet<_>>().len() != ids.len()
        {
            return Err(Error::Preparation("invalid tool chain commands"));
        }
        validate_size(&candidate)?;
        host.admit(chain)?;
        host.validate(&candidate)?;
        let key = canonical::document_digest(&json!([event.event_key()?, "tool_check"]))?;
        let input_digest = canonical::document_digest(&candidate)?;
        let ack = journal.claim(
            &key,
            &json!({"tool_check":metadata,
            "input_digest":input_digest,"transforms":chain.transforms,"guard":chain.guard}),
        )?;
        let reservation = JournalClaim {
            journal,
            event_key: &key,
        };
        self.validate_journal_ack(ack)?;
        let mut current = candidate;
        let mut steps = Vec::new();
        let result = (|| -> Result<bool, Error> {
            for command in &chain.transforms {
                let digest = canonical::document_digest(&current)?;
                self.validate_journal_ack(reservation.journal.append(
                    &key,
                    &json!({"kind":"transform_started","command":command,"input_digest":digest}),
                )?)?;
                let next = host.transform(
                    command,
                    &current,
                    remaining(deadline, cancellation)?,
                    cancellation,
                )?;
                remaining(deadline, cancellation)?;
                validate_size(&next)?;
                host.validate(&next)?;
                // Transformations may change arguments, never the trusted context.
                if next["tool_name"] != current["tool_name"] || next["cwd"] != current["cwd"] {
                    return Err(Error::Preparation("tool context changed"));
                }
                current = next;
                let step = json!({"command":command,"input_digest":digest,
                    "output_digest":canonical::document_digest(&current)?});
                self.validate_journal_ack(reservation.journal.append(&key, &step)?)?;
                steps.push(step);
            }
            let digest = canonical::document_digest(&current)?;
            self.validate_journal_ack(reservation.journal.append(
                &key,
                &json!({"kind":"guard_started","command":chain.guard,"input_digest":digest}),
            )?)?;
            let allowed = host.guard(
                &chain.guard,
                &current,
                remaining(deadline, cancellation)?,
                cancellation,
            )?;
            remaining(deadline, cancellation)?;
            Ok(allowed)
        })();
        let (allowed, reason) = match result {
            Ok(true) => (true, "check_passed"),
            Ok(false) => (false, "policy_denied"),
            Err(_) => (false, "check_failed_or_cancelled"),
        };
        let mut record = json!({"kind":"tool_check_completed","event_key":key,
            "input_digest":input_digest,"candidate_digest":canonical::document_digest(&current)?,
            "allowed":allowed,"reason":reason,"transforms":steps,
            "native_execution":"unconfirmed"});
        self.validate_journal_ack(reservation.journal.append(&key, &record)?)?;
        // Durable writes can overlap cancellation or exhaust the deadline.
        let allowed = allowed && remaining(deadline, cancellation).is_ok();
        if record["allowed"] != allowed {
            record["allowed"] = json!(false);
            record["reason"] = json!("cancelled_or_budget_exhausted_before_return");
            self.validate_journal_ack(reservation.journal.append(&key, &record)?)?;
        }
        Ok(ToolDecision {
            candidate: allowed.then_some(current),
            record,
        })
    }
}

fn remaining(deadline: Instant, cancellation: &dyn Cancellation) -> Result<u64, Error> {
    let ms = deadline
        .saturating_duration_since(Instant::now())
        .as_millis() as u64;
    if ms == 0 || cancellation.is_cancelled() {
        return Err(Error::Preparation(
            "tool check cancelled or deadline exceeded",
        ));
    }
    Ok(ms)
}
fn validate_size(candidate: &Value) -> Result<(), Error> {
    if !candidate.is_object() || canonical::bytes(candidate)?.len() > 1024 * 1024 {
        return Err(Error::Preparation("invalid tool candidate"));
    }
    Ok(())
}
