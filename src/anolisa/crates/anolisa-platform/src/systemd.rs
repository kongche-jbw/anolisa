//! Systemd service management bridge.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SystemdError {
    #[error("systemctl command failed: {0}")]
    CommandFailed(String),
    #[error("service not found: {0}")]
    NotFound(String),
}

/// Query the status of a systemd unit.
pub fn unit_status(_unit: &str) -> Result<UnitStatus, SystemdError> {
    // TODO: invoke systemctl show and parse output
    Ok(UnitStatus {
        active: false,
        enabled: false,
        description: String::new(),
    })
}

#[derive(Debug)]
pub struct UnitStatus {
    pub active: bool,
    pub enabled: bool,
    pub description: String,
}

/// Enable and start a systemd unit.
pub fn enable_unit(_unit: &str) -> Result<(), SystemdError> {
    todo!("systemctl enable --now")
}

/// Stop and disable a systemd unit.
pub fn disable_unit(_unit: &str) -> Result<(), SystemdError> {
    todo!("systemctl disable --now")
}
