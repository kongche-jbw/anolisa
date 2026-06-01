use clap::Parser;
use serde::Serialize;

use crate::context::CliContext;
use crate::response::{CliError, render_json};

#[derive(Parser)]
pub struct ListArgs {
    /// Show only capabilities available on this machine
    #[arg(long)]
    pub available: bool,
    /// Show only currently enabled capabilities
    #[arg(long)]
    pub enabled: bool,
}

#[derive(Serialize)]
struct ListPayload<'a> {
    filter: &'a str,
    note: &'a str,
}

pub fn handle(args: ListArgs, ctx: &CliContext) -> Result<(), CliError> {
    let filter = match (args.available, args.enabled) {
        (true, _) => "available",
        (_, true) => "enabled",
        _ => "all",
    };
    let note = "Capability Resolver not yet wired";

    if ctx.json {
        return render_json("list", ListPayload { filter, note });
    }

    if !ctx.quiet {
        println!("CAPABILITY              STATUS       NOTE");
        println!("(filter: {filter}) — {note}");
    }
    Ok(())
}
