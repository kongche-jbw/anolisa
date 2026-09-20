//! Bounded, claimed notification commands; no dispatch, transform or guard authority.

use crate::{
    execute::JournalClaim,
    ports::{Cancellation, HostError, Journal},
    Core, Error,
};
use aw_contracts::events::Notification;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

/// Trusted embedding's command runner; IDs resolve only within approved config.
pub trait NotificationHost {
    /// Runs one command with at most the remaining chain budget.
    ///
    /// The Host enforces time/output limits and cancellation. Only the exact
    /// `{"format":1,"observed":true}` acknowledgement denotes completion;
    /// arbitrary stdout cannot become a native response or control decision.
    ///
    /// # Errors
    /// Reports bounded process, pin or transport failures without private output.
    fn notify(
        &mut self,
        command: &str,
        event: &Notification,
        remaining_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<Value, HostError>;
}

impl Core {
    /// Executes at most four optional notifications under a two-second budget.
    ///
    /// Claims survive failure and cancellation. Terminal runtime notifications
    /// use this separate authority, never a fabricated running capability plan.
    /// All identity/source facts must already be authenticated by the embedding.
    ///
    /// # Errors
    /// Rejects malformed envelopes, duplicate command IDs, duplicate occurrences
    /// and unavailable journals. Command failures are recorded as coverage gaps.
    pub fn notify(
        &self,
        event: &Notification,
        commands: &[String],
        host: &mut impl NotificationHost,
        journal: &mut impl Journal,
        cancellation: &dyn Cancellation,
    ) -> Result<Value, Error> {
        let metadata = event.metadata()?;
        if commands.is_empty()
            || commands.len() > 4
            || commands.iter().any(|id| id.is_empty() || id.len() > 256)
            || commands.iter().collect::<HashSet<_>>().len() != commands.len()
        {
            return Err(Error::Preparation("invalid notification commands"));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let key = event.event_key()?;
        let ack = journal.claim(&key, &json!({"notification":metadata,"commands":commands}))?;
        let reservation = JournalClaim {
            journal,
            event_key: &key,
        };
        self.validate_journal_ack(ack)?;
        let mut results = Vec::new();
        for command in commands {
            let mut outcome = "skipped";
            let mut reason = Some("cancelled_or_budget_exhausted".to_owned());
            if !cancellation.is_cancelled() && Instant::now() < deadline {
                self.validate_journal_ack(reservation.journal.append(
                    &key,
                    &json!({"kind":"notification_started","command":command}),
                )?)?;
                // fsync may consume the remaining budget or overlap cancellation.
                let remaining = deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis() as u64;
                if remaining > 0 && !cancellation.is_cancelled() {
                    let result = host.notify(command, event, remaining, cancellation);
                    outcome = "failed";
                    if cancellation.is_cancelled() {
                        reason = Some("cancelled".into());
                    } else if Instant::now() > deadline {
                        reason = Some("deadline_exceeded".into());
                    } else {
                        match result {
                            Ok(ack) if ack == json!({"format":1,"observed":true}) => {
                                outcome = "completed";
                                reason = None;
                            }
                            Ok(_) => reason = Some("invalid_notification_ack".into()),
                            Err(error) => reason = Some(error.code),
                        }
                    }
                }
            }
            let result = json!({"command":command,"outcome":outcome,"reason":reason});
            self.validate_journal_ack(reservation.journal.append(&key, &result)?)?;
            results.push(result);
        }
        let report = json!({"kind":"notification_completed","event_key":key,
            "gap":results.iter().any(|r| r["outcome"] != "completed"),"commands":results});
        self.validate_journal_ack(reservation.journal.append(&key, &report)?)?;
        Ok(report)
    }
}
