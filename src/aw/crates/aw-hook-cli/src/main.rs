//! Thin CLI wrapper around the reviewed hook composition library.

use aw_adapters::Host;
use aw_hook_cli::{
    inspect_with_cancellation, mark_returned, parse_payload, project_with_cancellation,
    read_projection_settings, read_settings, read_stdin, unavailable_response,
};
mod output;
mod signals;
use signals::{install_termination, Termination, CANCELLED};
use std::sync::{atomic::Ordering, Arc};

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments == ["--help"] {
        println!("Usage: aw-hook-cli <qoder|codex|qoder-project> /absolute/SETTINGS.json\nqoder/codex observe content. qoder-project explicitly returns recorded candidates; delivery does not prove adoption.");
        return;
    }
    let result = (|| {
        install_termination()?;
        if arguments.len() != 2 {
            return Err(aw_hook_cli::Error::Input);
        }
        if arguments[0] == "qoder-project" {
            let settings = read_projection_settings(std::path::Path::new(&arguments[1]))?;
            let payload = parse_payload(&read_stdin()?)?;
            let result = project_with_cancellation(settings, payload, Arc::new(Termination))?;
            return Ok((result.response, true, Some(result.record_path)));
        }
        let host = match arguments[0].as_str() {
            "qoder" => Host::Qoder,
            "codex" => Host::Codex,
            _ => return Err(aw_hook_cli::Error::Input),
        };
        let settings = read_settings(std::path::Path::new(&arguments[1]))?;
        let payload = parse_payload(&read_stdin()?)?;
        let result = inspect_with_cancellation(host, settings, payload, Arc::new(Termination))?;
        Ok((result.response, result.inspected, None))
    })();
    let (response, complete, record_path) = match result {
        Ok(value) if value.2.is_none() || !CANCELLED.load(Ordering::Relaxed) => value,
        _ => {
            let _ = output::response(&unavailable_response());
            eprintln!("aw-hook: inspection unavailable");
            std::process::exit(1);
        }
    };
    let delivery = (|| {
        output::response(&response)?;
        if let Some(path) = record_path {
            mark_returned(&path)?;
        }
        Ok::<_, aw_hook_cli::Error>(complete)
    })();
    match delivery {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("aw-hook: inspection unavailable");
            std::process::exit(1);
        }
        Err(_) => {
            // Once transmission starts, never append a second JSON response.
            // Failed writes or flushes leave the durable record unreturned.
            eprintln!("aw-hook: inspection unavailable");
            std::process::exit(1);
        }
    }
}
