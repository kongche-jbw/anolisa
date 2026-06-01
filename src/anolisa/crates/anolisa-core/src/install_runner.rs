//! Install runner: copy a cached artifact into the ANOLISA-owned layout.
//!
//! This milestone only supports two backends:
//!   * `binary`  — the cached file IS the installed binary (one file in,
//!                 one file out). Manifest must declare exactly one dest.
//!   * `tar_gz`  — extract a gzipped tar archive, then copy each entry
//!                 whose basename matches a manifest dest into that dest.
//!
//! All destinations must resolve under one of the ANOLISA-owned roots
//! (`bin_dir`, `etc_dir`, `state_dir`, `lib_dir`, `libexec_dir`, `datadir`,
//! `log_dir`, `cache_dir`). Anything else is rejected as
//! `InstallError::ExternalPath`. The runner refuses to modify or even
//! create files outside those roots, so a failed install can roll back by
//! deleting just the paths it returns.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anolisa_platform::fs_layout::FsLayout;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use tar::Archive;

/// One destination file written by the runner, with the sha256 of the
/// installed bytes. Sub-C records these in `InstalledState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledFile {
    /// Absolute destination path actually written.
    pub path: PathBuf,
    /// Lowercase-hex sha256 of the installed bytes.
    pub sha256: String,
}

/// Aggregate result of a single [`InstallRunner::install`] call.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// One entry per destination written, in `resolved_dests` order.
    pub files: Vec<InstalledFile>,
}

/// Failure modes for [`InstallRunner::install`].
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("artifact_type '{0}' is not supported by this milestone (only 'binary' and 'tar_gz')")]
    UnsupportedArtifactType(String),

    #[error("manifest must declare at least one destination file")]
    NoDestinations,

    #[error("'binary' install requires exactly one manifest dest, got {0}")]
    BinaryRequiresSingleDest(usize),

    #[error("destination '{path}' is not under an ANOLISA-owned root")]
    ExternalPath { path: PathBuf },

    #[error(
        "destination '{path}' resolved to an unrendered template — manifest variable not substituted"
    )]
    UnresolvedTemplate { path: PathBuf },

    #[error("tar_gz archive entry for dest basename '{basename}' not found")]
    MissingArchiveEntry { basename: String },

    #[error("io error while accessing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("archive read error: {0}")]
    Archive(String),
}

/// Stateless installer bound to an [`FsLayout`] for ANOLISA-owned-root
/// validation. Construct one per `enable` invocation.
pub struct InstallRunner<'a> {
    layout: &'a FsLayout,
}

impl<'a> InstallRunner<'a> {
    /// Build a runner over `layout` — used only to validate that every
    /// destination resolves under an ANOLISA-owned root.
    pub fn new(layout: &'a FsLayout) -> Self {
        Self { layout }
    }

    /// Install `cached_artifact` to the destinations in `resolved_dests`,
    /// which must be absolute paths already substituted against the layout
    /// (Sub-C will pass the planner's `ComponentPlan.resolved_files`).
    ///
    /// `artifact_type` is the wire string from the EnablePlan (e.g. "binary",
    /// "tar_gz").
    ///
    /// On success returns one `InstalledFile` per written path with the
    /// final sha256 — Sub-C will copy these into `InstalledState.objects[].files`.
    pub fn install(
        &self,
        artifact_type: &str,
        cached_artifact: &Path,
        resolved_dests: &[PathBuf],
    ) -> Result<InstallOutcome, InstallError> {
        if resolved_dests.is_empty() {
            return Err(InstallError::NoDestinations);
        }
        for dest in resolved_dests {
            self.validate_dest(dest)?;
        }

        match artifact_type {
            "binary" => self.install_binary(cached_artifact, resolved_dests),
            "tar_gz" => self.install_tar_gz(cached_artifact, resolved_dests),
            other => Err(InstallError::UnsupportedArtifactType(other.to_string())),
        }
    }

    fn install_binary(
        &self,
        cached_artifact: &Path,
        resolved_dests: &[PathBuf],
    ) -> Result<InstallOutcome, InstallError> {
        if resolved_dests.len() != 1 {
            return Err(InstallError::BinaryRequiresSingleDest(resolved_dests.len()));
        }
        let dest = &resolved_dests[0];
        let bytes = read_file_bytes(cached_artifact)?;
        let installed = write_dest_atomic(dest, &bytes)?;
        Ok(InstallOutcome {
            files: vec![installed],
        })
    }

