//! Explicit installation-time admission; normal launches never refresh trust pins.

mod native;

use aw_contracts::canonical;
use aw_core::ports::Cancellation;
use aw_host_process::{Config, FilePin, Limits, PinState};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

pub(crate) const HELP: &str = "Usage: aw-hook-cli configure --qoder ABS --native-config ABS_DIR --sec-core ABS --tokenless ABS --herdr ABS --workspace ABS_DIR --output ABS_NEW_FILE [--providers ABS_FILE] [--sec-core-pin ABS_FILE ...]\nCreates a private native Qoder profile from explicitly selected installed components. Prints a user [aw] configuration; never changes user configuration or downloads components.";
const DEFAULT_PROVIDERS: &[u8] = include_bytes!("../../../providers/qoder-native.json");

struct Options {
    qoder: PathBuf,
    native_config: PathBuf,
    sec_core: PathBuf,
    tokenless: PathBuf,
    herdr: PathBuf,
    workspace: PathBuf,
    output: PathBuf,
    providers: Option<PathBuf>,
    sec_core_pins: Vec<PathBuf>,
}

impl Options {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut values = BTreeMap::new();
        let mut pins = Vec::new();
        for pair in arguments.chunks(2) {
            if pair.len() != 2 {
                return Err("configure requires a path after every option".into());
            }
            if !matches!(
                pair[0].as_str(),
                "--qoder"
                    | "--native-config"
                    | "--sec-core"
                    | "--tokenless"
                    | "--herdr"
                    | "--workspace"
                    | "--output"
                    | "--providers"
                    | "--sec-core-pin"
            ) {
                return Err(format!("unknown configure option: {}", pair[0]));
            }
            let path = PathBuf::from(&pair[1]);
            if !path.is_absolute() || pair[1].chars().any(char::is_control) {
                return Err(format!("{} requires an absolute path", pair[0]));
            }
            if pair[0] == "--sec-core-pin" {
                if pins.len() >= 32 || pins.contains(&path) {
                    return Err("too many or duplicate explicit SecCore pins".into());
                }
                pins.push(path);
            } else if values.insert(pair[0].as_str(), path).is_some() {
                return Err(format!("duplicate configure option: {}", pair[0]));
            }
        }
        let mut required = |key: &str| {
            values
                .remove(key)
                .ok_or_else(|| format!("missing configure option {key}"))
        };
        Ok(Self {
            qoder: required("--qoder")?,
            native_config: required("--native-config")?,
            sec_core: required("--sec-core")?,
            tokenless: required("--tokenless")?,
            herdr: required("--herdr")?,
            workspace: required("--workspace")?,
            output: required("--output")?,
            providers: values.remove("--providers"),
            sec_core_pins: pins,
        })
    }

    fn resolve_workspace(&mut self) -> Result<(), String> {
        // Native current_dir reports the physical directory, including when the
        // operator entered it through a symlink or parent-directory component.
        self.workspace = fs::canonicalize(&self.workspace)
            .map_err(|error| format!("resolve configured workspace: {error}"))?;
        Ok(())
    }
}

