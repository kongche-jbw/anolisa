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
use std::fs::{self, File, OpenOptions};
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
        "destination '{path}' contains a '.' or '..' segment — refuse to install via traversal"
    )]
    TraversalSegment { path: PathBuf },

    #[error(
        "destination '{path}' already exists — P1-F refuses to overwrite (backup/rollback lands in P1-G)"
    )]
    DestExists { path: PathBuf },

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
        // Fresh-install only for P1-F: refuse to overwrite anything already
        // on disk. Backup/restore of pre-existing ANOLISA-owned files lands
        // in P1-G; until then, the runner must never silently clobber.
        // Check all dests up front so a partial run can't leave half-written
        // siblings behind. Use `symlink_metadata` rather than `exists()` so
        // a broken symlink (target missing, `exists()` returns false) is
        // still caught and refused.
        for dest in resolved_dests {
            match fs::symlink_metadata(dest) {
                Ok(_) => {
                    return Err(InstallError::DestExists {
                        path: dest.to_path_buf(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(InstallError::Io {
                        path: dest.to_path_buf(),
                        source,
                    });
                }
            }
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
        // Lexical reject of traversal segments. Defeats render_files outputs
        // that look like `<bin_dir>/../<escape>` — without this check the
        // `starts_with(root)` below would pass, and `create_dir_all` /
        // `rename` would happily write outside the root.
        for component in dest.components() {
            if matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            ) {
                return Err(InstallError::TraversalSegment {
                    path: dest.to_path_buf(),
                });
            }
        }
        let lex_roots = self.lexical_roots();
        if !lex_roots.iter().any(|root| dest.starts_with(root)) {
            return Err(InstallError::ExternalPath {
                path: dest.to_path_buf(),
            });
        }
        // Canonicalize the deepest existing ancestor of `dest` and ensure
        // it still lives under a canonicalized root. Defeats symlink-in-
        // ancestor escapes (e.g. someone planted a symlink inside bin_dir
        // pointing at /etc). When neither the dest nor any ancestor of the
        // root exists yet, canonicalize_nearest_existing returns None and
        // we fall back to the lexical check above — acceptable for P1-F
        // since a fresh layout's roots are themselves under a tmp prefix
        // we control.
        if let Some(canonical_dest) = canonicalize_nearest_existing(dest) {
            let canonical_roots: Vec<PathBuf> = lex_roots
                .iter()
                .filter_map(|r| canonicalize_nearest_existing(r))
                .collect();
            if !canonical_roots.is_empty()
                && !canonical_roots
                    .iter()
                    .any(|r| canonical_dest.starts_with(r))
            {
                return Err(InstallError::ExternalPath {
                    path: dest.to_path_buf(),
                });
            }
        }
        Ok(())
    }

    fn lexical_roots(&self) -> Vec<&Path> {
        vec![
            self.layout.bin_dir.as_path(),
            self.layout.etc_dir.as_path(),
            self.layout.state_dir.as_path(),
            self.layout.lib_dir.as_path(),
            self.layout.libexec_dir.as_path(),
            self.layout.datadir.as_path(),
            self.layout.log_dir.as_path(),
            self.layout.cache_dir.as_path(),
        ]
    }
}

/// Walk up `p`'s ancestors until one exists, canonicalize that, and
/// re-attach the missing tail. Returns `None` only if not even `/` (or
/// the platform equivalent) can be canonicalized — effectively never on
/// the platforms this CLI targets.
fn canonicalize_nearest_existing(p: &Path) -> Option<PathBuf> {
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut current = p.to_path_buf();
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let mut out = canonical;
            for seg in suffix.iter().rev() {
                out.push(seg);
            }
            return Some(out);
        }
        let name = current.file_name()?.to_os_string();
        suffix.push(name);
        if !current.pop() {
            return None;
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
    // Security-critical: open the tmp sibling with O_CREAT|O_EXCL so a
    // pre-placed symlink (or any other existing entry) fails the open
    // with EEXIST/ELOOP instead of letting us write through it to a
    // path outside the ANOLISA-owned roots. On Unix we additionally pass
    // O_NOFOLLOW as belt-and-suspenders: even on a kernel that resolves
    // O_CREAT|O_EXCL race-y vs a concurrently-planted symlink, the final
    // component cannot be followed. `File::create` (the old code) did
    // NOT do either — it opened with O_TRUNC and followed symlinks,
    // which is exactly the hole this hardens against.
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut out = opts.open(tmp).map_err(|source| InstallError::Io {
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
    fn binary_install_refuses_to_overwrite_existing_dest() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let cached = write_cached(cache.path(), "agentsight", b"v2-bytes");
        let dest = layout.bin_dir.join("agentsight");

        // Pre-existing file from a prior install / external source.
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"v1-bytes").unwrap();

        let err = runner
            .install("binary", &cached, &[dest.clone()])
            .expect_err("second install must refuse");
        match err {
            InstallError::DestExists { path } => assert_eq!(path, dest),
            other => panic!("expected DestExists, got {other:?}"),
        }

        // Pre-existing file must be untouched — and no .tmp sibling left behind.
        assert_eq!(std::fs::read(&dest).unwrap(), b"v1-bytes");
        let tmp = tmp_sibling(&dest);
        assert!(!tmp.exists(), ".tmp sibling must not be created");
    }

    #[test]
    fn tar_gz_install_refuses_when_any_dest_preexists() {
        // Pre-existence check runs before extraction, so neither dest is
        // written even if only one of them collides.
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
        std::fs::create_dir_all(dest_data.parent().unwrap()).unwrap();
        std::fs::write(&dest_data, b"existing-data").unwrap();

        let err = runner
            .install("tar_gz", &cached, &[dest_bin.clone(), dest_data.clone()])
            .expect_err("must refuse");
        match err {
            InstallError::DestExists { path } => assert_eq!(path, dest_data),
            other => panic!("expected DestExists, got {other:?}"),
        }
        assert!(!dest_bin.exists(), "bin dest must not be created");
        assert_eq!(std::fs::read(&dest_data).unwrap(), b"existing-data");
    }

    #[test]
    fn binary_install_dotdot_segment_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        // dest = <bin_dir>/../escape/file — passes the old lexical
        // starts_with check but would write outside bin_dir.
        let dest = layout.bin_dir.join("..").join("escape").join("file");
        let err = runner
            .install("binary", &cached, &[dest.clone()])
            .expect_err("must reject");
        match err {
            InstallError::TraversalSegment { path } => assert_eq!(path, dest),
            other => panic!("expected TraversalSegment, got {other:?}"),
        }
    }

    #[test]
    fn binary_install_dotdot_at_tail_rejected() {
        // `..` as the final segment would resolve to a directory and let
        // rename overwrite something the user did not name. Same defense
        // as the mid-path case but covers the tail position explicitly.
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let dest = layout.bin_dir.join("sub").join("..");
        let err = runner
            .install("binary", &cached, &[dest.clone()])
            .expect_err("must reject");
        match err {
            InstallError::TraversalSegment { path } => assert_eq!(path, dest),
            other => panic!("expected TraversalSegment, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn binary_install_refuses_broken_symlink_dest() {
        // exists() returns false for a broken symlink (target missing) but
        // symlink_metadata() returns Ok. We must treat the broken symlink
        // as "occupied" and refuse, otherwise rename() would silently
        // replace it.
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "agentsight", b"new-bytes");

        let dest = layout.bin_dir.join("agentsight");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/nonexistent/target", &dest).unwrap();
        assert!(!dest.exists(), "test precondition: broken symlink");
        assert!(
            fs::symlink_metadata(&dest).is_ok(),
            "symlink itself present"
        );

        let err = runner
            .install("binary", &cached, &[dest.clone()])
            .expect_err("must refuse");
        match err {
            InstallError::DestExists { path } => assert_eq!(path, dest),
            other => panic!("expected DestExists, got {other:?}"),
        }
        // Symlink untouched.
        assert!(fs::symlink_metadata(&dest).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn binary_install_symlink_ancestor_escapes_root_rejected() {
        // bin_dir/escape -> <outside>, dest = bin_dir/escape/file. The
        // lexical starts_with check passes (it's literally under bin_dir),
        // but canonicalize_nearest_existing resolves the symlink and the
        // canonical dest no longer lives under the canonical root.
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        fs::create_dir_all(&layout.bin_dir).unwrap();
        let escape_link = layout.bin_dir.join("escape");
        std::os::unix::fs::symlink(outside.path(), &escape_link).unwrap();

        let dest = escape_link.join("file");
        let err = runner
            .install("binary", &cached, &[dest.clone()])
            .expect_err("must reject");
        assert!(
            matches!(err, InstallError::ExternalPath { ref path } if path == &dest),
            "expected ExternalPath for symlink-escape, got {err:?}",
        );
        assert!(
            !outside.path().join("file").exists(),
            "must not write through the symlink",
        );
    }

    #[cfg(unix)]
    #[test]
    fn binary_install_refuses_when_tmp_sibling_is_a_symlink() {
        // The atomic-write step writes to `{dest}.tmp` and then rename(2)s
        // it into place. If `{dest}.tmp` is a pre-placed symlink to a file
        // outside the ANOLISA-owned roots, the old code (`File::create`)
        // would follow it and corrupt that external file — bypassing
        // every dest-side guard we just added. The fix opens with
        // O_CREAT|O_EXCL (+ O_NOFOLLOW on Unix) so the open itself fails.
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "agentsight", b"new-bytes");

        let dest = layout.bin_dir.join("agentsight");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        // The plant lives at `{dest}.tmp` — the exact path
        // `tmp_sibling(dest)` returns — and targets an external file.
        let outside_target = outside.path().join("victim");
        fs::write(&outside_target, b"untouched-bytes").unwrap();
        let tmp_plant = {
            let mut s = dest.as_os_str().to_os_string();
            s.push(".tmp");
            PathBuf::from(s)
        };
        std::os::unix::fs::symlink(&outside_target, &tmp_plant).unwrap();

        let err = runner
            .install("binary", &cached, &[dest.clone()])
            .expect_err("must refuse to write through symlinked tmp");
        match err {
            InstallError::Io { path, .. } => assert_eq!(path, tmp_plant),
            other => panic!("expected Io on tmp, got {other:?}"),
        }

        // External file is untouched (the most important invariant).
        let victim_bytes = fs::read(&outside_target).expect("external file readable");
        assert_eq!(
            victim_bytes, b"untouched-bytes",
            "the symlink target must not be written through",
        );
        // Destination was never created.
        assert!(!dest.exists(), "dest must not be installed");
    }

    #[cfg(unix)]
    #[test]
    fn tar_gz_install_refuses_when_tmp_sibling_is_a_symlink() {
        // Same defense applies to the tar_gz backend — it routes through
        // the same `write_dest_atomic` helper so a single fix covers both,
        // but we lock that down with an explicit regression test so a
        // future refactor that splits the helpers cannot regress one
        // backend without tripping a test.
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let gz = build_tar_gz(&[("bin/agentsight", b"new-bytes")]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let dest = layout.bin_dir.join("agentsight");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let outside_target = outside.path().join("victim");
        fs::write(&outside_target, b"untouched-bytes").unwrap();
        let tmp_plant = {
            let mut s = dest.as_os_str().to_os_string();
            s.push(".tmp");
            PathBuf::from(s)
        };
        std::os::unix::fs::symlink(&outside_target, &tmp_plant).unwrap();

        let err = runner
            .install("tar_gz", &cached, &[dest.clone()])
            .expect_err("must refuse to write through symlinked tmp");
        match err {
            InstallError::Io { path, .. } => assert_eq!(path, tmp_plant),
            other => panic!("expected Io on tmp, got {other:?}"),
        }

        let victim_bytes = fs::read(&outside_target).expect("external file readable");
        assert_eq!(victim_bytes, b"untouched-bytes");
        assert!(!dest.exists());
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
