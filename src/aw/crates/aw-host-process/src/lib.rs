//! Native launch provenance and bounded process ownership shared by AW Hosts.
//! Provider descriptors, protocol mapping and receipts remain with each Host.

mod config;
mod process;

pub use config::{Config, FilePin, Limits, PinState, MAX_STREAM_BYTES};
pub use process::{run, Output};

/// Bounded diagnostics that never contain native output or configuration content.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Launch configuration or selected file pins failed validation.
    #[error("invalid native Host configuration: {0}")]
    Configuration(&'static str),
    /// Native exchange, cancellation or owned process cleanup failed.
    #[error("native Host process failed: {0}")]
    Process(&'static str),
}

impl Error {
    /// Static diagnostic for a Host-authored error category or failed receipt.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Configuration(code) | Self::Process(code) => code,
        }
    }
}
