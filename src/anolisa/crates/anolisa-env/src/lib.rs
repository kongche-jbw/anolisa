//! ANOLISA environment facts detection.
//!
//! [`EnvService::detect`] returns an [`EnvFacts`] snapshot describing the
//! host OS, arch, libc, kernel, distro family, BTF / `cap_bpf` availability,
//! container runtime, and current user identity. Every probe degrades
//! gracefully: detection never panics and unknown values fall back to
//! `None` or safe defaults.
//!
//! The legacy probe / cache / gate scaffolding that lived in this crate
//! during the skeleton phase is preserved on disk (see `cache.rs`,
//! `gate.rs`, `probes/`) but is no longer wired into the crate while we
//! consolidate around this simpler `EnvFacts` contract — later milestones
//! will re-integrate it on top of the new shape.

use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Snapshot of detected environment facts.
///
/// Optional fields are `None` when detection is unavailable on the
/// current platform (for example `libc` / `btf` on non-Linux hosts) or
/// when a probe failed without a usable fallback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvFacts {
    /// Operating system identifier (e.g. `"linux"`, `"darwin"`).
    pub os: String,
    /// CPU architecture (e.g. `"x86_64"`, `"aarch64"`).
    pub arch: String,
    /// libc flavor on Linux (`"glibc"` / `"musl"`); `None` elsewhere.
    pub libc: Option<String>,
    /// Kernel release as reported by `uname -r`.
    pub kernel: Option<String>,
    /// Package base family derived from `/etc/os-release`
    /// (e.g. `"anolis23"`, `"anolis8"`).
    pub pkg_base: Option<String>,
    /// Whether `/sys/kernel/btf/vmlinux` exists.
    pub btf: Option<bool>,
    /// Best-effort `CAP_BPF` availability. `Some(true)` only when
    /// detection is confident (effective uid 0); otherwise `None`.
    pub cap_bpf: Option<bool>,
    /// Container runtime hint based on well-known marker files
    /// (`"docker"` / `"podman"`); `None` when not inside a container.
    pub container: Option<String>,
    /// User-facing login name (from `$USER` / `$LOGNAME`, or passwd).
    pub user: String,
    /// Effective uid of the running process.
    pub uid: u32,
    /// Home directory as resolved by [`dirs::home_dir`].
    pub home: PathBuf,
}

/// Stateless façade exposing environment detection entry points.
pub struct EnvService;

impl EnvService {
    /// Detect facts for the current host. Never fails.
    pub fn detect() -> EnvFacts {
        Self::detect_for(std::env::consts::OS)
    }

    /// Detection variant that pretends the target OS is `target_os`.
    ///
    /// Useful for tests that want to assert non-Linux fallback behavior
    /// without running on a non-Linux host. Probes that consult the live
    /// filesystem (`/proc`, `/sys`, `/etc/os-release`, etc.) still read
    /// from the real machine.
    pub fn detect_for(target_os: &str) -> EnvFacts {
        let arch = std::env::consts::ARCH.to_string();
        let libc = detect_libc(target_os);
        let kernel = detect_kernel();
        let pkg_base = detect_pkg_base();
        let btf = detect_btf(target_os);
        let cap_bpf = detect_cap_bpf();
        let container = detect_container();
        let (user, uid) = detect_user_uid();
        let home = detect_home();
        EnvFacts {
            os: target_os.to_string(),
            arch,
            libc,
            kernel,
            pkg_base,
            btf,
            cap_bpf,
            container,
            user,
            uid,
            home,
        }
    }
}

fn detect_libc(target_os: &str) -> Option<String> {
    if target_os != "linux" {
        return None;
    }
    if let Ok(out) = Command::new("ldd").arg("--version").output() {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        )
        .to_lowercase();
        if combined.contains("musl") {
            return Some("musl".to_string());
        }
        if combined.contains("glibc") || combined.contains("gnu libc") {
            return Some("glibc".to_string());
        }
    }
    // Default assumption on Linux when probes are inconclusive.
    Some("glibc".to_string())
}