    fn install_tar_gz(
        &self,
        cached_artifact: &Path,
        resolved_dests: &[PathBuf],
    ) -> Result<InstallOutcome, InstallError> {
        let entries = read_tar_gz_basenames(cached_artifact)?;

        let mut out = Vec::with_capacity(resolved_dests.len());
        for dest in resolved_dests {
            let basename = dest
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| InstallError::ExternalPath { path: dest.clone() })?
                .to_string();
            let bytes = entries
                .get(&basename)
                .ok_or(InstallError::MissingArchiveEntry { basename })?;
            let installed = write_dest_atomic(dest, bytes)?;
            out.push(installed);
        }
        Ok(InstallOutcome { files: out })
    }

    fn validate_dest(&self, dest: &Path) -> Result<(), InstallError> {
        if dest.to_string_lossy().contains('{') {
            return Err(InstallError::UnresolvedTemplate {
                path: dest.to_path_buf(),
            });
        }
        let roots: Vec<&Path> = vec![
            self.layout.bin_dir.as_path(),
            self.layout.etc_dir.as_path(),
            self.layout.state_dir.as_path(),
            self.layout.lib_dir.as_path(),
            self.layout.libexec_dir.as_path(),
            self.layout.datadir.as_path(),
            self.layout.log_dir.as_path(),
            self.layout.cache_dir.as_path(),
        ];
        if roots.iter().any(|root| dest.starts_with(root)) {
            Ok(())
        } else {
            Err(InstallError::ExternalPath {
                path: dest.to_path_buf(),
            })
        }
    }
}

