//! Cache-only artifact downloader.
//!
//! Milestone scope: file:// plus HTTP(S); sha256 verification when an
//! expected hash is supplied; cache is overwrite-on-conflict, no content
//! addressing.
//!
//! The cache writes through a sibling `.tmp` file and renames into place so
//! a partial fetch never leaves a half-written entry visible at the cached
//! path.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Side-effect-free record of a successful download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedArtifact {
    /// Absolute path inside the cache. Stable across re-fetches of the
    /// same URL+sha256 pair.
    pub cached_path: PathBuf,
    /// Lowercase-hex sha256 of the cached bytes.
    pub sha256: String,
}

/// Errors raised by [`DownloadCache::fetch`].
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("unsupported URL scheme '{scheme}' — supported schemes: file, http, https")]
    UnsupportedScheme { scheme: String },

    #[error("malformed URL '{url}': {reason}")]
    MalformedUrl { url: String, reason: String },

    #[error("http status {status} while fetching {url}")]
    HttpStatus { url: String, status: u16 },

    #[error("network error while fetching {url}: {reason}")]
    Network { url: String, reason: String },

    #[error("io error while accessing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("sha256 mismatch for {url}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        url: String,
        expected: String,
        actual: String,
    },
}

/// Local artifact cache rooted at a single directory.
///
/// `fetch` resolves a URL to a file inside `<cache_root>/downloads/` and
/// optionally verifies its sha256. The cache is overwrite-on-conflict: a
/// repeat fetch of the same URL replaces the cached bytes.
pub struct DownloadCache {
    root: PathBuf,
}

impl DownloadCache {
    /// Build a cache rooted under `cache_root`. The directory is created
    /// lazily on first fetch.
    pub fn new(cache_root: PathBuf) -> Self {
        Self { root: cache_root }
    }

    /// Path the cache writes into. Useful for tests and `--verbose`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Fetch `url` (file:// or HTTP(S)). Verifies sha256 when
    /// `expected_sha256` is Some — mismatches return [`DownloadError::ChecksumMismatch`]
    /// and the cache file is removed before returning. When `expected_sha256`
    /// is None the bytes are still hashed and returned, but no verification
    /// is enforced (caller decides whether to refuse). Atomic on disk:
    /// writes to a sibling `.tmp` file then renames into place.
    ///
    /// Re-fetching the same URL is allowed and overwrites the cache entry
    /// (no content-addressable layout — keep this milestone simple).
    pub fn fetch(
        &self,
        url: &str,
        expected_sha256: Option<&str>,
    ) -> Result<DownloadedArtifact, DownloadError> {
        let scheme = scheme_of(url)?;
        let cached_path = self.cached_path_for(url);

        let downloads_dir = cached_path
            .parent()
            .expect("cached_path always has a parent under <root>/downloads")
            .to_path_buf();
        fs::create_dir_all(&downloads_dir).map_err(|source| DownloadError::Io {
            path: downloads_dir.clone(),
            source,
        })?;

        let tmp_path = tmp_sibling(&cached_path);
        let sha256 = match scheme {
            "file" => stream_copy_file_and_hash(&parse_file_url(url)?, &tmp_path),
            "http" | "https" => stream_http_and_hash(url, &tmp_path),
            other => Err(DownloadError::UnsupportedScheme {
                scheme: other.to_string(),
            }),
        };
        let sha256 = match sha256 {
            Ok(h) => h,
            Err(err) => {
                // Best-effort: drop the partial .tmp so we don't leak it.
                let _ = fs::remove_file(&tmp_path);
                return Err(err);
            }
        };

        fs::rename(&tmp_path, &cached_path).map_err(|source| {
            let _ = fs::remove_file(&tmp_path);
            DownloadError::Io {
                path: cached_path.clone(),
                source,
            }
        })?;

        if let Some(expected) = expected_sha256 {
            let expected_norm = expected.to_ascii_lowercase();
            if expected_norm != sha256 {
                // Remove the poisoned cache entry so a future invocation does
                // not see bytes whose hash we already rejected.
                let _ = fs::remove_file(&cached_path);
                return Err(DownloadError::ChecksumMismatch {
                    url: url.to_string(),
                    expected: expected_norm,
                    actual: sha256,
                });
            }
        }

        Ok(DownloadedArtifact {
            cached_path,
            sha256,
        })
    }

