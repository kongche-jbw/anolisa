//! Thin CLI wrapper around the reviewed hook composition library.

use aw_adapters::Host;
use aw_core::ports::Cancellation;
use aw_hook_cli::{
    inspect_with_cancellation, mark_returned, parse_payload, project_with_cancellation,
    read_projection_settings, read_settings, read_stdin, unavailable_response,
};
mod output;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

static CANCELLED: AtomicBool = AtomicBool::new(false);

struct Termination;
impl Cancellation for Termination {
    fn is_cancelled(&self) -> bool {
        CANCELLED.load(Ordering::Relaxed)
    }
}

extern "C" fn terminate(_: libc::c_int) {
    // AtomicBool is lock-free; no allocation, I/O or library work in the handler.
    CANCELLED.store(true, Ordering::Relaxed);
}

fn install_termination() -> Result<(), aw_hook_cli::Error> {
    // The CLI owns its signal dispositions. Embedding the library installs none.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = terminate as *const () as usize;
    // All fields are initialized; the handler has the required C ABI.
    if unsafe { libc::sigemptyset(&mut action.sa_mask) } != 0 {
        return Err(aw_hook_cli::Error::Input);
    }
    for signal in [libc::SIGTERM, libc::SIGINT] {
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0 {
            return Err(aw_hook_cli::Error::Input);
        }
    }
    Ok(())
}

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