pub(crate) fn run(arguments: &[String], cancellation: &dyn Cancellation) -> Result<String, String> {
    let mut options = Options::parse(arguments)?;
    for directory in [&options.workspace, &options.native_config] {
        if !directory.is_dir() {
            return Err(format!(
                "required directory unavailable: {}",
                directory.display()
            ));
        }
    }
    options.resolve_workspace()?;
    if fs::symlink_metadata(&options.output).is_ok() {
        return Err("configure refuses to replace an existing output file".into());
    }
    let policy = match &options.providers {
        Some(path) => native::read(path, 65_536)?,
        None => DEFAULT_PROVIDERS.to_vec(),
    };
    validate_policy(&policy)?;
    let mut sec = host(
        "sec-core",
        aw_sec_host::NATIVE_CLI_VERSION,
        &options.sec_core,
        &options.workspace,
        2000,
    )?;
    sec.environment
        .insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
    let home = std::env::var("HOME").map_err(|_| "configure requires an explicit UTF-8 HOME")?;
    if !Path::new(&home).is_absolute()
        || !Path::new(&home).is_dir()
        || home.chars().any(char::is_control)
    {
        return Err("configure HOME must be an existing absolute directory".into());
    }
    sec.environment.insert("HOME".into(), home);
    if let Some(path) = std::env::var_os("AGENT_SEC_DATA_DIR") {
        let path = path
            .into_string()
            .map_err(|_| "AGENT_SEC_DATA_DIR must be UTF-8")?;
        if !Path::new(&path).is_absolute()
            || path.chars().any(char::is_control)
            || (Path::new(&path).exists() && !Path::new(&path).is_dir())
        {
            return Err("AGENT_SEC_DATA_DIR must name an absolute directory".into());
        }
        sec.environment.insert("AGENT_SEC_DATA_DIR".into(), path);
    }
    sec.pins = native::sec_core_pins(&options.sec_core, &options.sec_core_pins)?;
    let mut tokenless = host(
        "tokenless",
        aw_tokenless_host::NATIVE_CLI_VERSION,
        &options.tokenless,
        &options.workspace,
        1000,
    )?;
    tokenless.environment.extend([
        ("TOKENLESS_STATS_ENABLED".into(), "0".into()),
        ("TOKENLESS_SLS_ENABLED".into(), "0".into()),
        ("TOKENLESS_COMPRESSION_ENABLED".into(), "1".into()),
    ]);
    if let Some(path) = &options.providers {
        let pin = FilePin {
            path: path.clone(),
            state: PinState::Sha256(canonical::digest(&policy)),
        };
        sec.pins.push(pin.clone());
        tokenless.pins.push(pin);
    }
    let qoder = Config {
        provider_id: "qoder".into(),
        provider_version: "1.1.47".into(),
        program: options.qoder.clone(),
        program_sha256: native::bounded_digest(&options.qoder, 256 * 1024 * 1024)?,
        cwd: options.workspace.clone(),
        args: vec![],
        environment: BTreeMap::new(),
        pins: vec![],
        limits: Limits {
            timeout_ms: 5000,
            input_bytes: 1024,
            output_bytes: 1024,
            stderr_bytes: 1024,
        },
    };
    let herdr = host("herdr", "0.9.0", &options.herdr, &options.workspace, 5000)?;
    verify_herdr(&herdr.program_sha256)?;
    qoder.validate().map_err(|error| error.to_string())?;
    check_qoder(&qoder)?;
    version(&qoder, "1.1.47\n", cancellation)?;
    check_qoder(&qoder)?;
    for (config, expected) in [
        (&sec, "agent-sec-cli 0.12.0\n"),
        (&tokenless, "tokenless 0.8.1\n"),
        (&herdr, "herdr 0.9.0\n"),
    ] {
        config.validate().map_err(|error| error.to_string())?;
        config.check_pins().map_err(|error| error.to_string())?;
        version(config, expected, cancellation)?;
        config.check_pins().map_err(|error| error.to_string())?;
    }
    if cancellation.is_cancelled() {
        return Err("configure cancelled before writing profile".into());
    }
    let profile = profile(&options, &qoder, sec, tokenless);
    let bytes = serde_json::to_vec_pretty(&profile).map_err(|error| error.to_string())?;
    write_new(&options.output, &bytes)?;
    Ok(format!(
        "[aw]\nconfig = {}\nconfig_sha256 = {}\nherdr = {}\nherdr_sha256 = {}\n",
        quoted_path(&options.output)?,
        quoted(&canonical::digest(&bytes))?,
        quoted_path(&options.herdr)?,
        quoted(&herdr.program_sha256)?
    ))
}

fn check_qoder(config: &Config) -> Result<(), String> {
    if native::bounded_digest(&config.program, 256 * 1024 * 1024)? != config.program_sha256 {
        return Err("Qoder executable changed during configuration".into());
    }
    Ok(())
}

fn version(config: &Config, expected: &str, cancellation: &dyn Cancellation) -> Result<(), String> {
    let output = aw_host_process::run(
        config,
        &["--version"],
        &[],
        config.limits.timeout_ms,
        cancellation,
    )
    .map_err(|error| format!("{} version probe: {error}", config.provider_id))?;
    if output.exit_code != 0 || output.stdout != expected.as_bytes() {
        return Err(format!(
            "{} native version does not match the admitted release",
            config.provider_id
        ));
    }
    Ok(())
}

fn validate_policy(bytes: &[u8]) -> Result<(), String> {
    let expected = canonical::parse(DEFAULT_PROVIDERS).map_err(|error| error.to_string())?;
    let policy = canonical::parse(bytes).map_err(|_| "invalid provider policy JSON")?;
    if policy != expected {
        return Err("provider policy differs from the supported Qoder/SecCore/Tokenless conservative native profile".into());
    }
    Ok(())
}

fn host(
    id: &str,
    version: &str,
    program: &Path,
    cwd: &Path,
    timeout_ms: u64,
) -> Result<Config, String> {
    Ok(Config {
        provider_id: id.into(),
        provider_version: version.into(),
        program: program.into(),
        program_sha256: native::digest(program)?,
        cwd: cwd.into(),
        args: vec![],
        environment: BTreeMap::new(),
        pins: vec![],
        limits: Limits {
            timeout_ms,
            input_bytes: 65_536,
            output_bytes: 262_144,
            stderr_bytes: 4096,
        },
    })
}

fn profile(options: &Options, qoder: &Config, sec: Config, tokenless: Config) -> Value {
    json!({
        "format":2,"required_safety":false,
        "qoder":{"program":options.qoder,"program_sha256":qoder.program_sha256},
        "native_config_directory":options.native_config,"cwd":options.workspace,
        "notifications":{},"tool_guard":{"transforms":[],"scanner":sec},
        "tool_response":{"tokenless":tokenless,"accepted_reversibility":["unrecoverable"],"allow_text_reencoding":false}
    })
}