fn detect_kernel() -> Option<String> {
    let out = Command::new("uname").arg("-r").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Parse an `os-release(5)` document and return a normalized
/// `pkg_base` identifier such as `"anolis23"` when the distro is
/// (or claims compatibility with) Anolis OS.
///
/// Returns `None` for distros outside the Anolis family — callers
/// can fall back to a different probe / default.
pub fn parse_os_release(content: &str) -> Option<String> {
    let mut id: Option<String> = None;
    let mut id_like: Option<String> = None;
    let mut version_id: Option<String> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if let Some(v) = line.strip_prefix("ID=") {
            id = Some(unquote(v));
        } else if let Some(v) = line.strip_prefix("ID_LIKE=") {
            id_like = Some(unquote(v));
        } else if let Some(v) = line.strip_prefix("VERSION_ID=") {
            version_id = Some(unquote(v));
        }
    }

    let is_anolis = id
        .as_deref()
        .map(|s| matches!(s, "anolis" | "openanolis"))
        .unwrap_or(false)
        || id_like
            .as_deref()
            .map(|s| {
                s.split_whitespace()
                    .any(|w| matches!(w, "anolis" | "openanolis"))
            })
            .unwrap_or(false);

    if !is_anolis {
        return None;
    }

    let major = version_id
        .as_deref()
        .and_then(|v| v.split('.').next())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    Some(match major {
        Some(m) => format!("anolis{m}"),
        None => "anolis".to_string(),
    })
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string()
}

fn detect_pkg_base() -> Option<String> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;
    parse_os_release(&content)
}

fn detect_btf(target_os: &str) -> Option<bool> {
    if target_os != "linux" {
        return None;
    }
    Some(std::path::Path::new("/sys/kernel/btf/vmlinux").exists())
}

fn detect_cap_bpf() -> Option<bool> {
    // Cheap heuristic: trust only when running as effective uid 0.
    // We avoid shelling out to `capsh` to keep detection panic-free
    // and side-effect-free.
    let euid = nix::unistd::Uid::effective().as_raw();
    if euid == 0 { Some(true) } else { None }
}

fn detect_container() -> Option<String> {
    if std::path::Path::new("/.dockerenv").exists() {
        return Some("docker".to_string());
    }
    if std::path::Path::new("/run/.containerenv").exists() {
        return Some("podman".to_string());
    }
    None
}

fn detect_user_uid() -> (String, u32) {
    let uid = nix::unistd::Uid::effective().as_raw();
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()
        .or_else(|| {
            nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
                .ok()
                .flatten()
                .map(|u| u.name)
        })
        .unwrap_or_else(|| "unknown".to_string());
    (user, uid)
}

fn detect_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_non_empty_os_and_arch() {
        let facts = EnvService::detect();
        assert!(!facts.os.is_empty(), "os should not be empty");
        assert!(!facts.arch.is_empty(), "arch should not be empty");
    }

    #[test]
    fn detect_for_non_linux_skips_libc_and_btf() {
        let facts = EnvService::detect_for("macos");
        assert_eq!(facts.os, "macos");
        assert!(facts.libc.is_none(), "libc should be None off-Linux");
        assert!(facts.btf.is_none(), "btf should be None off-Linux");
    }

    #[test]
    fn parse_os_release_anolis_23() {
        let content = "NAME=\"Anolis OS\"\n\
                       VERSION=\"23.0\"\n\
                       ID=\"anolis\"\n\
                       VERSION_ID=\"23\"\n";
        assert_eq!(parse_os_release(content).as_deref(), Some("anolis23"));
    }

    #[test]
    fn parse_os_release_id_like_matches() {
        let content = "NAME=\"Custom Distro\"\n\
                       ID=customdistro\n\
                       ID_LIKE=\"anolis rhel\"\n\
                       VERSION_ID=\"8.6\"\n";
        assert_eq!(parse_os_release(content).as_deref(), Some("anolis8"));
    }

    #[test]
    fn parse_os_release_unknown_distro_returns_none() {
        let content = "NAME=\"Ubuntu\"\nID=ubuntu\nVERSION_ID=\"22.04\"\n";
        assert!(parse_os_release(content).is_none());
    }

    #[test]
    fn parse_os_release_missing_version_falls_back_to_anolis() {
        let content = "ID=anolis\n";
        assert_eq!(parse_os_release(content).as_deref(), Some("anolis"));
    }
}
