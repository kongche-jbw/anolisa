//! Explicit input response authority, separate from notification acknowledgements.

use crate::{require, Error};
use serde_json::Value;

/// Validated v1 response; it cannot rewrite the submitted prompt or grant permission.
#[derive(Debug, PartialEq, Eq)]
pub enum InputResponse {
    /// Continue native handling, optionally adding bounded context.
    Continue(Option<String>),
    /// Reject this submitted input with a reason for the native owner.
    Reject(String),
}

impl InputResponse {
    /// Parses the exact `input.submit.respond` v1 output, rejecting extra fields.
    ///
    /// # Errors
    /// Rejects mixed decisions, native fields, missing reasons and oversized text.
    pub fn parse(value: &Value) -> Result<Self, Error> {
        let object = value.as_object().ok_or(Error::InvalidDocument)?;
        require(value["format"] == 1, "unsupported input response revision")?;
        let text = |key: &str| -> Result<String, Error> {
            let text = value[key].as_str().ok_or(Error::InvalidDocument)?;
            require(
                !text.trim().is_empty() && text.len() <= 16384 && !text.contains('\0'),
                "invalid input response text",
            )?;
            Ok(text.into())
        };
        match value["decision"].as_str() {
            Some("continue") => {
                require(
                    object.keys().all(|k| {
                        matches!(k.as_str(), "format" | "decision" | "additional_context")
                    }),
                    "unexpected input response field",
                )?;
                Ok(Self::Continue(
                    if object.contains_key("additional_context") {
                        Some(text("additional_context")?)
                    } else {
                        None
                    },
                ))
            }
            Some("reject") => {
                require(
                    object.len() == 3 && object.contains_key("reason"),
                    "invalid input rejection fields",
                )?;
                Ok(Self::Reject(text("reason")?))
            }
            _ => Err(Error::Invariant("invalid input response decision")),
        }
    }
}