fn verify_herdr(digest: &str) -> Result<(), String> {
    let manifest: Value =
        serde_json::from_str(include_str!("../../../integrations/herdr/upstream.json"))
            .map_err(|error| error.to_string())?;
    if manifest["assets"][std::env::consts::ARCH]["sha256"].as_str() != Some(digest) {
        return Err("Herdr does not match the fixed official v0.9.0 architecture asset".into());
    }
    Ok(())
}

fn quoted_path(path: &Path) -> Result<String, String> {
    quoted(path.to_str().ok_or("configuration paths must be UTF-8")?)
}

fn quoted(value: &str) -> Result<String, String> {
    // JSON basic-string escaping is valid for these UTF-8 paths and hex digests in TOML.
    if value.chars().any(char::is_control) {
        return Err("configuration paths may not contain control characters".into());
    }
    serde_json::to_string(value).map_err(|error| error.to_string())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("create new private profile: {error}"))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(format!("write private profile: {error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join(format!(
                    "configure-test-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove owned configure fixture");
        }
    }

    #[test]
    fn default_policy_only_admits_the_current_native_capabilities() {
        validate_policy(DEFAULT_PROVIDERS).unwrap();
        let mut changed: Value = serde_json::from_slice(DEFAULT_PROVIDERS).unwrap();
        changed["tool_response"]["allow_text_reencoding"] = json!(true);
        assert!(validate_policy(&serde_json::to_vec(&changed).unwrap()).is_err());
        changed = serde_json::from_slice(DEFAULT_PROVIDERS).unwrap();
        changed["model.before_request"] = json!(true);
        assert!(validate_policy(&serde_json::to_vec(&changed).unwrap()).is_err());
        assert!(validate_policy(br#"{"format":1,"format":1}"#).is_err());
    }

    #[test]
    fn configuration_arguments_require_explicit_paths_without_duplicates() {
        assert!(Options::parse(&["--qoder".into()]).is_err());
        assert!(Options::parse(&["--qoder".into(), "relative".into()]).is_err());
        assert!(Options::parse(&["--unknown".into(), "/file".into()]).is_err());
        assert!(
            Options::parse(&["--qoder".into(), "/a".into(), "--qoder".into(), "/b".into()])
                .is_err()
        );
    }
    #[test]
    fn profile_creation_is_private_and_never_replaces_existing_paths() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let directory = Directory::new();
        let path = directory.0.clone();
        let profile = path.join("profile.json");
        write_new(&profile, b"original").unwrap();
        assert_eq!(
            fs::metadata(&profile).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(write_new(&profile, b"replacement").is_err());
        let link = path.join("link.json");
        symlink(&profile, &link).unwrap();
        assert!(write_new(&link, b"replacement").is_err());
        assert_eq!(fs::read(&profile).unwrap(), b"original");
        drop(directory);
        assert!(!path.exists());
    }
    #[test]
    fn physical_workspace_is_shared_by_profile_and_native_hosts() {
        use std::os::unix::fs::symlink;
        let directory = Directory::new();
        let real = directory.0.join("real");
        let child = real.join("child");
        fs::create_dir_all(&child).unwrap();
        let linked = directory.0.join("linked");
        symlink(&real, &linked).unwrap();
        let executable = directory.0.join("component");
        fs::write(&executable, b"component fixture").unwrap();
        let entrypoint = directory.0.join("entrypoint");
        symlink(&executable, &entrypoint).unwrap();
        let expected = fs::canonicalize(&real).unwrap();
        for workspace in [linked, child.join("..")] {
            let args = [
                ("--qoder", &entrypoint),
                ("--native-config", &real),
                ("--sec-core", &entrypoint),
                ("--tokenless", &entrypoint),
                ("--herdr", &entrypoint),
                ("--workspace", &workspace),
                ("--output", &directory.0.join("profile.json")),
            ]
            .into_iter()
            .flat_map(|(key, value)| [key.to_owned(), value.to_str().unwrap().to_owned()])
            .collect::<Vec<_>>();
            let mut options = Options::parse(&args).unwrap();
            options.resolve_workspace().unwrap();
            assert_eq!(options.workspace, expected);
            assert_eq!(
                options.sec_core, entrypoint,
                "keep native environment entrypoint identity"
            );
            let qoder = host("qoder", "1.1.47", &options.qoder, &options.workspace, 5000).unwrap();
            let sec = host(
                "sec-core",
                "0.12.0",
                &options.sec_core,
                &options.workspace,
                2000,
            )
            .unwrap();
            let tokenless = host(
                "tokenless",
                "0.8.1",
                &options.tokenless,
                &options.workspace,
                1000,
            )
            .unwrap();
            let generated = profile(&options, &qoder, sec, tokenless);
            for cwd in [
                &generated["cwd"],
                &generated["tool_guard"]["scanner"]["cwd"],
                &generated["tool_response"]["tokenless"]["cwd"],
            ] {
                assert_eq!(cwd, &json!(expected));
            }
        }
        let path = directory.0.clone();
        drop(directory);
        assert!(!path.exists());
    }
}
