//! Rebind a native Herdr pane to its own cosh owner without replaying user argv.

use super::herdr::{self, PaneBinding, SessionDescriptor};
use crate::shell_host::ShellHostConfig;
use serde::de::DeserializeOwned;
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStringExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

fn directory() -> Result<PathBuf, String> {
    let path =
        PathBuf::from(std::env::var_os("COSH_AW_HERDR_SESSION").ok_or("AW pane session missing")?);
    let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !path.is_absolute()
        || !meta.is_dir()
        || meta.uid() != unsafe { nix::libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        return Err("AW pane requires an owner-private session directory".into());
    }
    Ok(path)
}

pub(super) fn read<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > 1024 * 1024 || meta.uid() != unsafe { nix::libc::geteuid() }
    {
        return Err("invalid AW pane record".into());
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn context() -> Result<(PathBuf, SessionDescriptor, PaneBinding), String> {
    let directory = directory()?;
    let descriptor: SessionDescriptor = read(&directory.join("launch.json"))?;
    if descriptor.format != 1
        || herdr::process_identity(descriptor.launcher_pid)? != descriptor.launcher_start_ticks
        || herdr::process_identity(descriptor.server_pid)? != descriptor.server_start_ticks
        || std::env::var_os("HERDR_SOCKET_PATH").as_deref() != Some(descriptor.socket.as_os_str())
    {
        return Err("AW pane session binding changed".into());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !directory.join("binding.json").exists() {
        if Instant::now() >= deadline {
            return Err("AW pane binding timed out".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let pane_id = std::env::var("HERDR_PANE_ID").map_err(|_| "AW pane identity missing")?;
    let process = herdr::rpc(
        &descriptor.socket,
        "pane.process_info",
        serde_json::json!({"pane_id":pane_id}),
    )?;
    let shell_pid = process["process_info"]["shell_pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .ok_or("AW pane owner missing")?;
    let initial: PaneBinding = read(&directory.join("binding.json"))?;
    if pane_id == initial.pane_id && shell_pid != initial.shell_pid {
        return Err("AW initial pane owner already exited".into());
    }
    let info = herdr::rpc(
        &descriptor.socket,
        "pane.get",
        serde_json::json!({"pane_id":pane_id}),
    )?;
    if info["pane"]["workspace_id"].as_str() != Some(initial.workspace_id.as_str()) {
        return Err("AW pane belongs to another workspace".into());
    }
    let binding = PaneBinding {
        workspace_id: initial.workspace_id,
        pane_id,
        shell_pid,
    };
    Ok((directory, descriptor, binding))
}

pub(super) fn run() -> Result<i32, String> {
    let directory = directory()?;
    let result = run_owned();
    // Herdr may respawn a pane shell after its original owner exits. Only the
    // bound owner may publish completion; a respawn must not overwrite it.
    let owner = read::<PaneBinding>(&directory.join("binding.json"))
        .is_ok_and(|binding| binding.shell_pid == std::process::id());
    if let Err(error) = &result {
        if !owner {
            return result;
        }
        let temporary = directory.join("failed.tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        writeln!(file, "{}", serde_json::json!({"status":1,"error":error}))
            .map_err(|e| e.to_string())?;
        fs::rename(temporary, directory.join("finished.json")).map_err(|e| e.to_string())?;
    }
    result
}

fn run_owned() -> Result<i32, String> {
    let (directory, descriptor, binding) = context()?;
    let (parent, _) =
        aw_hook_cli::process_identity(std::process::id()).map_err(|e| e.to_string())?;
    if binding.shell_pid != std::process::id() || parent != descriptor.server_pid {
        return Err("AW pane is not the owned Herdr child".into());
    }
    // This entry runs before cosh starts any threads; restore only the variables
    // temporarily isolated for Herdr, preserving the native Agent's environment.
    for (name, value) in [
        ("XDG_CONFIG_HOME", descriptor.xdg_config_home),
        ("XDG_STATE_HOME", descriptor.xdg_state_home),
    ] {
        match value {
            Some(value) => std::env::set_var(name, OsString::from_vec(value)),
            None => std::env::remove_var(name),
        }
    }
    // Keep every inner runtime temporary under the session owner so abnormal
    // pane death cannot strand directories after its registered processes stop.
    std::env::set_var("TMPDIR", &directory);
    crate::runtime::terminal::install_terminal_recovery();
    let status = {
        let mut config = ShellHostConfig::new(
            format!("aw-pane-{}", std::process::id()),
            directory.join(format!("shell-{}", std::process::id())),
        );
        config.bound_interactive_transcript();
        config.native_mode = std::env::var("COSH_SHELL_ISOLATED").as_deref() != Ok("1");
        if !config.native_mode {
            if let Ok(prompt) = std::env::var("COSH_POC_PS1") {
                config.prompt = prompt;
            }
        }
        let _scope = super::configure(&mut config, &crate::runtime::cli_args::RawShellKind::Bash)?;
        // A pane only hosts the native terminal; it does not select another
        // model adapter, print a second cosh welcome screen or analyze commands.
        crate::shell_host::run_raw_interactive_bash(&config)
            .map_err(|e| e.to_string())?
            .exit_status
            .unwrap_or(1)
    };
    if !initial_owner(&directory, &binding)? {
        return Ok(status);
    }
    let temporary = directory.join("finished.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{}", serde_json::json!({"status":status})).map_err(|e| e.to_string())?;
    fs::rename(temporary, directory.join("finished.json")).map_err(|e| e.to_string())?;
    Ok(status)
}

pub(super) fn configure(
    config: &mut ShellHostConfig,
    root: &Path,
    helper: &Path,
) -> Result<(), String> {
    if std::env::var_os("COSH_AW_HERDR_SESSION").is_none() {
        return Ok(());
    }
    let (directory, _, binding) = context()?;
    if binding.shell_pid != std::process::id() {
        return Err("AW pane configuration requires its cosh owner".into());
    }
    herdr::workspace::register(&directory, &binding, root)?;
    if !initial_owner(&directory, &binding)? {
        return Ok(());
    }
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    config.aw_bootstrap_command = Some(format!(
        "trap 'exit 130' INT; {} --aw-pane-run {}; exit $?\n",
        quote(helper),
        quote(root)
    ));
    Ok(())
}

pub(super) fn arguments(root: &Path) -> Result<Vec<OsString>, String> {
    let (directory, descriptor, binding) = context()?;
    if !initial_owner(&directory, &binding)? {
        return Err("AW automatic launch belongs to the initial pane owner".into());
    }
    let prepared: serde_json::Value = read(&root.join("prepared.json"))?;
    if prepared["owner_pid"].as_u64() != Some(u64::from(binding.shell_pid)) {
        return Err("AW launch is not owned by this pane".into());
    }
    // Claim only after normal native admission has checked the shell ancestry.
    let args: Vec<_> = descriptor
        .args
        .into_iter()
        .map(OsString::from_vec)
        .collect();
    aw_hook_cli::interactive::check_launch(root, &args, &|| false).map_err(|e| e.to_string())?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join("claimed"))
        .map_err(|_| "AW pane launch was already claimed")?;
    Ok(args)
}

pub(super) fn attached(root: &Path) -> Result<bool, String> {
    if std::env::var_os("COSH_AW_HERDR_SESSION").is_none() {
        return Ok(false);
    }
    let (_, _, binding) = context()?;
    let prepared: serde_json::Value = read(&root.join("prepared.json"))?;
    if prepared["owner_pid"].as_u64() != Some(u64::from(binding.shell_pid)) {
        return Err("AW shell does not own this Herdr pane".into());
    }
    Ok(true)
}

fn initial_owner(directory: &Path, binding: &PaneBinding) -> Result<bool, String> {
    let initial: PaneBinding = read(&directory.join("binding.json"))?;
    Ok(binding.pane_id == initial.pane_id && binding.shell_pid == initial.shell_pid)
}
