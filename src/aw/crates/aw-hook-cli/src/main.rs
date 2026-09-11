//! Thin CLI wrapper around the reviewed hook composition library.

use aw_adapters::Host;
use aw_core::ports::Cancellation;
use aw_hook_cli::{
    inspect_with_cancellation, parse_payload, read_settings, read_stdin, unavailable_response,
};
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
        println!("Usage: aw-hook-cli <qoder|codex> /absolute/SETTINGS.json\nOpt-in PostToolUse content observation. No enforcement or result replacement.");
        return;
    }
    let result = (|| {
        install_termination()?;
        if arguments.len() != 2 {
            return Err(aw_hook_cli::Error::Input);
        }
        let host = match arguments[0].as_str() {
            "qoder" => Host::Qoder,
            "codex" => Host::Codex,
            _ => return Err(aw_hook_cli::Error::Input),
        };
        let settings = read_settings(std::path::Path::new(&arguments[1]))?;
        let payload = parse_payload(&read_stdin()?)?;
        inspect_with_cancellation(host, settings, payload, Arc::new(Termination))
    })();
    match result {
        Ok(result) => {
            println!("{}", result.response);
            if !result.inspected {
                eprintln!("aw-hook: inspection unavailable");
                std::process::exit(1);
            }
        }
        Err(_) => {
            println!("{}", unavailable_response());
            eprintln!("aw-hook: inspection unavailable");
            std::process::exit(1);
        }
    }
}
