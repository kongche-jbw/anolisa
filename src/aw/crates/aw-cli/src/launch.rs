//! Generate private native bindings while preserving each host's callback scheduler.
use crate::{
    ipc,
    model::{strings, Configuration, Hook, Request},
    server, Error,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(crate) fn write_private(path: &Path, value: &Value) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}
pub(crate) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn args(binary: &Path, socket: &Path, agent: &str, adapter: &str, hook: &Hook) -> Vec<String> {
    let mut args = vec![
        binary.to_string_lossy().into(),
        "hook".into(),
        "--socket".into(),
        socket.to_string_lossy().into(),
        "--agent".into(),
        agent.into(),
        "--event".into(),
        hook.event.clone(),
        "--provider".into(),
        hook.provider.clone(),
    ];
    if let Some(policy) = &hook.on_error {
        args.extend([
            "--adapter".into(),
            adapter.into(),
            "--on-error".into(),
            policy.clone(),
        ]);
    }
    args
}
fn object(value: &mut Value, key: &str) -> Result<(), Error> {
    if value.get(key).is_none() {
        value[key] = json!({});
    }
    if !value[key].is_object() {
        return Err(format!("native {key} must be an object").into());
    }
    Ok(())
}
fn append(value: &mut Value, key: &str, entry: Value) -> Result<(), Error> {
    if value.get(key).is_none() {
        value[key] = json!([]);
    }
    value[key]
        .as_array_mut()
        .ok_or("native hook list must be an array")?
        .push(entry);
    Ok(())
}
fn base(path: Option<&Path>) -> Result<Value, Error> {
    match path {
        Some(path) => {
            let mut bytes = Vec::new();
            fs::File::open(path)?
                .take((crate::model::MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() > crate::model::MAX_BYTES {
                return Err("native base config exceeds 4 MiB".into());
            }
            let result: Value = serde_yaml_ng::from_slice(&bytes)?;
            if !result.is_object() {
                return Err("native config must be an object".into());
            }
            Ok(result)
        }
        None => Ok(json!({})),
    }
}
fn contains_include(value: &Value) -> bool {
    match value {
        Value::Object(fields) => {
            fields.contains_key("$include") || fields.values().any(contains_include)
        }
        Value::Array(items) => items.iter().any(contains_include),
        _ => false,
    }
}
pub(crate) struct Prepared {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    registration: Option<Registration>,
    // Keep the selected native profile exclusive until its Agent group is gone.
    _profile_lock: Option<File>,
}

struct Registration {
    path: PathBuf,
    token: String,
    adapter: String,
    hooks: usize,
}

fn native_profile(path: &Path) -> Result<(PathBuf, File), Error> {
    use std::os::fd::AsRawFd;
    if !path.is_absolute() {
        return Err("--native-state-dir must be absolute".into());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let path = path.canonicalize()?;
    let metadata = fs::metadata(&path)?;
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::getuid() } {
        return Err("native state directory must belong to the current user".into());
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.join(".aw-launch.lock"))?;
    // The descriptor stays open for the full native process lifetime.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!(
            "native profile is already in use or cannot be locked: {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )
        .into());
    }
    Ok((path, lock))
}
pub(crate) fn prepare(
    config: &Configuration,
    agent: &str,
    socket: &Path,
    dir: &Path,
    native: Option<&Path>,
    native_state: Option<&Path>,
) -> Result<Prepared, Error> {
    let agent_config = config.agent(agent)?;
    let adapter = agent_config["adapter"].as_str().ok_or("missing adapter")?;
    let hooks = config.hooks(agent)?;
    let binary = std::env::current_exe()?;
    server::private_dir(dir)?;
    let mut argv = strings(&agent_config["argv"])?;
    let mut env = BTreeMap::from([
        ("AW_BIN".into(), binary.to_string_lossy().into()),
        ("AW_SOCKET".into(), socket.to_string_lossy().into()),
        ("AW_AGENT".into(), agent.into()),
    ]);
    let mut value = base(native)?;
    let registration = matches!(adapter, "openclaw" | "qwenpaw").then(|| Registration {
        path: dir.join("registration.json"),
        token: format!("{}:{}", config.revision, std::process::id()),
        adapter: adapter.into(),
        hooks: hooks.len(),
    });
    if let Some(receipt) = &registration {
        env.insert(
            "AW_READY_FILE".into(),
            receipt.path.to_string_lossy().into(),
        );
        env.insert("AW_READY_TOKEN".into(), receipt.token.clone());
    }
    let mut profile_lock = None;
    match adapter {
        "qoder" => {
            object(&mut value, "hooks")?;
            for hook in &hooks {
                let name = if hook.event == "tool.before" {
                    "PreToolUse"
                } else {
                    "PostToolUse"
                };
                let command = args(&binary, socket, agent, adapter, hook);
                let timeout = config.value["spec"]["providers"][&hook.provider]["timeout_ms"]
                    .as_u64()
                    .ok_or("missing timeout")?
                    .div_ceil(1000)
                    + 2;
                let mut entry = json!({"matcher":"*","hooks":[{"type":"command","command":command[0],"args":command[1..],"timeout":timeout}]});
                if let Some(sequential) = hook.sequential {
                    entry["sequential"] = json!(sequential);
                }
                append(&mut value["hooks"], name, entry)?;
            }
            let path = dir.join("qoder-settings.json");
            write_private(&path, &value)?;
            argv.extend(["--settings".into(), path.to_string_lossy().into()]);
        }
        "hermes" => {
            if std::env::var("HERMES_SAFE_MODE").ok().is_some_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            }) {
                return Err(
                    "HERMES_SAFE_MODE disables shell hooks; AW cannot install this binding".into(),
                );
            }
            if native.is_none() {
                return Err("Hermes requires --native-config for an explicit isolated model/base configuration".into());
            }
            let callback_seconds = value["plugins"]["hook_callback_timeout"]
                .as_f64()
                .unwrap_or(30.0);
            if !callback_seconds.is_finite() || callback_seconds < 0.0 {
                return Err("invalid Hermes hook_callback_timeout".into());
            }
            for hook in &hooks {
                let required = config.value["spec"]["providers"][&hook.provider]["timeout_ms"]
                    .as_u64()
                    .ok_or("missing timeout")?
                    .div_ceil(1000)
                    + 2;
                if callback_seconds > 0.0 && required as f64 > callback_seconds {
                    return Err("Hermes Provider plus cleanup budget exceeds native plugins.hook_callback_timeout".into());
                }
            }
            object(&mut value, "hooks")?;
            for hook in &hooks {
                let name = if hook.event == "tool.before" {
                    "pre_tool_call"
                } else {
                    "post_tool_call"
                };
                let command = args(&binary, socket, agent, adapter, hook)
                    .iter()
                    .map(|v| quote(v))
                    .collect::<Vec<_>>()
                    .join(" ");
                let timeout = config.value["spec"]["providers"][&hook.provider]["timeout_ms"]
                    .as_u64()
                    .ok_or("missing timeout")?
                    .div_ceil(1000)
                    + 2;
                let mut entry = json!({"command":command,"timeout":timeout.min(300)});
                if hook.on_error.as_deref() == Some("block") {
                    entry["fail_closed"] = json!(true);
                }
                append(&mut value["hooks"], name, entry)?;
            }
            write_private(&dir.join("config.yaml"), &value)?;
            env.insert("HERMES_HOME".into(), dir.to_string_lossy().into());
        }
        "openclaw" => {
            if contains_include(&value) {
                return Err("OpenClaw $include is relative to its original config; provide an expanded native JSON configuration".into());
            }
            if argv
                .windows(2)
                .any(|pair| pair[0] == "agent" && pair[1] == "exec")
            {
                return Err("OpenClaw 2026.9.6 agent exec omits hook-only plugins; use the verified Gateway path".into());
            }
            if native.is_none() {
                return Err("OpenClaw requires --native-config for an explicit isolated model/base configuration".into());
            }
            let plugin = dir.join("aw-openclaw");
            server::private_dir(&plugin)?;
            fs::write(
                plugin.join("index.mjs"),
                include_str!("../../../adapters/openclaw/index.mjs"),
            )?;
            fs::write(
                plugin.join("package.json"),
                include_str!("../../../adapters/openclaw/package.json"),
            )?;
            fs::write(
                plugin.join("openclaw.plugin.json"),
                include_str!("../../../adapters/openclaw/openclaw.plugin.json"),
            )?;
            object(&mut value, "plugins")?;
            if value["plugins"]["enabled"] == false
                || value["plugins"]["deny"].as_array().is_some_and(|deny| {
                    deny.iter()
                        .any(|id| id.as_str().is_some_and(|id| id.trim() == "aw-native-hooks"))
                })
            {
                return Err("OpenClaw native configuration disables the AW plugin".into());
            }
            object(&mut value["plugins"], "load")?;
            object(&mut value["plugins"], "entries")?;
            append(&mut value["plugins"]["load"], "paths", json!(plugin))?;
            if value["plugins"]["entries"].get("aw-native-hooks").is_some() {
                return Err(
                    "native config already owns aw-native-hooks; use a base without that entry"
                        .into(),
                );
            }
            if let Some(allow) = value["plugins"]
                .get_mut("allow")
                .and_then(Value::as_array_mut)
            {
                if !allow.is_empty() && !allow.iter().any(|id| id == "aw-native-hooks") {
                    allow.push(json!("aw-native-hooks"));
                }
            }
            let mut registrations = json!({"before":[],"after":[],"persist":[],"result":[]});
            for hook in &hooks {
                let group = if hook.event == "tool.before" {
                    "before"
                } else if hook.point.as_deref() == Some("agent_tool_result") {
                    "result"
                } else if hook.point.is_some() {
                    "persist"
                } else {
                    "after"
                };
                let mut handler = json!({"provider":hook.provider});
                if let Some(policy) = &hook.on_error {
                    handler["onError"] = json!(policy);
                }
                if group != "result" {
                    handler["priority"] = json!(hook.priority.unwrap_or(100));
                }
                append(&mut registrations, group, handler)?;
            }
            value["plugins"]["entries"]["aw-native-hooks"] = json!({"enabled":true,"config":{"binary":binary,"socket":socket,"agent":agent,"timeoutMs":14000,"hooks":registrations}});
            let path = dir.join("openclaw.json");
            write_private(&path, &value)?;
            env.insert("OPENCLAW_CONFIG_PATH".into(), path.to_string_lossy().into());
            if let Some(selected) = native_state {
                let (profile, lock) = native_profile(selected)?;
                if profile.starts_with(dir) {
                    return Err(
                        "native state cannot be inside the temporary AW launch directory".into(),
                    );
                }
                env.insert(
                    "OPENCLAW_STATE_DIR".into(),
                    profile.to_string_lossy().into(),
                );
                profile_lock = Some(lock);
            } else {
                env.insert("OPENCLAW_STATE_DIR".into(), dir.to_string_lossy().into());
                env.insert("OPENCLAW_HOME".into(), dir.to_string_lossy().into());
            }
        }
        "qwenpaw" => {
            if native.is_some() {
                return Err("QwenPaw uses its own working-directory initialization; --native-config is not supported".into());
            }
            let working = std::env::var_os("QWENPAW_WORKING_DIR")
                .map(PathBuf::from)
                .ok_or("set an isolated QWENPAW_WORKING_DIR initialized by QwenPaw")?;
            if !working.is_absolute() {
                return Err("QWENPAW_WORKING_DIR must be absolute".into());
            }
            let plugin = working.join("plugins/aw-native");
            if plugin.exists() {
                return Err("QwenPaw working directory already owns plugins/aw-native".into());
            }
            fs::create_dir_all(working.join("plugins"))?;
            fs::create_dir(&plugin)?;
            let installed = (|| -> Result<(), Error> {
                fs::set_permissions(&plugin, fs::Permissions::from_mode(0o700))?;
                fs::write(
                    plugin.join("plugin.json"),
                    include_str!("../../../adapters/qwenpaw/plugin.json"),
                )?;
                fs::write(
                    plugin.join("plugin.py"),
                    include_str!("../../../adapters/qwenpaw/plugin.py"),
                )?;
                let path = dir.join("qwenpaw-hooks.json");
                write_private(&path, &json!({"hooks":hooks}))?;
                env.insert("AW_NATIVE_CONFIG".into(), path.to_string_lossy().into());
                Ok(())
            })();
            if let Err(error) = installed {
                fs::remove_dir_all(&plugin)?;
                return Err(error);
            }
        }
        _ => return Err("unknown adapter".into()),
    }
    Ok(Prepared {
        argv,
        env,
        registration,
        _profile_lock: profile_lock,
    })
}
pub(crate) fn request(op: &str) -> Request {
    Request {
        op: op.into(),
        pid: 0,
        agent: String::new(),
        provider: String::new(),
        event: String::new(),
        cwd: String::new(),
        input: vec![],
        environment: BTreeMap::new(),
    }
}
pub(crate) fn ensure_daemon(
    config: &Configuration,
    path: &Path,
    socket: &Path,
) -> Result<(), Error> {
    if socket.exists() {
        let response = ipc::call(socket, &request("status"))?;
        if response.revision != config.revision {
            return Err("socket serves a different configuration revision".into());
        }
        return Ok(());
    }
    if config.value["spec"]["daemon"]["startup"] == "external" {
        return Err("external AW daemon is unavailable".into());
    }
    let parent = socket.parent().ok_or("socket has no parent")?;
    server::private_dir(parent)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(parent.join("daemon.log"))?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["serve", "--config"])
        .arg(path)
        .arg("--socket")
        .arg(socket)
        .args(["--idle-timeout", "300"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .process_group(0)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let exited = child.try_wait()?.is_some();
        if socket.exists() {
            if let Ok(response) = ipc::call_until(socket, &request("status"), deadline) {
                if response.revision == config.revision {
                    return Ok(());
                }
            }
        }
        if exited && !socket.exists() {
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        thread::sleep(Duration::from_millis(20));
    }
    if child.try_wait()?.is_none() {
        child.kill()?;
    }
    child.wait()?;
    Err("AW daemon readiness deadline exceeded".into())
}
static AGENT_GROUP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static AGENT_SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
extern "C" fn forward_signal(signal: i32) {
    AGENT_SIGNAL.store(signal, std::sync::atomic::Ordering::Relaxed);
    let group = AGENT_GROUP.load(std::sync::atomic::Ordering::Relaxed);
    if group > 0 {
        unsafe {
            libc::kill(-group, signal);
        }
    }
}

struct AgentSignals(Vec<(i32, libc::sigaction)>);
impl AgentSignals {
    fn install() -> Result<Self, Error> {
        AGENT_SIGNAL.store(0, std::sync::atomic::Ordering::Relaxed);
        let mut saved = Self(Vec::new());
        for signal in [libc::SIGINT, libc::SIGTERM] {
            // The handler only accesses lock-free atomics and async-signal-safe kill.
            let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
            action.sa_sigaction = forward_signal as *const () as usize;
            let mut previous = unsafe { std::mem::zeroed::<libc::sigaction>() };
            if unsafe { libc::sigaction(signal, &action, &mut previous) } < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            saved.0.push((signal, previous));
        }
        Ok(saved)
    }
}
impl Drop for AgentSignals {
    fn drop(&mut self) {
        AGENT_GROUP.store(0, std::sync::atomic::Ordering::Relaxed);
        for (signal, previous) in &self.0 {
            unsafe { libc::sigaction(*signal, previous, std::ptr::null_mut()) };
        }
    }
}

struct AgentTerminal(Option<i32>);
impl AgentTerminal {
    fn set_foreground(group: i32) -> Result<(), Error> {
        // Restoring AW's foreground group runs while AW is in the background.
        let previous = unsafe { libc::signal(libc::SIGTTOU, libc::SIG_IGN) };
        if previous == libc::SIG_ERR {
            return Err(std::io::Error::last_os_error().into());
        }
        let result = unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, group) };
        let error = std::io::Error::last_os_error();
        unsafe { libc::signal(libc::SIGTTOU, previous) };
        if result < 0 {
            return Err(error.into());
        }
        Ok(())
    }
    fn transfer(group: i32) -> Result<Self, Error> {
        if unsafe { libc::isatty(libc::STDIN_FILENO) } == 0 {
            return Ok(Self(None));
        }
        let previous = unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) };
        if previous < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // A background invocation must not take the terminal from another job.
        if previous != unsafe { libc::getpgrp() } {
            return Ok(Self(None));
        }
        Self::set_foreground(group)?;
        Ok(Self(Some(previous)))
    }
    fn restore(&mut self) -> Result<(), Error> {
        if let Some(previous) = self.0 {
            Self::set_foreground(previous)?;
            self.0 = None;
        }
        Ok(())
    }
}
impl Drop for AgentTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

