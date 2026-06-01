//! `anolisa logs [OBJECT]` — query the central operation/audit log.
//!
//! Reads the JSONL log file resolved by [`anolisa_platform::fs_layout::FsLayout`]
//! and runs the request through [`anolisa_core::CentralLog::query`] (see
//! launch spec §7.1 and §8.4). Output is human by default; `--json` wraps the
//! result in the standard [`crate::response::CliResponse`] envelope.
//!
//! A missing log file is the expected fresh-install state and produces an
//! empty result (no error). All flags are passive filters; this command
//! never writes to the log.

use clap::Parser;

use anolisa_core::{CentralLog, LogFilter, LogKind, LogRecord, Severity};
use anolisa_platform::fs_layout::FsLayout;

use crate::context::{CliContext, InstallMode};
use crate::response::{CliError, render_json};

const COMMAND: &str = "logs";

/// Default cap on returned records when `--limit` is omitted.
const DEFAULT_LIMIT: usize = 50;

#[derive(Parser)]
pub struct LogsArgs {
    /// Filter target: capability / component / operation id / log source / `all`.
    /// Omit to query everything.
    #[arg(value_name = "OBJECT")]
    pub object: Option<String>,
    /// Match exact operation id (e.g. `op-20260601-001`).
    #[arg(long, value_name = "ID")]
    pub operation_id: Option<String>,
    /// Restrict to a record kind: `operation` or `component`.
    #[arg(long, value_name = "KIND")]
    pub kind: Option<String>,
    /// Match exact source (e.g. `anolisa-cli`, `agentsight`).
    #[arg(long, value_name = "SOURCE")]
    pub source: Option<String>,
    /// Match exact component name.
    #[arg(long, value_name = "COMP")]
    pub component: Option<String>,
    /// Minimum severity: `debug` | `info` | `warn` | `error`.
    #[arg(long, value_name = "LEVEL")]
    pub severity: Option<String>,
    /// Lexicographic ISO8601 lower bound on `started_at`.
    #[arg(long, value_name = "ISO")]
    pub since: Option<String>,
    /// Cap returned records (default 50).
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

pub fn handle(args: LogsArgs, ctx: &CliContext) -> Result<(), CliError> {
    let filter = build_filter(&args)?;
    let layout = resolve_layout(ctx);
    let log = CentralLog::open(layout.central_log.clone());

    let records = log
        .query(&filter)
        .map_err(|err| CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "failed to query central log at {}: {err}",
                layout.central_log.display()
            ),
        })?;

    if ctx.json {
        let data = serde_json::to_value(&records).map_err(|err| CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!("failed to serialize log records: {err}"),
        })?;
        return render_json(COMMAND, data);
    }

    if !ctx.quiet {
        render_human(&records, ctx.verbose);
    }
    Ok(())
}

/// Translate the parsed CLI args into a [`LogFilter`], surfacing
/// validation errors as `INVALID_ARGUMENT`.
fn build_filter(args: &LogsArgs) -> Result<LogFilter, CliError> {
    let kind = match args.kind.as_deref() {
        None => None,
        Some(v) => Some(parse_kind(v)?),
    };
    let severity_at_least = match args.severity.as_deref() {
        None => None,
        Some(v) => Some(parse_severity(v)?),
    };
    Ok(LogFilter {
        kind,
        source: args.source.clone(),
        component: args.component.clone(),
        operation_id: args.operation_id.clone(),
        severity_at_least,
        object: args.object.clone(),
        since: args.since.clone(),
        limit: Some(args.limit.unwrap_or(DEFAULT_LIMIT)),
    })
}

fn parse_kind(raw: &str) -> Result<LogKind, CliError> {
    match raw {
        "operation" => Ok(LogKind::Operation),
        "component" => Ok(LogKind::Component),
        other => Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!("--kind expects 'operation' or 'component', got '{other}'"),
        }),
    }
}

fn parse_severity(raw: &str) -> Result<Severity, CliError> {
    match raw {
        "debug" => Ok(Severity::Debug),
        "info" => Ok(Severity::Info),
        "warn" | "warning" => Ok(Severity::Warn),
        "error" => Ok(Severity::Error),
        other => Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!("--severity expects one of debug|info|warn|error, got '{other}'"),
        }),
    }
}

