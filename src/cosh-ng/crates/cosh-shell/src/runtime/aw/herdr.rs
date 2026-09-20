//! Own the optional native Herdr session around a foreground Agent invocation.

mod process;
mod protocol;

pub(super) use process::process_identity;
use process::{OwnedSession, TerminalRestore};
pub(super) use protocol::rpc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::{IsTerminal, Read},
    os::unix::{ffi::OsStrExt, fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionDescriptor {
    pub format: u32,
    pub args: Vec<Vec<u8>>,
    pub cwd: PathBuf,
    pub socket: PathBuf,
    pub launcher_pid: u32,
    pub launcher_start_ticks: u64,
    pub server_pid: u32,
    pub server_start_ticks: u64,
    pub xdg_config_home: Option<Vec<u8>>,
    pub xdg_state_home: Option<Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PaneBinding {
    pub pane_id: String,
    pub shell_pid: u32,
}

pub(super) fn try_launch(
    root: &Path,
    args: &[OsString],
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<i32>, String> {
    let Some(binary) = std::env::var_os("COSH_AW_HERDR") else {
        return Ok(None);
    };
    aw_hook_cli::interactive::check_launch(root, args, cancelled).map_err(|e| e.to_string())?;
    let binary = PathBuf::from(binary);
    verify_binary(&binary)?;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("AW Herdr launch requires an interactive terminal".into());
    }
    let _terminal = TerminalRestore::capture()?;
    let directory = tempfile::Builder::new()
        .prefix("cosh-herdr-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .map_err(|e| e.to_string())?;
    let temporary = directory.path();
    let socket = temporary.join("api.sock");
    if socket.as_os_str().as_bytes().len() >= 108 {
        return Err("Herdr private socket path exceeds the Unix socket limit".into());
    }
    let config = temporary.join("config.toml");
    let helper = std::env::current_exe().map_err(|e| e.to_string())?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    fs::write(&config, configuration(&helper)?).map_err(|e| e.to_string())?;
    let log = fs::File::create(temporary.join("herdr.log")).map_err(|e| e.to_string())?;
    let mut server_command = command(&binary, temporary, &cwd);
    server_command
        .arg("server")
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    // The server owns a separate session; the client keeps the foreground TTY.
    unsafe {
        server_command.pre_exec(|| {
            if nix::libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let server = server_command
        .spawn()
        .map_err(|e| format!("start Herdr server: {e}"))?;
    let mut owned = OwnedSession::new(server, socket.clone());
    let outcome = run(&mut owned, temporary, &binary, &cwd, args, cancelled);
    let cleanup = owned.close();
    match (outcome, cleanup) {
        (Ok(status), Ok(())) => Ok(Some(status)),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup: {cleanup}")),
    }
}

fn run(
    owned: &mut OwnedSession,
    temporary: &Path,
    binary: &Path,
    cwd: &Path,
    args: &[OsString],
    cancelled: &dyn Fn() -> bool,
) -> Result<i32, String> {
    let descriptor = SessionDescriptor {
        format: 1,
        args: args.iter().map(|arg| arg.as_bytes().to_vec()).collect(),
        cwd: cwd.into(),
        socket: owned.socket.clone(),
        launcher_pid: std::process::id(),
        launcher_start_ticks: process_identity(std::process::id())?,
        server_pid: owned.server.id(),
        server_start_ticks: process_identity(owned.server.id())?,
        xdg_config_home: std::env::var_os("XDG_CONFIG_HOME").map(|v| v.as_bytes().to_vec()),
        xdg_state_home: std::env::var_os("XDG_STATE_HOME").map(|v| v.as_bytes().to_vec()),
    };
    write_json(&temporary.join("launch.json"), &descriptor)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if cancelled() {
            return Err("AW Herdr launch cancelled".into());
        }
        owned.check_server()?;
        if rpc(&owned.socket, "ping", json!({})).is_ok() {
            break;
        }
        if Instant::now() >= deadline {
            return Err("Herdr server readiness timed out after 10 seconds".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
    let created = rpc(
        &owned.socket,
        "workspace.create",
        json!({"label":"AW Qoder", "cwd":cwd,"focus":true}),
    )?;
    owned.workspace = Some(
        created["workspace"]["workspace_id"]
            .as_str()
            .ok_or("Herdr workspace identity missing")?
            .to_owned(),
    );
    let pane = created["root_pane"]["pane_id"]
        .as_str()
        .ok_or("Herdr root pane identity missing")?;
    let pid = loop {
        let info = rpc(&owned.socket, "pane.process_info", json!({"pane_id":pane}))?;
        if let Some(pid) = info["process_info"]["shell_pid"]
            .as_u64()
            .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64)
        {
            break pid as u32;
        }
        if cancelled() || Instant::now() >= deadline {
            return Err("Herdr pane process readiness timed out or cancelled".into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    owned.register_pane(pid)?;
    write_json(
        &temporary.join("binding.json"),
        &PaneBinding {
            pane_id: pane.into(),
            shell_pid: pid,
        },
    )?;
    owned.client = Some(
        command(binary, temporary, cwd)
            .spawn()
            .map_err(|e| format!("start Herdr client: {e}"))?,
    );
    let deadline = Instant::now() + Duration::from_secs(86_400);
    let mut multiple_panes = false;
    loop {
        if cancelled() {
            return Err("AW Herdr session cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("AW Herdr session reached its 24-hour limit".into());
        }
        owned.check_server()?;
        if owned.client_exited()? {
            return completion_status(temporary)?
                .ok_or_else(|| "Herdr client detached before Agent completion".into());
        }
        owned.observe_descendants()?;
        let list = rpc(&owned.socket, "pane.list", json!({}))?;
        let panes = list["panes"].as_array().ok_or("Herdr pane list missing")?;
        multiple_panes |= panes.len() > 1;
        let first_present = panes
            .iter()
            .any(|entry| entry["pane_id"].as_str() == Some(pane));
        if !multiple_panes
            && (temporary.join("finished.json").exists()
                || !first_present
                || process_identity(pid).is_err())
        {
            if !temporary.join("finished.json").exists() {
                return Err("AW Herdr pane exited before reporting completion".into());
            }
            return completion_status(temporary)?
                .ok_or_else(|| "AW pane completion status missing".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn completion_status(temporary: &Path) -> Result<Option<i32>, String> {
    let path = temporary.join("finished.json");
    if !path.exists() {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if let Some(error) = value["error"].as_str() {
        return Err(format!("AW pane startup: {error}"));
    }
    let status = value["status"]
        .as_i64()
        .filter(|code| (0..=255).contains(code))
        .ok_or("invalid AW pane completion status")?;
    Ok(Some(status as i32))
}

fn command(binary: &Path, temporary: &Path, cwd: &Path) -> Command {
    let mut command = Command::new(binary);
    command.current_dir(cwd);
    // Each pane starts a non-login shell; an outer login marker must not replay
    // the account's login profiles inside that new Agent session.
    command.env_remove("COSH_LOGIN_SHELL");
    for (name, _) in std::env::vars_os() {
        if name.as_bytes().starts_with(b"HERDR_")
            || name.as_bytes().starts_with(b"COSH_AW_")
                && name != "COSH_AW_CONFIG"
                && name != "COSH_AW_CONFIG_SHA256"
                && name != "COSH_AW_HERDR"
                && name != "COSH_AW_HERDR_SHA256"
        {
            command.env_remove(name);
        }
    }
    command
        .env("COSH_AW_HERDR_SESSION", temporary)
        .env("HERDR_SOCKET_PATH", temporary.join("api.sock"))
        .env("HERDR_CONFIG_PATH", temporary.join("config.toml"))
        .env("XDG_CONFIG_HOME", temporary.join("config"))
        .env("XDG_STATE_HOME", temporary.join("state"));
    command
}

fn configuration(helper: &Path) -> Result<String, String> {
    let helper = helper
        .to_str()
        .ok_or("cosh executable path must be UTF-8")?;
    let mut config: toml::Value = toml::from_str(include_str!(
        "../../../../../../aw/integrations/herdr/interactive.toml"
    ))
    .map_err(|e| e.to_string())?;
    let table = config
        .as_table_mut()
        .ok_or("invalid embedded Herdr configuration")?;
    table.insert("onboarding".into(), toml::Value::Boolean(false));
    table.insert(
        "terminal".into(),
        toml::Value::try_from(json!({"default_shell":helper,"shell_mode":"non_login"}))
            .map_err(|e| e.to_string())?,
    );
    table.insert(
        "update".into(),
        toml::Value::try_from(json!({"version_check":false,"manifest_check":false}))
            .map_err(|e| e.to_string())?,
    );
    let ui = table
        .get_mut("ui")
        .and_then(toml::Value::as_table_mut)
        .ok_or("embedded Herdr UI configuration missing")?;
    ui.insert(
        "sidebar_start_collapsed".into(),
        toml::Value::Boolean(false),
    );
    ui.insert("sidebar_width".into(), toml::Value::Integer(48));
    // Herdr v0.9.0 otherwise clamps the requested width to its 36-column default.
    ui.insert("sidebar_max_width".into(), toml::Value::Integer(48));
    ui.insert("sidebar_collapsed_mode".into(), "hidden".into());
    toml::to_string(&config).map_err(|e| e.to_string())
}

fn verify_binary(binary: &Path) -> Result<(), String> {
    if !cfg!(target_os = "linux") || !binary.is_absolute() {
        return Err("AW Herdr requires an absolute Linux binary path".into());
    }
    let manifest: Value = serde_json::from_str(include_str!(
        "../../../../../../aw/integrations/herdr/upstream.json"
    ))
    .map_err(|e| e.to_string())?;
    let expected = manifest["assets"][std::env::consts::ARCH]["sha256"]
        .as_str()
        .ok_or("Herdr has no pinned binary for this architecture")?;
    let supplied =
        std::env::var("COSH_AW_HERDR_SHA256").map_err(|_| "Herdr trust digest missing")?;
    if supplied != expected {
        return Err("Herdr digest does not match the pinned v0.9.0 release".into());
    }
    let mut file = fs::File::open(binary).map_err(|e| format!("open Herdr: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 * 1024 {
        return Err("Herdr binary must be a regular file no larger than 256 MiB".into());
    }
    let mut hasher = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let count = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&bytes[..count]);
    }
    if format!("{:x}", hasher.finalize()) != expected {
        return Err("Herdr binary digest mismatch".into());
    }
    Ok(())
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    fs::write(
        &temporary,
        serde_json::to_vec(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_configuration_preserves_metadata_rows_and_quotes_path() {
        let helper = Path::new("/tmp/cosh shell\"binary");
        let config: toml::Value = toml::from_str(&configuration(helper).unwrap()).unwrap();
        assert_eq!(
            config["terminal"]["default_shell"].as_str(),
            helper.to_str()
        );
        assert_eq!(config["terminal"]["shell_mode"].as_str(), Some("non_login"));
        assert_eq!(config["update"]["version_check"].as_bool(), Some(false));
        assert_eq!(config["ui"]["sidebar_width"].as_integer(), Some(48));
        assert_eq!(config["ui"]["sidebar_max_width"].as_integer(), Some(48));
        assert_eq!(
            config["ui"]["sidebar_collapsed_mode"].as_str(),
            Some("hidden")
        );
        let rows = config["ui"]["sidebar"]["agents"]["rows"]
            .as_array()
            .unwrap();
        assert!(rows.iter().any(|row| row
            .as_array()
            .unwrap()
            .iter()
            .any(
                |cell| cell.get("token").and_then(toml::Value::as_str) == Some("$aw_observation")
            )));
        let workspace_rows = config["ui"]["sidebar"]["spaces"]["rows"]
            .as_array()
            .unwrap();
        assert_eq!(&workspace_rows[1..], &rows[1..]);
    }
}
