//! Experimental native-hook service and launcher; no cosh or Herdr dependency.
mod ipc;
mod launch;
mod mapping;
mod model;
mod process;
mod provider;
mod server;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};
type Error = Box<dyn std::error::Error + Send + Sync>;
const HELP:&str="aw native-hook lab (Linux)\n  aw validate --config FILE\n  aw plan AGENT --config FILE\n  aw check AGENT --config FILE\n  aw serve --config FILE --socket PATH [--idle-timeout SECONDS]\n  aw hook --socket PATH --agent ID --event tool.before|tool.after --provider ID\n  aw run AGENT --config FILE [--native-config FILE] [--state-dir DIR] [--socket PATH] [-- AGENT_ARGS...]\n  aw status|stop --socket PATH\nNative hook scheduling and ask decisions belong to each framework.\n";
fn execute() -> Result<i32, Error> {
    let mut args = std::env::args().skip(1);
    let operation = args.next().unwrap_or_else(|| "--help".into());
    if matches!(operation.as_str(), "--help" | "-h") {
        print!("{HELP}");
        return Ok(0);
    }
    if operation == "--version" {
        println!("aw native-hook-lab 0.1.0");
        return Ok(0);
    }
    let allowed = match operation.as_str() {
        "validate" | "plan" | "check" => vec!["--config"],
        "serve" => vec!["--config", "--socket", "--idle-timeout"],
        "hook" => vec![
            "--socket",
            "--agent",
            "--event",
            "--provider",
            "--adapter",
            "--on-error",
        ],
        "run" => vec!["--config", "--native-config", "--state-dir", "--socket"],
        "status" | "stop" => vec!["--socket"],
        _ => return Err("unknown command; use aw --help".into()),
    };
    let mut options = BTreeMap::new();
    let mut agent = None;
    let mut extra = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--" && operation == "run" {
            extra.extend(args);
            break;
        }
        if allowed.contains(&arg.as_str()) {
            let value = args.next().ok_or("option needs a value")?;
            if options.insert(arg, value).is_some() {
                return Err("duplicate CLI option".into());
            }
        } else if matches!(operation.as_str(), "run" | "plan" | "check")
            && !arg.starts_with('-')
            && agent.is_none()
        {
            agent = Some(arg);
        } else {
            return Err("unexpected CLI argument".into());
        }
    }
    let option = |name: &str| -> Result<&str, Error> {
        options
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("missing {name}").into())
    };
    if matches!(operation.as_str(), "status" | "stop" | "hook") {
        let fallback = match (options.get("--adapter"), options.get("--on-error")) {
            (Some(adapter), Some(policy)) => {
                Some(mapping::failure(adapter, option("--event")?, policy)?)
            }
            (None, None) => None,
            _ => return Err("--adapter and --on-error must be supplied together".into()),
        };
        let response = (|| -> Result<model::Response, Error> {
            let mut request = launch::request(if operation == "hook" {
                "invoke"
            } else {
                &operation
            });
            if operation == "hook" {
                request.agent = option("--agent")?.into();
                request.provider = option("--provider")?.into();
                request.event = option("--event")?.into();
                request.cwd = std::env::current_dir()?.to_string_lossy().into();
                request.environment = std::env::vars().collect();
                std::io::stdin()
                    .take((model::MAX_BYTES + 1) as u64)
                    .read_to_end(&mut request.input)?;
                if request.input.len() > model::MAX_BYTES {
                    return Err("native input exceeds 4 MiB".into());
                }
            }
            ipc::call(Path::new(option("--socket")?), &request)
        })();
        let response = match response {
            Ok(response) if fallback.is_none() || response.error.is_none() => response,
            result => {
                if let Some((code, stdout, stderr)) = fallback {
                    std::io::stdout().write_all(&stdout)?;
                    std::io::stderr().write_all(&stderr)?;
                    eprintln!("AW: structured callback failed");
                    return Ok(code);
                }
                result?
            }
        };
        std::io::stdout().write_all(&response.stdout)?;
        std::io::stderr().write_all(&response.stderr)?;
        if let Some(error) = response.error {
            eprintln!("AW: {error}");
        }
        return Ok(response.code);
    }
    let path = Path::new(option("--config")?).canonicalize()?;
    let config = model::Configuration::load(&path)?;
    if operation == "validate" {
        println!("Configuration is statically valid; native integration is not certified.");
        return Ok(0);
    }
    if operation == "serve" {
        server::serve(
            config,
            Path::new(option("--socket")?),
            options
                .get("--idle-timeout")
                .map(|n| n.parse())
                .transpose()?
                .unwrap_or(300),
        )?;
        return Ok(0);
    }
    let agent = agent.ok_or("missing Agent ID")?;
    let hooks = config.hooks(&agent)?;
    if config.agent(&agent)?["adapter"] == "qoder" {
        let native_argv = model::strings(&config.agent(&agent)?["argv"])?;
        if native_argv
            .iter()
            .chain(extra.iter())
            .any(|arg| arg == "--settings" || arg.starts_with("--settings="))
        {
            return Err("AW owns Qoder --settings; supply the base using --native-config".into());
        }
        if native_argv
            .iter()
            .chain(extra.iter())
            .any(|arg| arg == "--setting-sources" || arg.starts_with("--setting-sources="))
        {
            return Err(
                "Qoder --setting-sources excludes --settings hook bindings in 1.1.64; omit it"
                    .into(),
            );
        }
    }

    if config.agent(&agent)?["adapter"] == "openclaw" {
        let mut effective = model::strings(&config.agent(&agent)?["argv"])?;
        effective.extend(extra.iter().cloned());
        if effective
            .windows(2)
            .any(|pair| pair[0] == "agent" && pair[1] == "exec")
        {
            return Err("OpenClaw 2026.9.6 agent exec omits hook-only plugins; use the verified Gateway path".into());
        }
    }

    if config.agent(&agent)?["adapter"] == "qwenpaw" {
        let mut effective = model::strings(&config.agent(&agent)?["argv"])?;
        effective.extend(extra.iter().cloned());
        if effective.get(1).map(String::as_str) != Some("app") {
            return Err("QwenPaw 2.2.2b4 requires the app entrypoint; ACP/TUI do not load the AW Hook plugin".into());
        }
    }

    if operation == "plan" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"agent":agent,"adapter":config.agent(&agent)?["adapter"],"revision":config.revision,"hooks":hooks,"scheduling":"native","ask":"native-only; see framework and mode matrix"})
            )?
        );
        return Ok(0);
    }
    provider::check(&config, &agent)?;
    if operation == "check" {
        println!("Provider operations and private configuration admitted; native installation and effect adoption require runtime evidence.");
        return Ok(0);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or("XDG_RUNTIME_DIR or explicit --state-dir is required");
    let configured_state = config.value["spec"]["daemon"]["state_dir"]
        .as_str()
        .ok_or("missing state_dir")?;
    let state = if let Some(path) = options.get("--state-dir") {
        PathBuf::from(path)
    } else if configured_state != "auto" {
        PathBuf::from(configured_state)
    } else {
        runtime?.join(format!("aw-native-{}", &config.revision[..16]))
    };
    if !state.is_absolute() {
        return Err("state directory must be absolute".into());
    }
    server::private_dir(&state)?;
    let configured_endpoint = config.value["spec"]["daemon"]["endpoint"]
        .as_str()
        .ok_or("missing endpoint")?;
    let socket = options
        .get("--socket")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if configured_endpoint == "auto" {
                state.join("aw.sock")
            } else {
                PathBuf::from(
                    configured_endpoint
                        .strip_prefix("unix://")
                        .unwrap_or(configured_endpoint),
                )
            }
        });
    if !socket.is_absolute() {
        return Err("native daemon endpoint must be an absolute Unix socket path".into());
    }
    let binding = state.join(format!("launch-{}", std::process::id()));
    if binding.exists() {
        return Err("launch directory already exists".into());
    }
    let native = options.get("--native-config").map(Path::new);
    let prepared = match launch::prepare(&config, &agent, &socket, &binding, native) {
        Ok(prepared) => prepared,
        Err(error) => {
            if binding.exists() {
                std::fs::remove_dir_all(&binding)?;
            }
            return Err(error);
        }
    };
    let mut leased = false;
    let result = (|| {
        launch::ensure_daemon(&config, &path, &socket)?;
        let mut lease = launch::request("lease");
        lease.pid = std::process::id();
        let reply = ipc::call(&socket, &lease)?;
        if reply.code != 0 {
            return Err(reply.error.unwrap_or_else(|| "lease failed".into()).into());
        }
        leased = true;
        launch::run(prepared, extra)
    })();
    if leased {
        let mut release = launch::request("release");
        release.pid = std::process::id();
        if ipc::call(&socket, &release).is_err() {
            eprintln!("AW: daemon lease release failed; dead-owner cleanup remains active");
        }
    }
    // Only remove the exact generated launch directory, never a native user config.
    let cleanup = std::fs::remove_dir_all(&binding);
    if config.agent(&agent)?["adapter"] == "qwenpaw" {
        if let Some(working) = std::env::var_os("QWENPAW_WORKING_DIR") {
            std::fs::remove_dir_all(PathBuf::from(working).join("plugins/aw-native"))?;
        }
    }
    cleanup?;
    result
}
fn main() {
    match execute() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("AW: {error}");
            std::process::exit(125);
        }
    }
}