/// Resolve the install-mode layout from the shared context, honoring
/// `--prefix` for system-mode. User-mode resolves `$HOME` via
/// [`anolisa_env::EnvService::detect`] to stay consistent with the
/// `env` command's view of the world.
fn resolve_layout(ctx: &CliContext) -> FsLayout {
    match ctx.install_mode {
        InstallMode::System => FsLayout::system(ctx.prefix.clone()),
        InstallMode::User => {
            let home = anolisa_env::EnvService::detect().home;
            FsLayout::user(home)
        }
    }
}

fn render_human(records: &[LogRecord], verbose: bool) {
    for record in records {
        let op_id = record.operation_id.as_deref().unwrap_or("-");
        let status = record
            .status
            .map(severity_status_str)
            .unwrap_or_else(|| "-".to_string());
        let severity = severity_str(record.severity);
        let command = truncate(&record.command, 40);
        println!(
            "{ts}  {sev:<5}  {op:<18}  {cmd:<40}  {status}",
            ts = record.started_at,
            sev = severity,
            op = op_id,
            cmd = command,
            status = status,
        );
        if verbose {
            if !record.message.is_empty() {
                println!("    message: {}", record.message);
            }
            if !record.objects.is_empty() {
                println!("    objects: {}", record.objects.join(", "));
            }
            if !record.warnings.is_empty() {
                for warning in &record.warnings {
                    println!("    warning: {}", warning);
                }
            }
        }
    }
}

fn severity_str(sev: Severity) -> &'static str {
    match sev {
        Severity::Debug => "debug",
        Severity::Info => "info",
        Severity::Warn => "warn",
        Severity::Error => "error",
    }
}

fn severity_status_str(status: anolisa_core::LogStatus) -> String {
    match status {
        anolisa_core::LogStatus::Ok => "ok".to_string(),
        anolisa_core::LogStatus::Failed => "failed".to_string(),
        anolisa_core::LogStatus::RolledBack => "rolled_back".to_string(),
        anolisa_core::LogStatus::Partial => "partial".to_string(),
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        let mut out: String = value.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anolisa_core::{CentralLog, LogFilter};

    /// Write three hand-crafted JSONL records and confirm `CentralLog::query`
    /// applies the operation_id filter (the lower-level helper we depend on).
    #[test]
    fn query_helper_filters_records_by_operation_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("central.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"kind":"operation","operation_id":"op-1","command":"enable agent-observability","source":"anolisa-cli","severity":"info","message":"ok","actor":"kongche","started_at":"2026-06-01T10:00:00Z","status":"ok"}"#,
                "\n",
                r#"{"kind":"operation","operation_id":"op-2","command":"enable tokenless","source":"anolisa-cli","severity":"info","message":"ok","actor":"kongche","started_at":"2026-06-01T10:00:01Z","status":"ok"}"#,
                "\n",
                r#"{"kind":"operation","operation_id":"op-3","command":"enable ws-ckpt","source":"anolisa-cli","severity":"info","message":"ok","actor":"kongche","started_at":"2026-06-01T10:00:02Z","status":"ok"}"#,
                "\n",
            ),
        )
        .expect("write log file");

        let log = CentralLog::open(path);
        let hits = log
            .query(&LogFilter {
                operation_id: Some("op-2".to_string()),
                ..Default::default()
            })
            .expect("query");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].operation_id.as_deref(), Some("op-2"));
    }

    #[test]
    fn missing_log_file_yields_empty() {
        // Fresh-install path — file does not exist, query returns []
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist.jsonl");
        let log = CentralLog::open(path);
        let hits = log.query(&LogFilter::default()).expect("query");
        assert!(hits.is_empty());
    }

    #[test]
    fn parse_severity_accepts_known_values() {
        assert_eq!(parse_severity("debug").unwrap(), Severity::Debug);
        assert_eq!(parse_severity("info").unwrap(), Severity::Info);
        assert_eq!(parse_severity("warn").unwrap(), Severity::Warn);
        assert_eq!(parse_severity("warning").unwrap(), Severity::Warn);
        assert_eq!(parse_severity("error").unwrap(), Severity::Error);
        assert!(parse_severity("loud").is_err());
    }

    #[test]
    fn parse_kind_accepts_known_values() {
        assert_eq!(parse_kind("operation").unwrap(), LogKind::Operation);
        assert_eq!(parse_kind("component").unwrap(), LogKind::Component);
        assert!(parse_kind("other").is_err());
    }
}
