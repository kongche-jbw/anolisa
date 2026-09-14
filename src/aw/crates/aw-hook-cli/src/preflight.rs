//! Native dependency admission before a launcher creates an Agent or hook state.

use aw_contracts::canonical;
use aw_core::ports::Cancellation;
use aw_sec_host::{Config, SecHost};
use aw_tokenless_host::TokenlessHost;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};

/// Explicit native dependencies; executable paths retain their installation context.
///
/// Each configuration is the same `Config` consumed by its execution Host. No
/// PATH search, version fallback or current-file digest enrollment is performed.
#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Settings {
    /// Inspection does not require Tokenless or a display process.
    Inspect {
        /// Trusted SecCore launch configuration.
        security: Config,
    },
    /// Projection depends on both supported native components.
    Project {
        /// Required SecCore launch configuration.
        security: Config,
        /// Tokenless launch configuration, including explicit native controls.
        tokenless: Box<Config>,
    },
}

/// Admission diagnostics omit configuration values and captured native output.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The private configuration file or dependency selection is invalid.
    #[error("invalid preflight settings; use inspect/security or project/security/tokenless")]
    Settings,
    /// The selected SecCore installation does not satisfy its execution Host.
    #[error("SecCore preflight failed (expected CLI {expected}): {source}", expected = aw_sec_host::NATIVE_CLI_VERSION)]
    Security {
        /// Static Host diagnostic; never contains captured process output.
        #[from]
        source: aw_sec_host::Error,
    },
    /// The selected Tokenless installation does not satisfy its execution Host.
    #[error("Tokenless preflight failed (expected CLI {expected}): {source}", expected = aw_tokenless_host::NATIVE_CLI_VERSION)]
    Tokenless {
        /// Static Host diagnostic; never contains captured process output.
        #[from]
        source: aw_tokenless_host::Error,
    },
    /// The caller stopped preparation before readiness could be reported.
    #[error("preflight cancelled")]
    Cancelled,
}

/// Reads bounded, caller-owned settings without resolving executable symlinks.
///
/// # Errors
/// Rejects non-private/non-regular files, duplicate JSON keys and unknown fields.
pub fn read_settings(path: &Path) -> Result<Settings, Error> {
    let bytes =
        crate::input::read_private(path, crate::MAX_INPUT_BYTES).map_err(|_| Error::Settings)?;
    let value = canonical::parse(&bytes).map_err(|_| Error::Settings)?;
    serde_json::from_value(value).map_err(|_| Error::Settings)
}

/// Probes the exact dependencies through their existing execution Hosts.
///
/// This launches native version probes and a SecCore synthetic scan, using the
/// declared environment, budgets and cancellation. It creates no Agent, hook,
/// Journal or adoption
/// record. SecCore may emit its normal native audit event for the public probe.
/// Success is a dependency snapshot, not runtime or security readiness;
/// execution still performs its own admission and identity checks.
///
/// # Errors
/// Returns the responsible Host's version/protocol/pin/process failure.
/// Inspection does not probe Tokenless; projection requires both and stops on
/// SecCore failure.
pub fn run(
    settings: Settings,
    cancellation: Arc<dyn Cancellation + Send + Sync>,
) -> Result<Value, Error> {
    if cancellation.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let (security, tokenless) = match settings {
        Settings::Inspect { security } => (security, None),
        Settings::Project {
            security,
            tokenless,
        } => {
            if security.provider_id == tokenless.provider_id {
                return Err(Error::Settings);
            }
            (security, Some(tokenless))
        }
    };
    SecHost::new_cancellable(security, cancellation.clone())?.probe_protocol()?;
    let mut providers = vec![json!({
        "component":"sec-core", "native_version":aw_sec_host::NATIVE_CLI_VERSION,
        "protocol_profile":aw_sec_host::PROTOCOL_PROFILE, "protocol_probe":"passed",
        "probe_scope":"synthetic_content", "native_audit_possible":true
    })];
    let mode = if let Some(config) = tokenless {
        TokenlessHost::new_cancellable(*config, cancellation.clone())?;
        providers.push(json!({
            "component":"tokenless", "native_version":aw_tokenless_host::NATIVE_CLI_VERSION
        }));
        "project"
    } else {
        "inspect"
    };
    if cancellation.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(
        json!({"status":"dependencies_ready", "mode":mode, "providers":providers,
        "agent_started":false, "runtime_ready":false}),
    )
}