    fn cached_path_for(&self, url: &str) -> PathBuf {
        let basename = url
            .rsplit('/')
            .next()
            .unwrap_or("")
            .split(['?', '#'])
            .next()
            .unwrap_or("");
        let name = if basename.is_empty() {
            format!("url-{}", short_hash_of(url))
        } else {
            basename.to_string()
        };
        self.root.join("downloads").join(name)
    }
}

fn scheme_of(url: &str) -> Result<&str, DownloadError> {
    let Some(idx) = url.find("://") else {
        return Err(DownloadError::MalformedUrl {
            url: url.to_string(),
            reason: "missing scheme separator '://'".to_string(),
        });
    };
    Ok(&url[..idx])
}

fn parse_file_url(url: &str) -> Result<PathBuf, DownloadError> {
    let idx = url.find("://").expect("scheme already parsed by caller");
    let scheme = &url[..idx];
    let rest = &url[idx + 3..];
    if scheme != "file" {
        return Err(DownloadError::UnsupportedScheme {
            scheme: scheme.to_string(),
        });
    }
    // file:// requires an empty host, leaving the absolute path starting at '/'.
    if !rest.starts_with('/') {
        return Err(DownloadError::MalformedUrl {
            url: url.to_string(),
            reason: "file:// URL must have an empty host and absolute path".to_string(),
        });
    }
    Ok(PathBuf::from(rest))
}

