//! Public lifecycle names and notification envelopes, independent of native powers.

use crate::{canonical, require, Error};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Public configuration vocabulary; a name does not grant control or prove support.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventName {
    /// A logical session is created, loaded or restored.
    #[serde(rename = "session.start")]
    SessionStart,
    /// Input reaches a submission point, without asserting task acceptance.
    #[serde(rename = "input.submit")]
    InputSubmit,
    /// A tool intent reaches its native pre-execution hook.
    #[serde(rename = "tool.before")]
    ToolBefore,
    /// A tool result, including failure, reaches its native hook.
    #[serde(rename = "tool.after")]
    ToolAfter,
    /// The native owner requests a permission decision.
    #[serde(rename = "permission.request")]
    PermissionRequest,
    /// Context compaction is about to begin.
    #[serde(rename = "compact.before")]
    CompactBefore,
    /// The native owner reports a compaction result.
    #[serde(rename = "compact.after")]
    CompactAfter,
    /// A native subagent is started, not merely an OS child process.
    #[serde(rename = "subagent.start")]
    SubagentStart,
    /// A native subagent reaches its stopping point.
    #[serde(rename = "subagent.stop")]
    SubagentStop,
    /// The current task reaches its stop check, without asserting success.
    #[serde(rename = "turn.stop")]
    TurnStop,
    /// The native owner reports the end of a session instance.
    #[serde(rename = "session.end")]
    SessionEnd,
    /// An assembled model request reaches a verified sending boundary.
    #[serde(rename = "model.before_request")]
    ModelBeforeRequest,
    /// A trusted owner registers a runtime, without asserting readiness.
    #[serde(rename = "runtime.observed")]
    RuntimeObserved,
    /// A trusted process source observes the root runtime exit.
    #[serde(rename = "runtime.exited")]
    RuntimeExited,
    /// Provisional name for the final active safety check; notify grants no blocking authority.
    #[serde(rename = "security.violation")]
    SecurityViolation,
    /// A source reports a change in its actual coverage.
    #[serde(rename = "coverage.changed")]
    CoverageChanged,
}

/// Notification-only v1 envelope. Callers authenticate the source and identity.
///
/// Unknown task identity stays null. Payloads may contain private native content;
/// pass them to explicitly trusted commands, never store them in metadata journals.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Notification {
    /// Exact envelope revision; currently 1.
    pub format: u32,
    /// Public event selected by the source adapter.
    pub event: EventName,
    /// Trusted producer classification, not a native self-reported authority.
    pub source: String,
    /// Original hook name where applicable.
    pub native_event: Option<String>,
    /// Runtime-owner-assigned identity.
    pub runtime_id: String,
    /// Runtime generation, distinct from session attachment.
    pub runtime_generation: u64,
    /// Native logical session identity when known.
    pub session_id: Option<String>,
    /// Owner-assigned attachment epoch when known.
    pub session_epoch: Option<u64>,
    /// Only populated from demonstrated task acceptance evidence.
    pub turn_id: Option<String>,
    /// Explains why this source cannot establish an AW Turn.
    pub turn_unknown_reason: Option<String>,
    /// Native tool correlation, if supplied.
    pub tool_call_id: Option<String>,
    /// Native child Agent identity, never inferred from a PID.
    pub subagent_id: Option<String>,
    /// Owner-assigned occurrence; repeated legitimate callbacks may be distinct.
    pub occurrence_id: String,
    /// Digest of the explicitly trusted command configuration.
    pub config_revision: String,
    /// Exact native input or source-specific fact, preserved without reshaping.
    pub payload: Value,
}

impl Notification {
    /// Checks bounded structural invariants; this does not authenticate the source.
    pub fn validate(&self) -> Result<(), Error> {
        require(self.format == 1, "unsupported notification revision")?;
        for id in [
            &self.runtime_id,
            &self.occurrence_id,
            &self.config_revision,
            &self.source,
        ] {
            require(
                !id.is_empty() && id.len() <= 256,
                "invalid notification identity",
            )?;
        }
        for id in [
            &self.session_id,
            &self.turn_id,
            &self.tool_call_id,
            &self.subagent_id,
            &self.native_event,
            &self.turn_unknown_reason,
        ]
        .into_iter()
        .flatten()
        {
            require(
                !id.is_empty() && id.len() <= 256,
                "invalid optional identity",
            )?;
        }
        require(
            self.turn_id.is_some() != self.turn_unknown_reason.is_some(),
            "turn requires either evidence or an unknown reason",
        )?;
        require(
            self.session_id.is_some() == self.session_epoch.is_some(),
            "session requires an attachment epoch",
        )?;
        require(
            self.payload.is_object(),
            "notification payload must be an object",
        )?;
        let bytes = serde_json::to_vec(self).map_err(|_| Error::InvalidDocument)?;
        require(
            bytes.len() <= 1024 * 1024,
            "notification exceeds input budget",
        )
    }

    /// Metadata for durable claims; raw payload never enters the journal.
    pub fn metadata(&self) -> Result<Value, Error> {
        self.validate()?;
        let mut value = serde_json::to_value(self).map_err(|_| Error::InvalidDocument)?;
        value
            .as_object_mut()
            .ok_or(Error::InvalidDocument)?
            .remove("payload");
        value["payload_digest"] = json!(canonical::document_digest(&self.payload)?);
        Ok(value)
    }

    /// A configuration change cannot reacquire the same producer occurrence.
    pub fn event_key(&self) -> Result<String, Error> {
        self.validate()?;
        canonical::document_digest(&json!([
            self.runtime_id,
            self.runtime_generation,
            self.source,
            self.occurrence_id
        ]))
    }
}
