//! Package manager abstraction (dnf/apt/zypper).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PkgError {
    #[error("package manager command failed: {0}")]
    CommandFailed(String),
    #[error("unsupported package base")]
    Unsupported,
}

/// Abstraction over system package managers.
pub trait PackageManager {
    fn install(&self, packages: &[&str]) -> Result<(), PkgError>;
    fn remove(&self, packages: &[&str]) -> Result<(), PkgError>;
    fn is_installed(&self, package: &str) -> bool;
}

/// Placeholder implementations — to be filled when running on Linux.
pub struct DnfBackend;
pub struct AptBackend;

impl PackageManager for DnfBackend {
    fn install(&self, _packages: &[&str]) -> Result<(), PkgError> {
        todo!("dnf install")
    }
    fn remove(&self, _packages: &[&str]) -> Result<(), PkgError> {
        todo!("dnf remove")
    }
    fn is_installed(&self, _package: &str) -> bool {
        todo!("rpm -q")
    }
}

impl PackageManager for AptBackend {
    fn install(&self, _packages: &[&str]) -> Result<(), PkgError> {
        todo!("apt install")
    }
    fn remove(&self, _packages: &[&str]) -> Result<(), PkgError> {
        todo!("apt remove")
    }
    fn is_installed(&self, _package: &str) -> bool {
        todo!("dpkg -l")
    }
}
