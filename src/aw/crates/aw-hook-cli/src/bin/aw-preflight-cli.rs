//! Explicit dependency preflight; all child ownership stays with native Hosts.

use aw_hook_cli::preflight;
use std::{path::Path, sync::Arc};

#[path = "../output.rs"]
mod output;
#[path = "../signals.rs"]
mod signals;
use signals::CANCELLED;

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments == ["--help"] {
        println!("Usage: aw-preflight-cli /absolute/SETTINGS.json\nProbe explicit native dependencies for inspect or project; does not start an Agent or install hooks.");
        return;
    }
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        signals::install_termination()?;
        let [path] = arguments.as_slice() else {
            return Err(preflight::Error::Settings.into());
        };
        let settings = preflight::read_settings(Path::new(path))?;
        let value = preflight::run(settings, Arc::new(signals::Termination))?;
        output::response(&value).map_err(|_| "preflight output unavailable or cancelled")?;
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("aw-preflight: {error}");
        std::process::exit(1);
    }
}
