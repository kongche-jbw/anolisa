//! Explicit main-agent stop authority, separate from observation and task success.

use crate::{require, Error};
use serde_json::Value;

/// A stop check may accept stopping or request one bounded continuation.
#[derive(Debug, PartialEq, Eq)]
pub enum StopResponse {
    /// Permit native stopping without asserting that the task succeeded.
    AllowStop,
    /// Ask the native owner to continue with the supplied reason.
    Continue(String),
}

impl StopResponse {
    /// Parses the exact `turn.stop.respond` v1 output without native authority fields.
    ///
    /// # Errors
    /// Rejects mixed decisions, extra fields, empty reasons and oversized text.
    pub fn parse(value: &Value) -> Result<Self, Error> {
        let object = value.as_object().ok_or(Error::InvalidDocument)?;
        require(value["format"] == 1, "unsupported stop response revision")?;
        match value["decision"].as_str() {
            Some("allow_stop") => {
                require(object.len() == 2, "unexpected stop response field")?;
                Ok(Self::AllowStop)
            }
            Some("continue") => {
                require(object.len() == 3, "unexpected stop response field")?;
                let reason = value["reason"].as_str().ok_or(Error::InvalidDocument)?;
                require(
                    !reason.trim().is_empty() && reason.len() <= 16384 && !reason.contains('\0'),
                    "invalid stop response reason",
                )?;
                Ok(Self::Continue(reason.into()))
            }
            _ => Err(Error::Invariant("invalid stop response decision")),
        }
    }
}
