use clap::Parser;

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct RestartArgs {
    /// Capability whose underlying service to restart
    pub capability: String,
}

pub fn handle(args: RestartArgs, _ctx: &CliContext) -> Result<(), CliError> {
    Err(CliError::not_implemented(format!(
        "restart {}",
        args.capability
    )))
}
