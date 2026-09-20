//! Exec-only native attachment: no fork, PTY relay or second wait/reap owner.

use super::{evidence, storage, Binding, Error, Prepared, State};
use aw_contracts::canonical;
use serde_json::json;
use std::{
    ffi::OsString, io::IsTerminal, os::unix::process::CommandExt, path::Path, process::Command,
};

/// Replaces a foreground shim with the pinned native interactive Qoder process.
///
/// `args` are native arguments, never shell command text. Only the documented
/// foreground profile is admitted. The native shell retains terminal and reaping
/// ownership. On success this function does not return.
///
/// # Errors
/// Rejects detached/nested entry, unsupported options, changed pins or failed exec.
pub fn launch(root: &Path, args: &[OsString]) -> Result<(), Error> {
    launch_with_cancellation(root, args, &|| false)
}

/// Attaches a native process while allowing the owner to cancel startup probes.
///
/// # Errors
/// Returns the same profile and exec errors as [`launch`], or a cancellation error.
pub fn launch_with_cancellation(
    root: &Path,
    args: &[OsString],
    cancelled: &dyn Fn() -> bool,
) -> Result<(), Error> {
    if cancelled() {
        return Err(Error::Profile("native launch cancelled"));
    }
    let prepared: Prepared = storage::read(&root.join("prepared.json"))?;
    storage::descendant(prepared.owner_pid, prepared.owner_ticks)?;
    let agent_pid = std::process::id();
    let (shell_pid, agent_ticks) = crate::process_identity(agent_pid).map_err(evidence)?;
    if crate::process_identity(shell_pid).map_err(evidence)?.0 != prepared.owner_pid
        || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal()
        || !std::io::stderr().is_terminal()
        // These calls only query this process's terminal and process group.
        || unsafe { libc::tcgetpgrp(0) != libc::getpgrp() }
    {
        return Err(Error::Profile(
            "only a direct foreground launch in the owning shell is supported",
        ));
    }
    native_args(args)?;
    let config = &prepared.config;
    if std::env::current_dir()? != config.cwd()? {
        return Err(Error::Profile(
            "current directory differs from trusted configuration",
        ));
    }
    if std::env::var("QODER_CONFIG_DIR_NAME").is_ok_and(|name| name != ".qoder") {
        return Err(Error::Profile("unsupported project settings directory"));
    }
    super::config::check_agent(&config.qoder)?;
    let probe_config = aw_host_process::Config {
        provider_id: "qoder-version".into(),
        provider_version: "1.1.47".into(),
        program: config.qoder.program.clone(),
        program_sha256: config.qoder.program_sha256.clone(),
        cwd: config.cwd()?.to_path_buf(),
        args: vec![],
        environment: Default::default(),
        pins: vec![],
        limits: aw_host_process::Limits {
            timeout_ms: 5000,
            input_bytes: 1024,
            output_bytes: 1024,
            stderr_bytes: 1024,
        },
    };
    probe_config.validate().map_err(evidence)?;
    let probe = aw_host_process::run(
        &probe_config,
        &["--version"],
        b"",
        5000,
        &super::hooks::CancellationCheck(cancelled),
    )
    .map_err(evidence)?;
    if cancelled() {
        return Err(Error::Profile("native launch cancelled"));
    }
    if probe.exit_code != 0 || probe.stdout != b"1.1.47\n" {
        return Err(Error::Profile("Qoder 1.1.47 required"));
    }
    if std::fs::read_dir(root)?.take(133).count() >= 132 {
        return Err(Error::Profile(
            "shell launch capacity reached; open a new cosh session",
        ));
    }
    let run = root.join(format!("run-{agent_pid}-{agent_ticks}"));
    storage::directory(&run)?;
    let result = (|| {
        storage::directory(&run.join("calls"))?;
        if prepared.config.format == 2 {
            storage::directory(&run.join("notifications"))?;
        }
        storage::create(&run.join("state.lock"), &json!({}))?;
        storage::create(&run.join("state.json"), &State::default())?;
        let runtime_id = canonical::digest(run.as_os_str().as_encoded_bytes());
        let binding = Binding {
            prepared,
            runtime_id,
            agent_pid,
            agent_ticks,
        };

        let legacy_events = [
            "SessionStart",
            "SessionEnd",
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
        ];
        let events: &[&str] = if binding.prepared.config.format == 2 {
            &aw_adapters::qoder_events::NATIVE_EVENTS
        } else {
            &legacy_events
        };
        let hooks: serde_json::Map<String, serde_json::Value> = events
            .iter()
            .copied()
            .map(|event| {
                let entry = if event == "PreToolUse" && binding.prepared.config.tool_guard.is_some()
                {
                    "--aw-guard"
                } else if event == "UserPromptSubmit"
                    && binding.prepared.config.input_response.is_some()
                {
                    "--aw-input"
                } else if event == "Stop" && binding.prepared.config.stop_response.is_some() {
                    "--aw-stop"
                } else {
                    "--aw-hook"
                };
                let hook = format!(
                    "{} {} {}",
                    quote(&binding.prepared.helper.to_string_lossy()),
                    entry,
                    quote(&run.to_string_lossy())
                );
                (
                    event.into(),
                    json!([{
                        "matcher":"*", "hooks":[{"type":"command","command":hook,"timeout":5}]
                    }]),
                )
            })
            .collect();
        storage::create(&run.join("native.json"), &json!({"hooks":hooks}))?;
        storage::create(&run.join("binding.json"), &binding)?;
        super::config::check_agent(&binding.prepared.config.qoder)?;
        if cancelled() {
            return Err(Error::Profile("native launch cancelled"));
        }
        if super::runtime_events::configured(&binding.prepared.config) {
            // The owner opens the pidfd before exec, so even an immediate exit
            // is observable without stealing Bash's wait/reap ownership.
            storage::create(&run.join("owner-request.json"), &json!({}))?;
            super::runtime_events::await_registration(&run, cancelled)?;
        }
        // Native configuration and login state stay in place. Additional settings
        // use Qoder's merge mechanism; AW never rewrites existing hooks/plugins.
        let error = Command::new(&binding.prepared.config.qoder.program)
            .arg("--config-dir")
            .arg(&binding.prepared.config.native_config_directory)
            .arg("--settings")
            .arg(run.join("native.json"))
            .args(args)
            .exec();
        Err(Error::Io(error))
    })();
    // A published request may be concurrently opening a pidfd. Leave it to the
    // owner even on cancellation/exec failure; registration never proves ready.
    if !run.join("owner-request.json").try_exists()? {
        std::fs::remove_dir_all(&run)?;
    }
    result
}

fn native_args(args: &[OsString]) -> Result<(), Error> {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index]
            .to_str()
            .ok_or(Error::Profile("UTF-8 native arguments required"))?;
        match arg {
            "--model" | "-m" | "--resume" | "-r" | "--name" | "-n" => {
                index += 1;
                if args
                    .get(index)
                    .and_then(|value| value.to_str())
                    .is_none_or(|value| value.is_empty() || value.starts_with('-'))
                {
                    return Err(Error::Profile("native option requires an explicit value"));
                }
            }
            "--" => return Ok(()),
            _ if !arg.starts_with('-') => {}
            _ => {
                return Err(Error::Profile(
                    "native option is outside the interactive profile",
                ))
            }
        }
        index += 1;
    }
    Ok(())
}

pub(super) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
