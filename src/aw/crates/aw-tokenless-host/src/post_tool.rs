//! Native post-tool exchange without requiring an AW Turn or capability plan.

use crate::{host_error, protocol::ProjectionRequest, Config, Error, NATIVE_CLI_VERSION};
use aw_contracts::Registry;
use aw_core::ports::{Cancellation, HostError};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Checks the explicit native controls without launching or reading executable pins.
///
/// # Errors
/// Rejects invalid launch bounds or controls that permit native telemetry.
pub fn validate_config(config: &Config) -> Result<(), Error> {
    config
        .validate()
        .map_err(|e| Error::Configuration(e.code()))?;
    for (key, value) in [
        ("TOKENLESS_STATS_ENABLED", "0"),
        ("TOKENLESS_SLS_ENABLED", "0"),
        ("TOKENLESS_COMPRESSION_ENABLED", "1"),
    ] {
        if config.environment.get(key).map(String::as_str) != Some(value) {
            return Err(Error::Configuration(
                "explicit native controls are required",
            ));
        }
    }
    Ok(())
}

/// Projects an authenticated, successfully completed command-output artifact.
///
/// The caller establishes native success and authenticates `actor_id`, `session_id`
/// and `tool_use_id` in `scope`. No Turn, running state or adoption is inferred.
/// The input follows context-projection-prepare/v2; `None` means native preserve.
/// Version admission, pins, encoding and compression share the supplied time budget.
///
/// # Errors
/// Rejects unsupported recovery, source digests, native attribution, malformed
/// responses, changed pins, cancellation and expired budgets without content.
pub fn project(
    config: &Config,
    input: &Value,
    scope: &Value,
    remaining_ms: u64,
    cancellation: &dyn Cancellation,
) -> Result<Option<Value>, HostError> {
    let deadline =
        Instant::now() + Duration::from_millis(remaining_ms.min(config.limits.timeout_ms));
    let remaining = || {
        let millis = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u64;
        if cancellation.is_cancelled() || millis == 0 {
            Err(host_error("provider_cancelled_or_expired"))
        } else {
            Ok(millis)
        }
    };
    validate_config(config).map_err(|_| host_error("invalid_tokenless_configuration"))?;
    let registry = Registry::new().map_err(|_| host_error("invalid_registry"))?;
    let request = ProjectionRequest::new(&registry, input, scope).map_err(host_error)?;
    if request.stdin().len() > config.limits.input_bytes {
        return Err(host_error("input_budget_exceeded"));
    }
    config.check_pins().map_err(|e| host_error(e.code()))?;
    let version = aw_host_process::run(config, &["--version"], &[], remaining()?, cancellation)
        .map_err(|e| host_error(e.code()))?;
    config.check_pins().map_err(|e| host_error(e.code()))?;
    if version.exit_code != 0
        || version.stdout != format!("tokenless {NATIVE_CLI_VERSION}\n").as_bytes()
    {
        return Err(host_error("unsupported_native_version"));
    }
    let output = aw_host_process::run(
        config,
        &request.args(),
        request.stdin(),
        remaining()?,
        cancellation,
    )
    .map_err(|e| host_error(e.code()))?;
    config.check_pins().map_err(|e| host_error(e.code()))?;
    let candidate = request
        .project(output.exit_code, &output.stdout, &registry)
        .map_err(host_error)?;
    remaining()?;
    Ok(candidate)
}
