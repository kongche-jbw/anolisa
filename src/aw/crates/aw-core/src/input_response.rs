//! Claimed, bounded input response execution without task-acceptance assertions.

use crate::{
    execute::JournalClaim,
    ports::{Cancellation, HostError, Journal},
    Core, Error,
};
use aw_contracts::{
    canonical,
    events::{EventName, Notification},
    input_response::InputResponse,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// One explicitly configured response command under the caller's shared deadline.
pub struct InputResponseCommand<'a> {
    /// Trusted configuration identifier, never selected by input payload.
    pub id: &'a str,
    /// Callback deadline, including any preceding notification work.
    pub deadline: Instant,
}

/// Explicitly trusted response command, distinct from notification-only runners.
pub trait InputResponseHost {
    /// Runs the selected command with a bounded, versioned response request.
    ///
    /// # Errors
    /// Reports command/pin/output failures without exposing private content.
    fn respond(
        &mut self,
        command: &str,
        request: &Value,
        remaining_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<Value, HostError>;
}

impl Core {
    /// Claims one input occurrence and validates its response before native mapping.
    ///
    /// `None` means command failure/cancellation and must be mapped to rejection.
    /// The embedding authenticates the envelope and fences attachment changes.
    /// A returned response proves neither native adoption nor model consumption.
    ///
    /// # Errors
    /// Rejects invalid input identity, duplicate claims and unavailable journals.
    pub fn respond_to_input(
        &self,
        event: &Notification,
        command: &InputResponseCommand<'_>,
        host: &mut impl InputResponseHost,
        journal: &mut impl Journal,
        cancellation: &dyn Cancellation,
    ) -> Result<Option<InputResponse>, Error> {
        let deadline = command
            .deadline
            .min(Instant::now() + Duration::from_secs(2));
        let command = command.id;
        let metadata = event.metadata()?;
        if event.event != EventName::InputSubmit
            || event.session_id.is_none()
            || event.subagent_id.is_some()
            || command.is_empty()
            || command.len() > 256
        {
            return Err(Error::Preparation("invalid input response request"));
        }
        let key = canonical::document_digest(&json!([event.event_key()?, "input_response"]))?;
        let ack = journal.claim(&key, &json!({"input_response":metadata,"command":command}))?;
        let reservation = JournalClaim {
            journal,
            event_key: &key,
        };
        self.validate_journal_ack(ack)?;
        let active = || !cancellation.is_cancelled() && Instant::now() < deadline;
        let response = (|| -> Result<InputResponse, Error> {
            self.validate_journal_ack(reservation.journal.append(
                &key,
                &json!({"kind":"input_response_started","command":command}),
            )?)?;
            let remaining_ms = deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64;
            if !active() || remaining_ms == 0 {
                return Err(Error::Preparation("input response cancelled or expired"));
            }
            let output = host.respond(
                command,
                &json!({"format":1,"scope":"input.submit.respond","event":event}),
                remaining_ms,
                cancellation,
            )?;
            if !active() {
                return Err(Error::Preparation("input response cancelled or expired"));
            }
            Ok(InputResponse::parse(&output)?)
        })()
        .ok();
        let decision = match &response {
            Some(InputResponse::Continue(_)) => "continue",
            Some(InputResponse::Reject(_)) => "reject",
            None => "failed_or_cancelled",
        };
        self.validate_journal_ack(reservation.journal.append(
            &key,
            &json!({"kind":"input_response_completed","decision":decision,
                "native_adoption":"unconfirmed"}),
        )?)?;
        // Durable completion may itself overlap cancellation or consume the budget.
        if !active() {
            self.validate_journal_ack(reservation.journal.append(
                &key,
                &json!({"kind":"input_response_withheld","reason":"cancelled_or_expired"}),
            )?)?;
            return Ok(None);
        }
        Ok(response)
    }
}
