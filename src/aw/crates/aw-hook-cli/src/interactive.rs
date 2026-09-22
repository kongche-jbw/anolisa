//! Experimental native interactive attachment; the embedding shell owns the PTY.
//!
//! Optional native Bash guards do not grant final dispatch or OS protection authority.
//! Private local state detects accidental misbinding, not a same-user attacker.

mod activity;
mod config;
mod effects;
mod event_view;
mod herdr;
mod hooks;
mod input_response;
mod launch;
mod notifications;
mod runtime_events;
mod stop_response;
mod storage;
mod tool_guard;
mod tool_response;
mod view;
mod workspace;

pub use config::prepare;
pub use herdr::HerdrBridge;
pub use hooks::{callback, callback_with_cancellation, guard_callback_with_cancellation};
pub use hooks::{input_callback_with_cancellation, stop_callback_with_cancellation};
pub use launch::{check_launch, launch, launch_with_cancellation};
pub use runtime_events::RuntimeObserver;
pub use view::query;
pub use workspace::tokens as workspace_tokens;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Failure at the interactive admission or evidence boundary.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A profile or operator declaration cannot be honored.
    #[error("AW interactive profile: {0}")]
    Profile(&'static str),
    /// Local evidence is unavailable; details never include native payloads.
    #[error("AW interactive evidence unavailable")]
    Evidence,
    /// An owned local artifact could not be accessed.
    #[error("AW interactive storage: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    format: u32,
    qoder: NativeExecutable,
    native_config_directory: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    handler: Option<aw_host_process::Config>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notifications: Option<
        std::collections::BTreeMap<aw_contracts::events::EventName, Vec<aw_host_process::Config>>,
    >,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_guard: Option<tool_guard::Settings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_response: Option<aw_host_process::Config>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_response: Option<aw_host_process::Config>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_response: Option<tool_response::Settings>,
    required_safety: bool,
}

impl Config {
    fn cwd(&self) -> Result<&std::path::Path, Error> {
        match self.format {
            1 => self.handler.as_ref().map(|h| h.cwd.as_path()),
            2 => self.cwd.as_deref(),
            _ => None,
        }
        .ok_or(Error::Profile("invalid configuration revision"))
    }

    fn legacy_handler(&self) -> Result<&aw_host_process::Config, Error> {
        self.handler
            .as_ref()
            .ok_or(Error::Profile("legacy observer is not configured"))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeExecutable {
    program: PathBuf,
    program_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    config: Config,
    revision: String,
    owner_pid: u32,
    owner_ticks: u64,
    helper: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    prepared: Prepared,
    runtime_id: String,
    agent_pid: u32,
    agent_ticks: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    #[serde(default)]
    activity: activity::Activity,
    session_id: Option<String>,
    #[serde(default)]
    session_start_observed: bool,
    attachment: u64,
    attached: bool,
    retired_sessions: Vec<String>,
    occurrences: u64,
    gap: bool,
    #[serde(default)]
    effect_gap_attachment: Option<u64>,
    #[serde(default)]
    notification_sequence: u64,
    #[serde(default)]
    event_counts: Option<std::collections::BTreeMap<aw_contracts::events::EventName, u64>>,
}

fn evidence<T>(_: T) -> Error {
    Error::Evidence
}
