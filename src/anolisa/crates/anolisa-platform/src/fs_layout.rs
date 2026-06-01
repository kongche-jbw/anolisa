//! Filesystem layout resolution for user-mode vs system-mode installs.
//!
//! `system-mode` strictly follows FHS 3.0 (binaries under `/usr/bin`,
//! state under `/var/lib/anolisa`, etc.); `user-mode` strictly follows
//! the XDG Base Directory Specification (`$XDG_DATA_HOME/anolisa` and
//! friends). A custom prefix (typically `/opt/<name>`) may be supplied
//! in system-mode to relocate the whole tree.
//!
//! The [`InstallMode`] enum here mirrors `anolisa_cli::context::InstallMode`
//! but lives in this crate to keep `anolisa-platform` independent of the
//! CLI; the CLI layer converts between the two at the boundary.

use std::path::{Path, PathBuf};

/// Where ANOLISA installs files: user-mode (XDG under `$HOME`) or
/// system-mode (FHS under `/`, redirectable via a prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    System,
    User,
}

/// Resolved filesystem paths for a given install mode.
///
/// All fields are absolute. `lock_file` and `central_log` are
/// individual file paths; everything else is a directory.
#[derive(Debug, Clone)]
pub struct FsLayout {
    pub mode: InstallMode,
    pub prefix: PathBuf,
    pub bin_dir: PathBuf,
    pub lib_dir: PathBuf,
    pub libexec_dir: PathBuf,
    /// Configuration root.
    pub etc_dir: PathBuf,
    /// State root — `installed-state.toml` lives here.
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
    pub backup_dir: PathBuf,
    pub lock_file: PathBuf,
    /// Central JSONL log file consumed by `anolisa logs`.
    pub central_log: PathBuf,
    /// Catalog overlay directory for this install mode.
    pub manifests_overlay: PathBuf,
}

impl FsLayout {
    /// System (FHS) install layout. `prefix` defaults to `/`.
    ///
    /// When a non-`/` prefix is supplied, every default absolute path
    /// below is rebased under it (so `prefix=/opt/x` yields
    /// `/opt/x/usr/bin`, `/opt/x/etc/anolisa`, etc.).
    pub fn system(prefix: Option<PathBuf>) -> Self {
        let prefix = prefix.unwrap_or_else(|| PathBuf::from("/"));
        let rebase = |p: &str| rebase_under(&prefix, p);

        Self {
            mode: InstallMode::System,
            bin_dir: rebase("/usr/bin"),
            lib_dir: rebase("/usr/lib/anolisa"),
            libexec_dir: rebase("/usr/libexec/anolisa"),
            etc_dir: rebase("/etc/anolisa"),
            state_dir: rebase("/var/lib/anolisa"),
            cache_dir: rebase("/var/cache/anolisa"),
            log_dir: rebase("/var/log/anolisa"),
            backup_dir: rebase("/var/lib/anolisa/backups"),
            lock_file: rebase("/run/anolisa/install.lock"),
            central_log: rebase("/var/log/anolisa/central.jsonl"),
            manifests_overlay: rebase("/etc/anolisa/manifests"),
            prefix,
        }
    }

    /// User (XDG) install layout under `home`. XDG environment
    /// variables, when set, take precedence over the `$HOME`-based
    /// defaults.
    pub fn user(home: PathBuf) -> Self {
        let xdg_data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
        let xdg_config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let xdg_cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from);
        Self::user_with_xdg(home, xdg_data, xdg_config, xdg_cache)
    }

    /// Test-friendly variant of [`Self::user`] that takes the XDG
    /// directories explicitly instead of reading them from the
    /// process environment.
    pub(crate) fn user_with_xdg(
        home: PathBuf,
        xdg_data: Option<PathBuf>,
        xdg_config: Option<PathBuf>,
        xdg_cache: Option<PathBuf>,
    ) -> Self {
        let data = xdg_data.unwrap_or_else(|| home.join(".local/share"));
        let config = xdg_config.unwrap_or_else(|| home.join(".config"));
        let cache = xdg_cache.unwrap_or_else(|| home.join(".cache"));

        let prefix = data.join("anolisa");
        let bin_dir = prefix.join("bin");
        let lib_dir = prefix.join("lib");
        let libexec_dir = prefix.join("libexec");
        let state_dir = prefix.join("state");
        let backup_dir = prefix.join("backups");
        let lock_file = prefix.join("install.lock");

        let etc_dir = config.join("anolisa");
        let manifests_overlay = etc_dir.join("manifests");

        let cache_dir = cache.join("anolisa");
        let log_dir = cache_dir.join("log");
        let central_log = log_dir.join("central.jsonl");

        Self {
            mode: InstallMode::User,
            prefix,
            bin_dir,
            lib_dir,
            libexec_dir,
            etc_dir,
            state_dir,
            cache_dir,
            log_dir,
            backup_dir,
            lock_file,
            central_log,
            manifests_overlay,
        }
    }
}

