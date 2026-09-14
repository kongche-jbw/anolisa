//! Process-owned termination state shared by the two native-process CLIs.

use aw_core::ports::Cancellation;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) static CANCELLED: AtomicBool = AtomicBool::new(false);

pub(crate) struct Termination;
impl Cancellation for Termination {
    fn is_cancelled(&self) -> bool {
        CANCELLED.load(Ordering::Relaxed)
    }
}

extern "C" fn terminate(_: libc::c_int) {
    // AtomicBool is lock-free; no allocation, I/O or library work in the handler.
    CANCELLED.store(true, Ordering::Relaxed);
}

pub(crate) fn install_termination() -> Result<(), aw_hook_cli::Error> {
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
