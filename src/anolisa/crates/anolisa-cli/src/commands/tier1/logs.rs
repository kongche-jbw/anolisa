use clap::Parser;

use crate::context::CliContext;
use crate::response::CliError;

/// `anolisa logs [OBJECT]` is the central log query entry point.
///
/// It exposes ANOLISA's own operation/audit log plus the component-reported
/// log (see launch spec §7.1). OBJECT is an optional filter and may be any
/// of:
///   - a capability name (`agent-observability`)
///   - a component name (`agentsight`, `tokenless`)
///   - an operation id (`op-20260601-001`)
///   - a log source (`component`, `operation`)
///   - the literal `all` to query every source
///
/// Omitting OBJECT returns the unfiltered central log.
#[derive(Parser)]
pub struct LogsArgs {
    /// Filter target: capability / component / operation id / log source / `all`.
    /// Omit to query everything.
    #[arg(value_name = "OBJECT")]
    pub object: Option<String>,
    /// Stream new log entries (like `tail -f`)
    #[arg(long)]
    pub follow: bool,
    /// Time window (e.g. `5m`, `1h`, `1d`)
    #[arg(long, value_name = "DURATION")]
    pub since: Option<String>,
    /// Number of trailing lines to show
    #[arg(long, value_name = "N")]
    pub lines: Option<u32>,
}

pub fn handle(args: LogsArgs, _ctx: &CliContext) -> Result<(), CliError> {
    let command = match &args.object {
        Some(obj) => format!("logs {obj}"),
        None => "logs".to_string(),
    };
    Err(CliError::not_implemented_with_hint(
        command,
        "central log store (anolisa-core::central_log) is not implemented yet",
    ))
}
