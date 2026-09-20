//! Explicit optional result replacement, without tool execution or safety authority.

use crate::{require, Error};
use serde_json::Value;

/// A result handler may preserve the native result or replace its supported text.
#[derive(Debug, PartialEq, Eq)]
pub enum ToolResponse {
    /// Leave the native result unchanged.
    Preserve,
    /// Replace text after the caller explicitly accepts unrecoverable projection.
    Replace(String),
}

impl ToolResponse {
    /// Parses `tool.after.respond` v1 and binds replacement to the supplied source.
    ///
    /// # Errors
    /// Rejects foreign digests, native control fields, mixed decisions and invalid text.
    pub fn parse(value: &Value, source_digest: &str) -> Result<Self, Error> {
        let object = value.as_object().ok_or(Error::InvalidDocument)?;
        require(value["format"] == 1, "unsupported tool response revision")?;
        match value["decision"].as_str() {
            Some("preserve") => {
                require(object.len() == 2, "unexpected tool response field")?;
                Ok(Self::Preserve)
            }
            Some("replace") => {
                require(object.len() == 4, "unexpected tool response field")?;
                require(
                    value["source_digest"] == source_digest,
                    "tool source changed",
                )?;
                let text = value["text"].as_str().ok_or(Error::InvalidDocument)?;
                require(
                    !text.is_empty() && text.len() <= 65536 && !text.contains('\0'),
                    "invalid tool replacement text",
                )?;
                Ok(Self::Replace(text.into()))
            }
            _ => Err(Error::Invariant("invalid tool response decision")),
        }
    }
}
