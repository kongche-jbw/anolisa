use clap::Parser;

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct StatusArgs {
    /// Show detail for a specific capability (omit for aggregate view)
    pub capability: Option<String>,
}

pub fn handle(args: StatusArgs, _ctx: &CliContext) -> Result<(), CliError> {
    let command = match args.capability {
        Some(cap) => format!("status {cap}"),
        None => "status".to_string(),
    };
    Err(CliError::not_implemented(command))
}