fn read_file_bytes(path: &Path) -> Result<Vec<u8>, InstallError> {
    fs::read(path).map_err(|source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Last-write-wins on basename collisions; sufficient for this milestone
/// since manifests address payload files by basename only.
fn read_tar_gz_basenames(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, InstallError> {
    let file = File::open(path).map_err(|source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = Archive::new(GzDecoder::new(file));
    let mut out: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let entries = archive
        .entries()
        .map_err(|e| InstallError::Archive(format!("entries: {e}")))?;
    for entry_res in entries {
        let mut entry = entry_res.map_err(|e| InstallError::Archive(format!("entry: {e}")))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let entry_path = entry
            .path()
            .map_err(|e| InstallError::Archive(format!("path: {e}")))?
            .into_owned();
        let Some(basename) = entry_path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let basename = basename.to_string();
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| InstallError::Archive(format!("read entry '{basename}': {e}")))?;
        out.insert(basename, buf);
    }
    Ok(out)
}

fn write_dest_atomic(dest: &Path, bytes: &[u8]) -> Result<InstalledFile, InstallError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|source| InstallError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let tmp = tmp_sibling(dest);
    let sha = match stream_write_and_hash(&tmp, bytes) {
        Ok(h) => h,
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            return Err(err);
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        fs::set_permissions(&tmp, perms).map_err(|source| InstallError::Io {
            path: tmp.clone(),
            source,
        })?;
    }
    fs::rename(&tmp, dest).map_err(|source| {
        let _ = fs::remove_file(&tmp);
        InstallError::Io {
            path: dest.to_path_buf(),
            source,
        }
    })?;
    Ok(InstalledFile {
        path: dest.to_path_buf(),
        sha256: sha,
    })
}

fn stream_write_and_hash(tmp: &Path, bytes: &[u8]) -> Result<String, InstallError> {
    let mut out = File::create(tmp).map_err(|source| InstallError::Io {
        path: tmp.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    for chunk in bytes.chunks(8 * 1024) {
        hasher.update(chunk);
        out.write_all(chunk).map_err(|source| InstallError::Io {
            path: tmp.to_path_buf(),
            source,
        })?;
    }
    out.flush().map_err(|source| InstallError::Io {
        path: tmp.to_path_buf(),
        source,
    })?;
    Ok(to_lower_hex(&hasher.finalize()))
}

fn tmp_sibling(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

fn to_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, Header};
    use tempfile::tempdir;

    fn layout_for(home: &Path) -> FsLayout {
        FsLayout::user(home.to_path_buf())
    }

    fn write_cached(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }

    fn sha256_of(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        to_lower_hex(&h.finalize())
    }

    fn build_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let buf: Vec<u8> = Vec::new();
        let enc = GzEncoder::new(buf, Compression::default());
        let mut tar = Builder::new(enc);
        for (path, data) in entries {
            let mut hdr = Header::new_gnu();
            hdr.set_size(data.len() as u64);
            hdr.set_mode(0o644);
            hdr.set_cksum();
            tar.append_data(&mut hdr, path, *data).unwrap();
        }
        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn binary_install_single_dest_succeeds() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let payload = b"fake-binary-bytes";
        let cached = write_cached(cache.path(), "agentsight", payload);
        let dest = layout.bin_dir.join("agentsight");

        let outcome = runner
            .install("binary", &cached, &[dest.clone()])
            .expect("install ok");

        assert_eq!(outcome.files.len(), 1);
        assert_eq!(outcome.files[0].path, dest);
        assert_eq!(outcome.files[0].sha256, sha256_of(payload));
        assert!(dest.exists());
        let got = fs::read(&dest).unwrap();
        assert_eq!(got, payload);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
        }
    }

    #[test]
    fn binary_install_two_dests_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let d1 = layout.bin_dir.join("a");
        let d2 = layout.bin_dir.join("b");
        let err = runner
            .install("binary", &cached, &[d1, d2])
            .expect_err("must error");
        match err {
            InstallError::BinaryRequiresSingleDest(n) => assert_eq!(n, 2),
            other => panic!("expected BinaryRequiresSingleDest, got {other:?}"),
        }
    }

    #[test]
    fn binary_install_unresolved_template_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let dest = PathBuf::from("{bindir}/foo");
        let err = runner
            .install("binary", &cached, &[dest.clone()])
            .expect_err("must error");
        match err {
            InstallError::UnresolvedTemplate { path } => assert_eq!(path, dest),
            other => panic!("expected UnresolvedTemplate, got {other:?}"),
        }
    }

    #[test]
    fn binary_install_external_path_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let dest = PathBuf::from("/tmp/escape/foo");
        let err = runner
            .install("binary", &cached, &[dest.clone()])
            .expect_err("must error");
        match err {
            InstallError::ExternalPath { path } => assert_eq!(path, dest),
            other => panic!("expected ExternalPath, got {other:?}"),
        }
    }

    #[test]
    fn binary_install_creates_missing_parent_dirs() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"deep");

        let dest = layout.state_dir.join("sub").join("deep").join("file.bin");
        let outcome = runner
            .install("binary", &cached, &[dest.clone()])
            .expect("install ok");
        assert!(dest.exists());
        assert_eq!(outcome.files[0].sha256, sha256_of(b"deep"));
    }

    #[test]
    fn tar_gz_install_extracts_matching_basenames() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let bin_bytes: &[u8] = b"agentsight-binary";
        let data_bytes: &[u8] = b"data-file-contents";
        let gz = build_tar_gz(&[
            ("bin/agentsight", bin_bytes),
            ("share/data.toml", data_bytes),
        ]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let dest_bin = layout.bin_dir.join("agentsight");
        let dest_data = layout.datadir.join("data.toml");
        let outcome = runner
            .install("tar_gz", &cached, &[dest_bin.clone(), dest_data.clone()])
            .expect("install ok");

        assert_eq!(outcome.files.len(), 2);
        assert_eq!(fs::read(&dest_bin).unwrap(), bin_bytes);
        assert_eq!(fs::read(&dest_data).unwrap(), data_bytes);
        assert_eq!(outcome.files[0].sha256, sha256_of(bin_bytes));
        assert_eq!(outcome.files[1].sha256, sha256_of(data_bytes));
    }

    #[test]
    fn tar_gz_install_missing_entry_reports_basename() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let gz = build_tar_gz(&[("bin/something-else", b"x")]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let dest = layout.bin_dir.join("missing");
        let err = runner
            .install("tar_gz", &cached, &[dest])
            .expect_err("must error");
        match err {
            InstallError::MissingArchiveEntry { basename } => assert_eq!(basename, "missing"),
            other => panic!("expected MissingArchiveEntry, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_artifact_type_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let dest = layout.bin_dir.join("a");
        let err = runner
            .install("rpm", &cached, &[dest])
            .expect_err("must error");
        match err {
            InstallError::UnsupportedArtifactType(s) => assert_eq!(s, "rpm"),
            other => panic!("expected UnsupportedArtifactType, got {other:?}"),
        }
    }

    #[test]
    fn no_dests_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let err = runner
            .install("binary", &cached, &[])
            .expect_err("must error");
        assert!(matches!(err, InstallError::NoDestinations));
    }

    #[test]
    fn tar_gz_external_dest_rejected_before_extraction() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let gz = build_tar_gz(&[("bin/foo", b"foo-bytes")]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let dest = PathBuf::from("/tmp/escape/foo");
        let err = runner
            .install("tar_gz", &cached, &[dest])
            .expect_err("must error");
        assert!(matches!(err, InstallError::ExternalPath { .. }));
        let leaked = layout.bin_dir.join("foo");
        assert!(!leaked.exists(), "must not extract before validating dest");
    }
}
