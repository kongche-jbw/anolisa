//! Central audit/operation log.
//!
//! `CentralLog` is the append-only JSON Lines store that backs the
//! `anolisa logs` command (launch spec §8.4). Each record is serialised
//! on a single line with a trailing `\n`, so callers can tail/grep the
//! file without needing structured tooling.
//!
//! The current implementation is the P1-A skeleton: append uses
//! `OpenOptions::append`, and `query` is a sequential scan with simple
//! filters. Rotation, indexing, and follow-mode are future work.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A single line in the central log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRecord {
    /// ISO8601 UTC timestamp (e.g. `2026-06-01T10:00:00Z`).
    pub ts: String,
    /// Event name, e.g. `capability.enable`, `tx.commit`.
    pub event: String,
    /// Who emitted the record (`cli` for now; later `daemon`, components).
    pub actor: String,
    /// Originating CLI invocation, e.g. `anolisa enable agent-observability`.
    pub command: String,
    /// Object the event is about (capability or component name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    /// Outcome tag: `ok` | `fail` | `skipped` | `dry-run`.
    pub outcome: String,
    /// Free-form structured payload, may be `Null`.
    #[serde(default)]
    pub details: serde_json::Value,
    /// Transaction id when emitted inside a `tx.*` block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_id: Option<String>,
}

/// Subset of fields to filter on during [`CentralLog::query`].
#[derive(Debug, Default, Clone)]
pub struct LogFilter {
    /// Match exact `object` (capability or component name).
    pub object: Option<String>,
    /// Match exact `event` name.
    pub event: Option<String>,
    /// Lexicographic lower bound on `ts` (ISO8601 sorts correctly).
    pub since: Option<String>,
    /// Cap the returned record count (taken from the tail of the match list).
    pub limit: Option<usize>,
}

/// Append-only JSONL central log.
#[derive(Debug, Clone)]
pub struct CentralLog {
    path: PathBuf,
}

/// Errors raised by [`CentralLog`].
#[derive(Debug, thiserror::Error)]
pub enum CentralLogError {
    #[error("io error while accessing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to serialize log record: {0}")]
    Serialize(#[from] serde_json::Error),
}

impl CentralLog {
    /// Open (does not create) a log handle for `path`. The file is
    /// created lazily on the first `append`.
    pub fn open(path: PathBuf) -> Self {
        Self { path }
    }

    /// Path the log writes to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append a single record, terminated by `\n`. Parent directories
    /// are created on demand.
    pub fn append(&self, record: &LogRecord) -> Result<(), CentralLogError> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| CentralLogError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
        }

        let mut line = serde_json::to_string(record)?;
        line.push('\n');

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| CentralLogError::Io {
                path: self.path.clone(),
                source,
            })?;
        file.write_all(line.as_bytes())
            .map_err(|source| CentralLogError::Io {
                path: self.path.clone(),
                source,
            })?;
        Ok(())
    }

    /// Sequentially scan the log, returning matching records. Missing
    /// file yields an empty result.
    pub fn query(&self, filter: &LogFilter) -> Result<Vec<LogRecord>, CentralLogError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path).map_err(|source| CentralLogError::Io {
            path: self.path.clone(),
            source,
        })?;
        let reader = BufReader::new(file);

        let mut matches: Vec<LogRecord> = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(|source| CentralLogError::Io {
                path: self.path.clone(),
                source,
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let record: LogRecord = serde_json::from_str(&line)?;
            if record_matches(&record, filter) {
                matches.push(record);
            }
        }

        if let Some(limit) = filter.limit {
            if matches.len() > limit {
                let drop = matches.len() - limit;
                matches.drain(..drop);
            }
        }
        Ok(matches)
    }
}

fn record_matches(record: &LogRecord, filter: &LogFilter) -> bool {
    if let Some(obj) = &filter.object {
        match &record.object {
            Some(record_obj) if record_obj == obj => {}
            _ => return false,
        }
    }
    if let Some(ev) = &filter.event {
        if &record.event != ev {
            return false;
        }
    }
    if let Some(since) = &filter.since {
        if record.ts.as_str() < since.as_str() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(ts: &str, event: &str, object: Option<&str>) -> LogRecord {
        LogRecord {
            ts: ts.to_string(),
            event: event.to_string(),
            actor: "cli".to_string(),
            command: "anolisa test".to_string(),
            object: object.map(|s| s.to_string()),
            outcome: "ok".to_string(),
            details: serde_json::Value::Null,
            tx_id: None,
        }
    }

    #[test]
    fn append_creates_file_and_writes_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let log = CentralLog::open(dir.path().join("nested").join("audit.jsonl"));

        log.append(&rec("2026-06-01T10:00:00Z", "tx.begin", Some("foo")))
            .unwrap();
        log.append(&rec("2026-06-01T10:00:01Z", "tx.commit", Some("foo")))
            .unwrap();

        let contents = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        // Each line must parse independently.
        for line in lines {
            serde_json::from_str::<LogRecord>(line).unwrap();
        }
    }

    #[test]
    fn query_missing_file_yields_empty() {
        let dir = tempfile::tempdir().unwrap();
        let log = CentralLog::open(dir.path().join("audit.jsonl"));
        let out = log.query(&LogFilter::default()).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn query_filters_by_object_event_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let log = CentralLog::open(dir.path().join("audit.jsonl"));

        log.append(&rec("2026-06-01T10:00:00Z", "tx.begin", Some("foo")))
            .unwrap();
        log.append(&rec("2026-06-01T10:00:01Z", "tx.commit", Some("foo")))
            .unwrap();
        log.append(&rec("2026-06-01T10:00:02Z", "tx.commit", Some("bar")))
            .unwrap();

        // event filter
        let event_only = log
            .query(&LogFilter {
                event: Some("tx.commit".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(event_only.len(), 2);

        // object filter
        let foo_only = log
            .query(&LogFilter {
                object: Some("foo".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(foo_only.len(), 2);
        assert!(foo_only.iter().all(|r| r.object.as_deref() == Some("foo")));

        // limit takes the tail
        let limited = log
            .query(&LogFilter {
                limit: Some(2),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].ts, "2026-06-01T10:00:01Z");
        assert_eq!(limited[1].ts, "2026-06-01T10:00:02Z");
    }

    #[test]
    fn query_since_uses_lexicographic_lower_bound() {
        let dir = tempfile::tempdir().unwrap();
        let log = CentralLog::open(dir.path().join("audit.jsonl"));
        log.append(&rec("2026-05-01T00:00:00Z", "tx.begin", None))
            .unwrap();
        log.append(&rec("2026-06-01T00:00:00Z", "tx.commit", None))
            .unwrap();

        let recent = log
            .query(&LogFilter {
                since: Some("2026-05-15T00:00:00Z".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].event, "tx.commit");
    }
}