struct AgentChild {
    child: std::process::Child,
    reaped: bool,
}
impl AgentChild {
    fn signal(&self, signal: i32) -> Result<(), Error> {
        // Keep the group leader unreaped until the final group signal: its PID
        // cannot then be reused by an unrelated process group during cleanup.
        if unsafe { libc::kill(-(self.child.id() as i32), signal) } < 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    fn exited(&self) -> Result<bool, Error> {
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        if unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } < 0
        {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(error.into());
        }
        Ok(unsafe { info.si_pid() } != 0)
    }
    fn wait_native(&self) -> Result<(), Error> {
        let mut deadline = None;
        while !self.exited()? {
            if AGENT_SIGNAL.load(std::sync::atomic::Ordering::Relaxed) != 0 {
                let deadline = deadline.get_or_insert(Instant::now() + Duration::from_secs(2));
                if Instant::now() >= *deadline {
                    self.signal(libc::SIGKILL)?;
                    return Ok(());
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
    fn cleanup(&mut self) -> Result<i32, Error> {
        self.signal(libc::SIGTERM)?;
        let grace = Instant::now() + Duration::from_secs(1);
        let deadline = grace + Duration::from_secs(1);
        let mut killed = false;
        loop {
            if Instant::now() >= deadline {
                return Err("Agent process-group cleanup deadline exceeded".into());
            }
            if self.exited()? && crate::process::group_stopped(self.child.id(), deadline)? {
                // Disable signal forwarding before reaping releases the reserved PID.
                AGENT_GROUP.store(0, std::sync::atomic::Ordering::Relaxed);
                let status = self.child.wait()?;
                self.reaped = true;
                return Ok(status
                    .code()
                    .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)));
            }
            if !killed && Instant::now() >= grace {
                self.signal(libc::SIGKILL)?;
                killed = true;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Registration {
    fn loaded(&self, group: u32) -> Result<bool, Error> {
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err("native registration receipt exceeds 4 KiB".into());
        }
        let receipt: Value = serde_json::from_slice(&bytes)?;
        let pid = receipt["pid"]
            .as_u64()
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
            .ok_or("native registration has no valid PID")?;
        if receipt["version"] != 1
            || receipt["adapter"] != self.adapter
            || receipt["token"] != self.token
            || receipt["hooks"] != self.hooks
            || unsafe { libc::getpgid(pid) } != group as i32
        {
            return Err("native registration receipt does not match this launch".into());
        }
        Ok(true)
    }

    fn wait(&self, child: &AgentChild) -> Result<(), Error> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if AGENT_SIGNAL.load(std::sync::atomic::Ordering::Relaxed) != 0 {
                return Err("native registration wait interrupted".into());
            }
            if child.exited()? {
                return Err(format!(
                    "{} exited before AW Hook registration was confirmed",
                    self.adapter
                )
                .into());
            }
            if self.loaded(child.child.id())? {
                eprintln!(
                    "AW: {} Hook registrations confirmed ({})",
                    self.adapter, self.hooks
                );
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "{} did not confirm AW Hook registration within 30 seconds",
                    self.adapter
                )
                .into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for AgentChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.signal(libc::SIGKILL);
            AGENT_GROUP.store(0, std::sync::atomic::Ordering::Relaxed);
            let _ = self.child.try_wait();
        }
    }
}

pub(crate) fn run(prepared: Prepared, extra: Vec<String>) -> Result<i32, Error> {
    let _signals = AgentSignals::install()?;
    let mut child = AgentChild {
        child: Command::new(&prepared.argv[0])
            .args(&prepared.argv[1..])
            .args(extra)
            .envs(&prepared.env)
            .process_group(0)
            .spawn()?,
        reaped: false,
    };
    AGENT_GROUP.store(
        child.child.id() as i32,
        std::sync::atomic::Ordering::Relaxed,
    );
    let mut terminal = AgentTerminal::transfer(child.child.id() as i32)?;
    if terminal.0.is_some() {
        // A child that read stdin before the handoff may have received SIGTTIN.
        child.signal(libc::SIGCONT)?;
    }
    let pending = AGENT_SIGNAL.load(std::sync::atomic::Ordering::Relaxed);
    if pending != 0 {
        child.signal(pending)?;
    }
    let waited = (|| {
        if let Some(registration) = &prepared.registration {
            registration.wait(&child)?;
        }
        child.wait_native()
    })();
    let cleaned = child.cleanup();
    terminal.restore()?;
    waited?;
    cleaned
}
