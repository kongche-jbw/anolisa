//! Claimed optional result responses with source binding and metadata-only journaling.

use crate::{
    execute::JournalClaim,
    ports::{Cancellation, HostError, Journal},
    Core, Error,
};
use aw_contracts::{
    canonical,
    events::{EventName, Notification},
    tool_response::ToolResponse,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// A trusted response command and the text selected by the native adapter.
pub struct ToolResponseCommand<'a> {
    /// Operator-selected command identifier.
    pub id: &'a str,
    /// Unmodified supported result text, not a fabricated AW Turn.
    pub source: &'a str,
    /// Original callback deadline, including preceding notification work.
    pub deadline: Instant,
}

/// Explicit replacement authority, separate from notification acknowledgements.
pub trait ToolResponseHost {
    /// Runs a bounded command; failures never imply a successful projection.
    ///
    /// # Errors
    /// Reports admission, execution or output failure without private content.
    fn respond(
        &mut self,
        command: &str,
        request: &Value,
        remaining_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<Value, HostError>;
}

impl Core {
    /// Claims one authenticated tool result and validates an optional replacement.
    ///
    /// The embedding selects a supported successful result, authenticates identity
    /// and fences attachment changes. `None` preserves the original result with a
    /// diagnostic. Journals retain digests, never source or candidate text, and do
    /// not establish native adoption, security inspection or lossless recovery.
    ///
    /// # Errors
    /// Rejects invalid identity, duplicate claims and unavailable journal evidence.
    pub fn respond_to_tool(
        &self,
        event: &Notification,
        command: &ToolResponseCommand<'_>,
        host: &mut impl ToolResponseHost,
        journal: &mut impl Journal,
        cancellation: &dyn Cancellation,
    ) -> Result<Option<ToolResponse>, Error> {
        let deadline = command
            .deadline
            .min(Instant::now() + Duration::from_secs(2));
        let metadata = event.metadata()?;
        if event.event != EventName::ToolAfter
            || event.session_id.is_none()
            || event.tool_call_id.is_none()
            || event.subagent_id.is_some()
            || command.id.is_empty()
            || command.id.len() > 256
            || command.source.len() > 1048576
        {
            return Err(Error::Preparation("invalid tool response request"));
        }
        let source_digest = canonical::digest(command.source.as_bytes());
        let key = canonical::document_digest(&json!([event.event_key()?, "tool_response"]))?;
        let ack = journal.claim(
            &key,
            &json!({"tool_response":metadata,
            "command":command.id,"source_digest":source_digest}),
        )?;
        let reservation = JournalClaim {
            journal,
            event_key: &key,
        };
        self.validate_journal_ack(ack)?;
        let active = || !cancellation.is_cancelled() && Instant::now() < deadline;
        let response = (|| -> Result<ToolResponse, Error> {
            self.validate_journal_ack(reservation.journal.append(
                &key,
                &json!({"kind":"tool_response_started","command":command.id}),
            )?)?;
            let remaining_ms = deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64;
            if !active() || remaining_ms == 0 {
                return Err(Error::Preparation("tool response cancelled or expired"));
            }
            let output = host.respond(
                command.id,
                &json!({"format":1,"scope":"tool.after.respond","event":event,
                    "source":{"text":command.source,"digest":source_digest},
                    "accepted_reversibility":["unrecoverable"]}),
                remaining_ms,
                cancellation,
            )?;
            if !active() {
                return Err(Error::Preparation("tool response cancelled or expired"));
            }
            Ok(ToolResponse::parse(&output, &source_digest)?)
        })()
        .ok();
        let (decision, digest) = match &response {
            Some(ToolResponse::Preserve) => ("preserve", None),
            Some(ToolResponse::Replace(text)) => {
                ("replace", Some(canonical::digest(text.as_bytes())))
            }
            None => ("failed_or_cancelled", None),
        };
        self.validate_journal_ack(reservation.journal.append(
            &key,
            &json!({"kind":"tool_response_completed","decision":decision,
                "candidate_digest":digest,"native_adoption":"unconfirmed"}),
        )?)?;
        if !active() {
            self.validate_journal_ack(reservation.journal.append(
                &key,
                &json!({"kind":"tool_response_withheld","reason":"cancelled_or_expired"}),
            )?)?;
            return Ok(None);
        }
        Ok(response)
    }
}
