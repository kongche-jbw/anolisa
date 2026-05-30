//! Filesystem layout resolution for user-mode vs system-mode installs.

use std::path::PathBuf;

/// Install mode determines where files go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    User,
    System,
}

/// Resolved filesystem paths for a given install mode.
#[derive(Debug, Clone)]
pub struct FsLayout {
    pub mode: InstallMode,
    pub prefix: PathBuf,
    pub bin_dir: PathBuf,
    pub libexec_dir: PathBuf,
    pub share_dir: PathBuf,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl FsLayout {
    /// Resolve paths for the given mode, optionally with a custom prefix.
    pub fn resolve(mode: InstallMode, custom_prefix: Option<PathBuf>) -> Self {
        match mode {
            InstallMode::User => {
                let prefix = custom_prefix.unwrap_or_else(|| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("/tmp"))
                        .join(".local")
                });
                Self {
                    mode,
                    bin_dir: prefix.join("bin"),
                    libexec_dir: prefix.join("libexec/anolisa"),
                    share_dir: prefix.join("share/anolisa"),
                    config_dir: dirs::config_dir()
                        .unwrap_or_else(|| prefix.join("../config"))
                        .join("anolisa"),
                    state_dir: dirs::state_dir()
                        .unwrap_or_else(|| prefix.join("state"))
                        .join("anolisa"),
                    cache_dir: dirs::cache_dir()
                        .unwrap_or_else(|| prefix.join("cache"))
                        .join("anolisa"),
                    prefix,
                }
            }
            InstallMode::System => {
                let prefix = custom_prefix.unwrap_or_else(|| PathBuf::from("/usr/local"));
                Self {
                    mode,
                    bin_dir: prefix.join("bin"),
                    libexec_dir: PathBuf::from("/usr/libexec/anolisa"),
                    share_dir: PathBuf::from("/usr/share/anolisa"),
                    config_dir: PathBuf::from("/etc/anolisa"),
                    state_dir: PathBuf::from("/var/lib/anolisa"),
                    cache_dir: PathBuf::from("/var/cache/anolisa"),
                    prefix,
                }
            }
        }
    }
}