/// Join `path` under `prefix`, stripping the leading `/` so that
/// `Path::join` does not discard the prefix.
fn rebase_under(prefix: &Path, path: &str) -> PathBuf {
    if prefix == Path::new("/") {
        return PathBuf::from(path);
    }
    let stripped = path.strip_prefix('/').unwrap_or(path);
    prefix.join(stripped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_default_uses_fhs_root() {
        let layout = FsLayout::system(None);
        assert_eq!(layout.mode, InstallMode::System);
        assert_eq!(layout.prefix, PathBuf::from("/"));
        assert_eq!(layout.bin_dir, PathBuf::from("/usr/bin"));
        assert_eq!(layout.lib_dir, PathBuf::from("/usr/lib/anolisa"));
        assert_eq!(layout.libexec_dir, PathBuf::from("/usr/libexec/anolisa"));
        assert_eq!(layout.etc_dir, PathBuf::from("/etc/anolisa"));
        assert_eq!(layout.state_dir, PathBuf::from("/var/lib/anolisa"));
        assert_eq!(layout.cache_dir, PathBuf::from("/var/cache/anolisa"));
        assert_eq!(layout.log_dir, PathBuf::from("/var/log/anolisa"));
        assert_eq!(layout.backup_dir, PathBuf::from("/var/lib/anolisa/backups"));
        assert_eq!(layout.lock_file, PathBuf::from("/run/anolisa/install.lock"));
        assert_eq!(
            layout.central_log,
            PathBuf::from("/var/log/anolisa/central.jsonl")
        );
        assert_eq!(
            layout.manifests_overlay,
            PathBuf::from("/etc/anolisa/manifests")
        );
    }

    #[test]
    fn system_custom_prefix_rebases_paths() {
        let layout = FsLayout::system(Some(PathBuf::from("/opt/x")));
        assert_eq!(layout.bin_dir, PathBuf::from("/opt/x/usr/bin"));
        assert_eq!(layout.etc_dir, PathBuf::from("/opt/x/etc/anolisa"));
        assert_eq!(
            layout.lock_file,
            PathBuf::from("/opt/x/run/anolisa/install.lock")
        );
        assert_eq!(
            layout.central_log,
            PathBuf::from("/opt/x/var/log/anolisa/central.jsonl")
        );
    }

    #[test]
    fn user_layout_under_home_with_no_xdg() {
        // Use the env-free helper so parallel tests in other crates
        // can't race us by mutating XDG_* in their own processes.
        let layout = FsLayout::user_with_xdg(PathBuf::from("/tmp/h"), None, None, None);
        assert_eq!(layout.mode, InstallMode::User);
        assert_eq!(layout.prefix, PathBuf::from("/tmp/h/.local/share/anolisa"));
        assert_eq!(
            layout.bin_dir,
            PathBuf::from("/tmp/h/.local/share/anolisa/bin")
        );
        assert_eq!(
            layout.libexec_dir,
            PathBuf::from("/tmp/h/.local/share/anolisa/libexec")
        );
        assert_eq!(layout.etc_dir, PathBuf::from("/tmp/h/.config/anolisa"));
        assert_eq!(layout.cache_dir, PathBuf::from("/tmp/h/.cache/anolisa"));
        assert_eq!(layout.log_dir, PathBuf::from("/tmp/h/.cache/anolisa/log"));
        assert_eq!(
            layout.central_log,
            PathBuf::from("/tmp/h/.cache/anolisa/log/central.jsonl")
        );
        assert_eq!(
            layout.manifests_overlay,
            PathBuf::from("/tmp/h/.config/anolisa/manifests")
        );
    }

    #[test]
    fn user_layout_honors_explicit_xdg_dirs() {
        let layout = FsLayout::user_with_xdg(
            PathBuf::from("/tmp/h"),
            Some(PathBuf::from("/data")),
            Some(PathBuf::from("/conf")),
            Some(PathBuf::from("/cache")),
        );
        assert_eq!(layout.prefix, PathBuf::from("/data/anolisa"));
        assert_eq!(layout.etc_dir, PathBuf::from("/conf/anolisa"));
        assert_eq!(layout.cache_dir, PathBuf::from("/cache/anolisa"));
    }
}
