use clap::Parser;

use crate::context::CliContext;
use crate::response::CliError;

#[derive(Parser)]
pub struct DisableArgs {
    /// Capability to disable
    pub capability: String,
    /// Disable only the named sub-feature (capability stays enabled)
    #[arg(long, value_name = "NAME")]
    pub feature: Option<String>,
    /// Also remove installed files and config
    #[arg(long)]
    pub purge: bool,
}

pub fn handle(args: DisableArgs, _ctx: &CliContext) -> Result<(), CliError> {
    Err(CliError::not_implemented(format!(
        "disable {}",
        args.capability
    )))
}
