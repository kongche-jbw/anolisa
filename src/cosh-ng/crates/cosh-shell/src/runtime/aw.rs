//! Opt-in AW product composition; native shells retain PTY and process ownership.

use crate::{runtime::cli_args::RawShellKind, shell_host::ShellHostConfig};

pub(crate) fn dispatch() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().collect();
    let shim = args.first().is_some_and(|arg| {
        std::path::Path::new(arg)
            .file_name()
            .is_some_and(|name| name == "qoder")
    });
    let internal = args.get(1).and_then(|arg| arg.to_str());
    if !shim
        && !matches!(
            internal,
            Some("--aw-hook" | "--aw-guard" | "--aw-input" | "--aw-stop" | "--aw-query")
        )
    {
        return None;
    }
    #[cfg(feature = "aw")]
    let result = dispatch_enabled(&args, shim);
    #[cfg(not(feature = "aw"))]
    let result: Result<(), String> = Err("build cosh-shell with the aw feature".into());
    match result {
        Ok(()) => Some(0),
        Err(error) => {
            eprintln!("{error}");
            if matches!(internal, Some("--aw-guard" | "--aw-input")) {
                // Qoder treats exit 2 as blocking; optional hook errors cannot
                // be reused for an explicitly selected tool safety callback.
                Some(2)
            } else if internal == Some("--aw-stop") {
                // Stop exit 2 asks the model to continue; errors must not do so.
                println!("{{\"continue\":false,\"stopReason\":\"AW stop check unavailable.\",\"systemMessage\":\"AW stop check unavailable; check not passed.\"}}");
                Some(0)
            } else if internal == Some("--aw-hook") {
                println!("{{\"systemMessage\":\"AW optional observation unavailable; native behavior unchanged.\"}}");
                Some(0)
            } else {
                Some(1)
            }
        }
    }
}

#[cfg(feature = "aw")]
fn dispatch_enabled(args: &[std::ffi::OsString], shim: bool) -> Result<(), String> {
    use aw_hook_cli::interactive;
    use std::{io::Write, path::Path};
    if shim {
        let root = std::env::var_os("COSH_AW_ROOT").ok_or("AW shell binding missing")?;
        let termination = HookTermination::new()?;
        crate::shell_host::sigpipe::restore_in_child().map_err(|error| error.to_string())?;
        return interactive::launch_with_cancellation(Path::new(&root), &args[1..], &|| {
            termination
                .cancelled
                .load(std::sync::atomic::Ordering::Relaxed)
        })
        .map_err(|error| error.to_string());
    }
    if args.len() != 3 {
        return Err("AW helper requires one binding directory".into());
    }
    let root = Path::new(&args[2]);
    let value = if args[1] == "--aw-query" {
        interactive::query(root)
    } else {
        let termination = HookTermination::new()?;
        let bytes = aw_hook_cli::read_stdin().map_err(|error| error.to_string())?;
        let value = aw_hook_cli::parse_payload(&bytes).map_err(|error| error.to_string())?;
        let cancelled = || {
            termination
                .cancelled
                .load(std::sync::atomic::Ordering::Relaxed)
        };
        if args[1] == "--aw-guard" {
            interactive::guard_callback_with_cancellation(root, value, &cancelled)
        } else if args[1] == "--aw-input" {
            interactive::input_callback_with_cancellation(root, value, &cancelled)
        } else if args[1] == "--aw-stop" {
            interactive::stop_callback_with_cancellation(root, value, &cancelled)
        } else {
            interactive::callback_with_cancellation(root, value, &cancelled)
        }
    }
    .map_err(|error| error.to_string())?;
    let mut output = std::io::stdout().lock();
    writeln!(output, "{value}").map_err(|error| error.to_string())
}

#[cfg(feature = "aw")]
struct HookTermination {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}

#[cfg(feature = "aw")]
impl HookTermination {
    fn new() -> Result<Self, String> {
        let mut guard = Self {
            cancelled: Default::default(),
            registrations: vec![],
        };
        for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
            guard.registrations.push(
                signal_hook::flag::register(signal, guard.cancelled.clone())
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(guard)
    }
}

#[cfg(feature = "aw")]
impl Drop for HookTermination {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
}

pub(crate) fn configure(
    config: &mut ShellHostConfig,
    kind: &RawShellKind,
) -> Result<Option<AwScope>, String> {
    let Some(path) = std::env::var_os("COSH_AW_CONFIG") else {
        return Ok(None);
    };
    #[cfg(not(feature = "aw"))]
    {
        let _ = (config, kind, path);
        Err("AW requested but this cosh-shell was built without the aw feature".into())
    }
    #[cfg(feature = "aw")]
    {
        use std::path::Path;
        if !matches!(kind, RawShellKind::Bash) || !config.integration.uses_markers() {
            return Err(
                "AW interactive profile requires Bash with enhanced shell integration".into(),
            );
        }
        let digest =
            std::env::var("COSH_AW_CONFIG_SHA256").map_err(|_| "AW config trust digest missing")?;
        let directory = tempfile::Builder::new()
            .prefix("cosh-aw-")
            .tempdir()
            .map_err(|error| error.to_string())?;
        let root = directory.path().join("scope");
        let helper = std::env::current_exe().map_err(|error| error.to_string())?;
        aw_hook_cli::interactive::prepare(Path::new(&path), &digest, &root, &helper)
            .map_err(|error| error.to_string())?;
        let observer = aw_hook_cli::interactive::RuntimeObserver::start(root.clone())
            .map_err(|error| error.to_string())?;
        config.aw_shim_directory = Some(root.join("bin"));
        config
            .env_overrides
            .push(("COSH_AW_ROOT".into(), root.to_string_lossy().into_owned()));
        let viewer = match (
            std::env::var_os("HERDR_SOCKET_PATH"),
            std::env::var("HERDR_PANE_ID"),
        ) {
            (Some(socket), Ok(pane)) => {
                match aw_hook_cli::interactive::HerdrBridge::start(root, socket.into(), pane) {
                    Ok(viewer) => Some(viewer),
                    Err(error) => {
                        eprintln!("AW optional Herdr view unavailable: {error}");
                        None
                    }
                }
            }
            _ => None,
        };
        Ok(Some(AwScope {
            _viewer: viewer,
            _observer: observer,
            _directory: directory,
        }))
    }
}

pub(crate) struct AwScope {
    // Field drop order joins the viewer before removing its evidence directory.
    #[cfg(feature = "aw")]
    _viewer: Option<aw_hook_cli::interactive::HerdrBridge>,
    #[cfg(feature = "aw")]
    _observer: Option<aw_hook_cli::interactive::RuntimeObserver>,
    #[cfg(feature = "aw")]
    _directory: tempfile::TempDir,
}
