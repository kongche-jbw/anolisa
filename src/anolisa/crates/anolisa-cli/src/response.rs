//! Unified CLI response envelope, error model, and renderer.
//!
//! Both human-readable and `--json` output flow through the same
//! [`CliResponse`] envelope (see launch spec §4). Handlers may render
//! their own human text directly to stdout, and on `--json` they hand a
//! payload to [`render_json`] / [`render_error`] so the on-the-wire
//! shape stays consistent across surfaces.
//!
//! Exit codes:
//! - `NOT_IMPLEMENTED` -> 64 (reserved CLI code for "command exists but
//!   handler is not wired"; chosen because POSIX `EX_USAGE` is 64 and is
//!   the closest established sentinel — launch spec §4 does not pin an
//!   exact value, so we pick a non-zero reserved code and document it
//!   here for future tightening).
//! - `INVALID_ARGUMENT` -> 2 (POSIX convention shared with clap).

use std::process::ExitCode;

use serde::Serialize;

use crate::context::CliContext;

/// JSON schema version for the CLI response envelope. Bump when the
/// envelope shape changes.
pub const SCHEMA_VERSION: u32 = 1;

/// Common envelope shared by human and JSON output paths.
#[derive(Debug, Serialize)]
pub struct CliResponse<T: Serialize> {
    pub ok: bool,
    pub schema_version: u32,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CliErrorPayload>,
}

#[derive(Debug, Serialize)]
pub struct CliErrorPayload {
    pub code: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Errors a handler can surface. The dispatcher converts these into the
/// process exit code via [`render_error`].
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Command exists in the surface but no real implementation yet.
    #[error("command '{command}' is not implemented")]
    NotImplemented {
        command: String,
        hint: Option<String>,
    },

    /// Caller-supplied arguments violated a contract.
    #[error("invalid argument: {reason}")]
    InvalidArgument { command: String, reason: String },
}

impl CliError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotImplemented { .. } => "NOT_IMPLEMENTED",
            Self::InvalidArgument { .. } => "INVALID_ARGUMENT",
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NotImplemented { .. } => 64,
            Self::InvalidArgument { .. } => 2,
        }
    }

    pub fn command(&self) -> &str {
        match self {
            Self::NotImplemented { command, .. } => command,
            Self::InvalidArgument { command, .. } => command,
        }
    }

    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::NotImplemented { hint, .. } => hint.as_deref(),
            Self::InvalidArgument { .. } => None,
        }
    }

    pub fn reason(&self) -> String {
        match self {
            Self::NotImplemented { command, .. } => {
                format!("command '{command}' is not implemented")
            }
            Self::InvalidArgument { reason, .. } => reason.clone(),
        }
    }

    pub fn not_implemented(command: impl Into<String>) -> Self {
        Self::NotImplemented {
            command: command.into(),
            hint: None,
        }
    }

    pub fn not_implemented_with_hint(command: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::NotImplemented {
            command: command.into(),
            hint: Some(hint.into()),
        }
    }
}

/// Print a successful JSON envelope to stdout. Callers should only invoke
/// this on the `--json` branch (human path stays plain `println!`).
pub fn render_json<T: Serialize>(command: &str, data: T) -> Result<(), CliError> {
    let response = CliResponse {
        ok: true,
        schema_version: SCHEMA_VERSION,
        command: command.to_string(),
        data: Some(data),
        warnings: Vec::new(),
        error: None,
    };
    write_json(&response);
    Ok(())
}

/// Print an empty success envelope (no data payload).
#[allow(dead_code)]
pub fn render_ok(command: &str) {
    let response: CliResponse<()> = CliResponse {
        ok: true,
        schema_version: SCHEMA_VERSION,
        command: command.to_string(),
        data: None,
        warnings: Vec::new(),
        error: None,
    };
    write_json(&response);
}

/// Render an error and return the process exit code to surface.
///
/// On `--json` we emit a `CliResponse` envelope on stdout (so machine
/// callers always get parseable output, error or not). On the human path
/// we write to stderr per launch spec §4 ("warnings/debug to stderr").
pub fn render_error(ctx: &CliContext, err: &CliError) -> ExitCode {
    if ctx.json {
        let payload = CliErrorPayload {
            code: err.code().to_string(),
            reason: err.reason(),
            hint: err.hint().map(|s| s.to_string()),
        };
        let response: CliResponse<()> = CliResponse {
            ok: false,
            schema_version: SCHEMA_VERSION,
            command: err.command().to_string(),
            data: None,
            warnings: Vec::new(),
            error: Some(payload),
        };
        write_json(&response);
    } else {
        eprintln!("error[{}]: {}", err.code(), err.reason());
        if let Some(hint) = err.hint() {
            eprintln!("hint: {}", hint);
        }
    }
    ExitCode::from(err.exit_code())
}

fn write_json<T: Serialize>(response: &CliResponse<T>) {
    match serde_json::to_string_pretty(response) {
        Ok(s) => println!("{}", s),
        Err(e) => eprintln!("internal: failed to serialize response: {e}"),
    }
}