fn tmp_sibling(cached_path: &Path) -> PathBuf {
    let mut s = cached_path.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

fn stream_copy_file_and_hash(src: &Path, dst: &Path) -> Result<String, DownloadError> {
    let mut input = File::open(src).map_err(|source| DownloadError::Io {
        path: src.to_path_buf(),
        source,
    })?;
    stream_reader_and_hash(&mut input, dst, src)
}

fn stream_http_and_hash(url: &str, dst: &Path) -> Result<String, DownloadError> {
    let mut last_error = None;
    for attempt in 1..=3 {
        let _ = fs::remove_file(dst);
        match stream_http_once_and_hash(url, dst) {
            Ok(hash) => return Ok(hash),
            Err(err @ DownloadError::Network { .. }) if attempt < 3 => {
                last_error = Some(err);
                std::thread::sleep(Duration::from_secs(attempt));
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_error.expect("retry loop stores the last network error"))
}

fn stream_http_once_and_hash(url: &str, dst: &Path) -> Result<String, DownloadError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .build();
    let response = agent.get(url).call().map_err(|err| match err {
        ureq::Error::Status(status, _) => DownloadError::HttpStatus {
            url: url.to_string(),
            status,
        },
        ureq::Error::Transport(transport) => DownloadError::Network {
            url: url.to_string(),
            reason: transport.to_string(),
        },
    })?;
    let mut input = response.into_reader();
    stream_reader_and_hash(&mut input, dst, Path::new(url)).map_err(|err| match err {
        DownloadError::Io { source, .. } => DownloadError::Network {
            url: url.to_string(),
            reason: source.to_string(),
        },
        other => other,
    })
}

fn stream_reader_and_hash<R: Read>(
    input: &mut R,
    dst: &Path,
    read_path: &Path,
) -> Result<String, DownloadError> {
    // Same hardening as InstallRunner::stream_write_and_hash: open the
    // tmp sibling with O_CREAT|O_EXCL (+ O_NOFOLLOW on Unix) so a
    // pre-placed `.tmp` symlink can't redirect the cache write through
    // to a path the attacker chose. `File::create` (the old code) used
    // O_TRUNC and followed symlinks, which is the hole this closes.
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut output = opts.open(dst).map_err(|source| DownloadError::Io {
        path: dst.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8 * 1024];
    loop {
        let n = input.read(&mut buf).map_err(|source| DownloadError::Io {
            path: read_path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        output
            .write_all(&buf[..n])
            .map_err(|source| DownloadError::Io {
                path: dst.to_path_buf(),
                source,
            })?;
    }
    output.flush().map_err(|source| DownloadError::Io {
        path: dst.to_path_buf(),
        source,
    })?;
    Ok(to_lower_hex(&hasher.finalize()))
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

fn short_hash_of(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let full = to_lower_hex(&hasher.finalize());
    full[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::net::TcpListener;
    use std::thread;
    use tempfile::tempdir;

    fn write_source(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).expect("write source file");
        p
    }

    fn file_url(p: &Path) -> String {
        format!("file://{}", p.to_str().expect("utf8 path"))
    }

    fn sha256_of(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        to_lower_hex(&h.finalize())
    }

    fn serve_once(status: &str, body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let status = status.to_string();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept one request");
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write head");
            stream.write_all(body).expect("write body");
        });
        format!("http://{addr}/agentsight")
    }

    fn serve_drop_then_ok(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept dropped request");
            drop(stream);

            let (mut stream, _) = listener.accept().expect("accept retry request");
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write head");
            stream.write_all(body).expect("write body");
        });
        format!("http://{addr}/agentsight")
    }

    #[test]
    fn fetch_file_uri_with_matching_sha_succeeds() {
        let src_dir = tempdir().unwrap();
        let cache_dir = tempdir().unwrap();
        let content = b"hello world";
        let src = write_source(src_dir.path(), "x.bin", content);
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());

        let expected = sha256_of(content);
        let got = cache
            .fetch(&file_url(&src), Some(&expected))
            .expect("fetch ok");

        assert!(got.cached_path.exists());
        assert_eq!(got.sha256, expected);
    }

    #[test]
    fn fetch_file_uri_without_expected_sha_returns_computed_sha() {
        let src_dir = tempdir().unwrap();
        let cache_dir = tempdir().unwrap();
        let src = write_source(src_dir.path(), "x.bin", b"hello world");
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());

        let got = cache.fetch(&file_url(&src), None).expect("fetch ok");

        assert_eq!(got.sha256.len(), 64);
        assert!(
            got.sha256
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
    }

    #[test]
    fn fetch_file_uri_with_mismatched_sha_returns_error_and_removes_cache_file() {
        let src_dir = tempdir().unwrap();
        let cache_dir = tempdir().unwrap();
        let src = write_source(src_dir.path(), "x.bin", b"hello world");
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());

        let wrong = "0".repeat(64);
        let err = cache
            .fetch(&file_url(&src), Some(&wrong))
            .expect_err("must error");

        match err {
            DownloadError::ChecksumMismatch { .. } => {}
            other => panic!("expected ChecksumMismatch, got {other:?}"),
        }
        let cached = cache_dir.path().join("downloads").join("x.bin");
        assert!(!cached.exists(), "poisoned cache file must be removed");
    }

    #[test]
    fn fetch_file_uri_missing_source_returns_io_error() {
        let cache_dir = tempdir().unwrap();
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());

        let err = cache
            .fetch("file:///nonexistent/path/to/missing.bin", None)
            .expect_err("must error");

        match err {
            DownloadError::Io { .. } => {}
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn fetch_http_url_with_matching_sha_succeeds() {
        let cache_dir = tempdir().unwrap();
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());
        let content = b"hello from release";
        let url = serve_once("200 OK", content);

        let expected = sha256_of(content);
        let got = cache.fetch(&url, Some(&expected)).expect("fetch ok");

        assert!(got.cached_path.exists());
        assert_eq!(got.sha256, expected);
        assert_eq!(fs::read(got.cached_path).unwrap(), content);
    }

    #[test]
    fn fetch_http_non_success_returns_status_error() {
        let cache_dir = tempdir().unwrap();
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());
        let url = serve_once("404 Not Found", b"missing");

        let err = cache.fetch(&url, None).expect_err("must error");

        match err {
            DownloadError::HttpStatus { status, .. } => assert_eq!(status, 404),
            other => panic!("expected HttpStatus, got {other:?}"),
        }
    }

    #[test]
    fn fetch_http_retries_transient_network_error() {
        let cache_dir = tempdir().unwrap();
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());
        let content = b"retry payload";
        let url = serve_drop_then_ok(content);

        let got = cache
            .fetch(&url, Some(&sha256_of(content)))
            .expect("retry should recover");

        assert_eq!(got.sha256, sha256_of(content));
        assert_eq!(fs::read(got.cached_path).unwrap(), content);
    }

    #[test]
    fn fetch_malformed_url_without_scheme_returns_malformed() {
        let cache_dir = tempdir().unwrap();
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());

        let err = cache.fetch("/tmp/whatever", None).expect_err("must error");

        match err {
            DownloadError::MalformedUrl { .. } => {}
            other => panic!("expected MalformedUrl, got {other:?}"),
        }
    }

    #[test]
    fn cache_root_is_returned() {
        let cache_dir = tempdir().unwrap();
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());
        assert_eq!(cache.root(), cache_dir.path());
    }

    #[cfg(unix)]
    #[test]
    fn fetch_refuses_when_tmp_sibling_is_a_symlink_and_does_not_corrupt_target() {
        // The cache writes through `<cached>.tmp` before renaming into
        // place. A pre-placed `.tmp` symlink targeting any file the
        // attacker chooses would, under the old `File::create` path, be
        // followed and overwritten — defeating every other safety in
        // the runner. With O_CREAT|O_EXCL + O_NOFOLLOW the open itself
        // fails and the external file is untouched.
        let src_dir = tempdir().unwrap();
        let cache_dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let src = write_source(src_dir.path(), "x.bin", b"new-cache-bytes");
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());

        // Compute the cached_path and plant `.tmp` as a symlink to
        // an external "victim" file before calling fetch.
        let downloads = cache_dir.path().join("downloads");
        fs::create_dir_all(&downloads).unwrap();
        let cached = downloads.join("x.bin");
        let outside_target = outside.path().join("victim");
        fs::write(&outside_target, b"untouched-bytes").unwrap();
        let tmp_plant = {
            let mut s = cached.as_os_str().to_os_string();
            s.push(".tmp");
            PathBuf::from(s)
        };
        std::os::unix::fs::symlink(&outside_target, &tmp_plant).unwrap();

        let err = cache
            .fetch(&file_url(&src), None)
            .expect_err("must refuse to write through symlinked tmp");
        match err {
            DownloadError::Io { path, .. } => assert_eq!(path, tmp_plant),
            other => panic!("expected Io on tmp, got {other:?}"),
        }

        // External file is untouched.
        let victim_bytes = fs::read(&outside_target).expect("external file readable");
        assert_eq!(
            victim_bytes, b"untouched-bytes",
            "the symlink target must not be written through",
        );
        // Cached entry was never produced.
        assert!(!cached.exists(), "no cache file may exist after refusal");
    }

    #[test]
    fn fetch_overwrites_existing_cache_entry() {
        let src_dir = tempdir().unwrap();
        let cache_dir = tempdir().unwrap();
        let src = write_source(src_dir.path(), "x.bin", b"first");
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());

        let first = cache.fetch(&file_url(&src), None).expect("fetch ok");
        assert_eq!(first.sha256, sha256_of(b"first"));

        fs::write(&src, b"second-larger").expect("rewrite source");
        let second = cache.fetch(&file_url(&src), None).expect("refetch ok");

        assert_eq!(second.sha256, sha256_of(b"second-larger"));
        assert_eq!(first.cached_path, second.cached_path);
        let bytes = fs::read(&second.cached_path).unwrap();
        assert_eq!(bytes, b"second-larger");
    }
}
