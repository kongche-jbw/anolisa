//! Bounded scan-code execution using the existing native process owner.

use crate::Config;
use aw_core::ports::{Cancellation, HostError};
use aw_sec_core::code::CodeRequest;
use std::time::Instant;

/// Checks a Bash command without executing it or granting native permission.
///
/// # Errors
/// Changed pins, version mismatch, failed processes or invalid scanner output
/// reject the check. The caller maps failure to its native deny response.
pub fn check(
    config: &Config,
    command: &str,
    remaining_ms: u64,
    cancellation: &dyn Cancellation,
) -> Result<bool, HostError> {
    let started = Instant::now();
    let budget = remaining_ms.min(config.limits.timeout_ms);
    let error = || HostError {
        code: "native_code_check_failed".into(),
    };
    config.validate().map_err(|_| error())?;
    config.check_pins().map_err(|_| error())?;
    let request = CodeRequest::new(command).map_err(|_| error())?;
    if command.len() > config.limits.input_bytes {
        return Err(error());
    }
    let remaining = || {
        budget
            .checked_sub(started.elapsed().as_millis() as u64)
            .filter(|ms| *ms > 0)
            .ok_or_else(error)
    };
    let version = aw_host_process::run(config, &["--version"], &[], remaining()?, cancellation)
        .map_err(|_| error())?;
    if version.exit_code != 0
        || version.stdout != format!("agent-sec-cli {}\n", crate::NATIVE_CLI_VERSION).as_bytes()
    {
        return Err(error());
    }
    config.check_pins().map_err(|_| error())?;
    let output = aw_host_process::run(config, &request.args(), &[], remaining()?, cancellation)
        .map_err(|_| error())?;
    config.check_pins().map_err(|_| error())?;
    remaining()?;
    if cancellation.is_cancelled() {
        return Err(error());
    }
    request
        .allows(output.exit_code, &output.stdout)
        .map_err(|_| error())
}
