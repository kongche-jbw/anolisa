use clap::Parser;
use serde::Serialize;

use crate::context::CliContext;
use crate::response::{CliError, render_json};

#[derive(Parser)]
pub struct EnvArgs {
    /// Include all probe details
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Serialize)]
struct EnvPayload {
    facts: serde_json::Value,
}

pub fn handle(args: EnvArgs, ctx: &CliContext) -> Result<(), CliError> {
    let facts = anolisa_env::EnvFacts::placeholder();

    if ctx.json {
        // EnvFacts may not implement Serialize yet; fall back to Debug
        // repr so we still produce a valid JSON envelope for now.
        let dump = serde_json::Value::String(format!("{facts:#?}"));
        return render_json("env", EnvPayload { facts: dump });
    }

    let verbose = args.verbose || ctx.verbose;
    if verbose {
        println!("{:#?}", facts);
    } else {
        println!("Platform:    {:?}", facts.platform);
        println!("Kernel:      {}", facts.kernel.version);
        println!(
            "Distro:      {} {}",
            facts.distro.name, facts.distro.version
        );
        println!("Arch:        {:?}", facts.arch);
        println!(
            "Filesystem:  btrfs={}, overlayfs={}",
            facts.filesystem.btrfs_available, facts.filesystem.overlayfs_available
        );
        println!("Frameworks:  {} detected", facts.frameworks.len());
    }
    Ok(())
}
