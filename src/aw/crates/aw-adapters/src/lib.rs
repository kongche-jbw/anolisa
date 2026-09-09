#![forbid(unsafe_code)]
//! Native boundary capture and shared Core invocation without plugin side effects.
//!
//! The embedding plugin owns registration, policy and native result formatting.
//! Profiles describe this adapter's implemented powers, not all host features.

pub mod native;
pub mod profiles;

mod bridge;
mod capture;

pub use bridge::{AdaptedExecution, PreparedEvent, StepOptions};
pub use capture::{CaptureRequest, CapturedEvent, NativeContext};

use aw_contracts::Registry;
use aw_core::Core;

/// Native host whose concrete event vocabulary is preserved by the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// Qoder CLI's subprocess hooks.
    Qoder,
    /// Codex subprocess hooks, without result replacement in this profile.
    Codex,
    /// Qwen Code's extension hooks.
    QwenCode,
    /// Hermes native callbacks, supplied through a local argument container.
    Hermes,
    /// OpenClaw plugin callbacks, supplied through a local argument container.
    OpenClaw,
    /// COSH's own loop, not the internal loop of an external child Agent.
    Cosh,
}

impl Host {
    /// Stable adapter-local host identifier used in the six bundled profiles.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Qoder => "qoder",
            Self::Codex => "codex",
            Self::QwenCode => "qwencode",
            Self::Hermes => "hermes",
            Self::OpenClaw => "openclaw",
            Self::Cosh => "cosh",
        }
    }
}

/// Adapter admission errors never imply that a native tool was blocked.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A bundled descriptor contradicts this implemented adapter profile.
    #[error("invalid native profile: {0}")]
    InvalidProfile(&'static str),
    /// The native event has no supported mapping for this host.
    #[error("unsupported native event")]
    UnsupportedEvent,
    /// The native shape is ambiguous or outside the supported text extraction.
    #[error("unsupported native payload: {0}")]
    UnsupportedPayload(&'static str),
    /// Observed native IDs or plan identity differ from trusted runtime context.
    #[error("native identity mismatch: {0}")]
    IdentityMismatch(&'static str),
    /// A capability needs a boundary power not supplied by this adapter.
    #[error("capability is unsupported at this native boundary")]
    UnsupportedCapability,
    /// Existing AW schemas or semantic invariants rejected a supplied value.
    #[error(transparent)]
    Contract(#[from] aw_contracts::Error),
    /// Core preparation, Host invocation or journal acknowledgement failed.
    #[error(transparent)]
    Core(#[from] aw_core::Error),
}

/// Reusable adapter with one host profile and the real Core implementation.
pub struct Adapter {
    host: Host,
    registry: Registry,
    core: Core,
}

impl Adapter {
    /// Loads the exact bundled host profile and offline contract registries.
    ///
    /// # Errors
    /// Rejects invalid embedded profiles or an unusable schema bundle.
    pub fn new(host: Host) -> Result<Self, Error> {
        profiles::profile(host)?;
        Ok(Self {
            host,
            registry: Registry::new()?,
            core: Core::new()?,
        })
    }
}
