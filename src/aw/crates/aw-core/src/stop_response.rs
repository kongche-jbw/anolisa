//! Claimed, bounded stop response execution without task-acceptance assertions.

use crate::{
    execute::JournalClaim,
    ports::{Cancellation, HostError, Journal},
    Core, Error,
};
use aw_contracts::{
    canonical,
    events::{EventName, Notification},
    stop_response::StopResponse,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// One explicitly configured response command under the caller's shared deadline.
pub struct StopResponseCommand<'a> {
    /// Trusted configuration identifier, never selected by stop payload.
    pub id: &'a str,
    /// Callback deadline, including any preceding notification work.
    pub deadline: Instant,
}

/// Explicitly trusted response command, distinct from notification-only runners.
pub trait StopResponseHost {
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
    /// Claims one main-agent stop occurrence and validates its response.
    ///
    /// `None` means unavailable or repeated checking; the native mapping must stop
    /// further work with diagnostic fields, never request another continuation.
    /// The embedding authenticates the envelope and fences attachment changes.
    /// A returned response proves neither native adoption nor model consumption.
    ///
    /// # Errors
    /// Rejects invalid stop identity, duplicate claims and unavailable journals.
    pub fn respond_to_stop(
        &self,
        event: &Notification,
        command: &StopResponseCommand<'_>,
        host: &mut impl StopResponseHost,
        journal: &mut impl Journal,
        cancellation: &dyn Cancellation,
    ) -> Result<Option<StopResponse>, Error> {
        let deadline = command
            .deadline
            .min(Instant::now() + Duration::from_secs(2));
        let command = command.id;
        let metadata = event.metadata()?;
        if event.event != EventName::TurnStop
            || event.session_id.is_none()
            || event.subagent_id.is_some()
            || !event.payload["stop_hook_active"].is_boolean()
            || command.is_empty()
            || command.len() > 256
        {
            return Err(Error::Preparation("invalid stop response request"));
        }
        let key = canonical::document_digest(&json!([event.event_key()?, "stop_response"]))?;
        let ack = journal.claim(&key, &json!({"stop_response":metadata,"command":command}))?;
        let reservation = JournalClaim {
            journal,
            event_key: &key,
        };
        self.validate_journal_ack(ack)?;
        // Native retry checks do not prove a new task. Never recursively request
        // more work, even if the configured command would always ask to continue.
        if event.payload["stop_hook_active"] == true {
            self.validate_journal_ack(reservation.journal.append(
                &key,
                &json!({"kind":"stop_response_skipped","reason":"native_stop_hook_active",
                    "native_adoption":"unconfirmed"}),
            )?)?;
            return Ok(None);
        }
        let active = || !cancellation.is_cancelled() && Instant::now() < deadline;
        let response = (|| -> Result<StopResponse, Error> {
            self.validate_journal_ack(reservation.journal.append(
                &key,
                &json!({"kind":"stop_response_started","command":command}),
            )?)?;
            let remaining_ms = deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64;
            if !active() || remaining_ms == 0 {
                return Err(Error::Preparation("stop response cancelled or expired"));
            }
            let output = host.respond(
                command,
                &json!({"format":1,"scope":"turn.stop.respond","event":event}),
                remaining_ms,
                cancellation,
            )?;
            if !active() {
                return Err(Error::Preparation("stop response cancelled or expired"));
            }
            Ok(StopResponse::parse(&output)?)
        })()
        .ok();
        let decision = match &response {
            Some(StopResponse::AllowStop) => "allow_stop",
            Some(StopResponse::Continue(_)) => "continue",
            None => "failed_or_cancelled",
        };
        self.validate_journal_ack(reservation.journal.append(
            &key,
            &json!({"kind":"stop_response_completed","decision":decision,
                "native_adoption":"unconfirmed"}),
        )?)?;
        // Durable completion may itself overlap cancellation or consume the budget.
        if !active() {
            self.validate_journal_ack(reservation.journal.append(
                &key,
                &json!({"kind":"stop_response_withheld","reason":"cancelled_or_expired"}),
            )?)?;
            return Ok(None);
        }
        Ok(response)
    }
}
