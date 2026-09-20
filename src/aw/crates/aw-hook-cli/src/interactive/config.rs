//! Explicit configuration trust and shell-scoped shim preparation.

use super::{evidence, storage, Config, Error, Prepared};
use aw_contracts::canonical;
use std::{fs, os::unix::fs::symlink, path::Path};

/// Prepares a new shell-scoped directory from an explicitly trusted config digest.
///
/// The embedding shell removes `root` after its native session exits. This only
/// prepares artifacts: it starts no Agent, handler, observer or supervisor.
///
/// # Errors
/// Rejects untrusted bytes, existing roots, unsupported guarantees or unsafe pins.
pub fn prepare(
    config_path: &Path,
    expected_digest: &str,
    root: &Path,
    helper: &Path,
) -> Result<(), Error> {
    let bytes =
        crate::input::read_private(config_path, crate::MAX_INPUT_BYTES).map_err(evidence)?;
    if canonical::digest(&bytes) != expected_digest {
        return Err(Error::Profile(
            "configuration is not trusted at this revision",
        ));
    }
    let config: Config =
        serde_json::from_value(canonical::parse(&bytes).map_err(evidence)?).map_err(evidence)?;
    validate(&config)?;
    if !root.is_absolute() || !helper.is_absolute() || !helper.is_file() {
        return Err(Error::Profile(
            "absolute root and installed helper required",
        ));
    }
    let owner_pid = std::process::id();
    let (_, owner_ticks) = crate::process_identity(owner_pid).map_err(evidence)?;
    storage::directory(root)?;
    let result = (|| {
        storage::directory(&root.join("bin"))?;
        storage::create(
            &root.join("prepared.json"),
            &Prepared {
                config,
                revision: expected_digest.into(),
                owner_pid,
                owner_ticks,
                helper: helper.to_path_buf(),
            },
        )?;
        symlink(helper, root.join("bin/qoder"))?;
        Ok(())
    })();
    if result.is_err() {
        fs::remove_dir_all(root)?;
    }
    result
}

fn validate(config: &Config) -> Result<(), Error> {
    if !cfg!(target_os = "linux") || config.required_safety {
        return Err(Error::Profile(
            "only Linux optional observation is supported; required safety unavailable",
        ));
    }
    check_agent(&config.qoder)?;
    if !config.native_config_directory.is_absolute() || !config.native_config_directory.is_dir() {
        return Err(Error::Profile("invalid native or handler profile"));
    }
    match config.format {
        1 if config.cwd.is_none()
            && config.notifications.is_none()
            && config.tool_guard.is_none()
            && config.input_response.is_none()
            && config.stop_response.is_none()
            && config.tool_response.is_none() =>
        {
            check_handler(config, config.legacy_handler()?)?;
        }
        2 if config.handler.is_none() => {
            if !config.cwd()?.is_absolute() || !config.cwd()?.is_dir() {
                return Err(Error::Profile("absolute notification workspace required"));
            }
            let routes = config
                .notifications
                .as_ref()
                .ok_or(Error::Profile("notification routes required"))?;
            if let Some(guard) = &config.tool_guard {
                super::tool_guard::validate(config, guard)?;
            }
            if let Some(command) = &config.input_response {
                check_handler(config, command)?;
            }
            if let Some(command) = &config.stop_response {
                check_handler(config, command)?;
            }
            if let Some(settings) = &config.tool_response {
                check_handler(config, settings.provider()?)?;
                if let Some(tokenless) = &settings.tokenless {
                    aw_tokenless_host::post_tool::validate_config(tokenless).map_err(evidence)?;
                }
                if settings.accepted_reversibility != ["unrecoverable"] {
                    return Err(Error::Profile(
                        "tool response requires unrecoverable opt-in",
                    ));
                }
            }
            if routes.is_empty()
                && config.tool_guard.is_none()
                && config.input_response.is_none()
                && config.stop_response.is_none()
                && config.tool_response.is_none()
            {
                return Err(Error::Profile("notification routes are empty"));
            }
            for (event, commands) in routes {
                if !super::runtime_events::owns(*event)
                    && !aw_adapters::qoder_events::NATIVE_EVENTS.iter().any(|name| {
                        aw_adapters::qoder_events::event_name(name).is_ok_and(|e| e == *event)
                    })
                {
                    return Err(Error::Profile(
                        "event source is not connected in this profile",
                    ));
                }
                if commands.is_empty() || commands.len() > 4 {
                    return Err(Error::Profile(
                        "each notification requires one to four commands",
                    ));
                }
                let mut ids = std::collections::HashSet::new();
                for command in commands {
                    check_handler(config, command)?;
                    if !ids.insert(&command.provider_id) {
                        return Err(Error::Profile("duplicate notification command ID"));
                    }
                }
            }
        }
        _ => {
            return Err(Error::Profile(
                "invalid configuration revision or mixed formats",
            ))
        }
    }
    Ok(())
}

fn check_handler(config: &Config, handler: &aw_host_process::Config) -> Result<(), Error> {
    handler.validate().map_err(evidence)?;
    if config.qoder.program == handler.program
        || handler.limits.timeout_ms > 1000
        || handler.cwd != config.cwd()?
    {
        return Err(Error::Profile("invalid notification command profile"));
    }
    Ok(())
}

pub(super) fn check_agent(native: &super::NativeExecutable) -> Result<(), Error> {
    use sha2::{Digest, Sha256};
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    const MAX_AGENT_BYTES: u64 = 512 * 1024 * 1024;
    if !native.program.is_absolute() || native.program_sha256.len() != 64 {
        return Err(Error::Profile("invalid Agent executable pin"));
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&native.program)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_AGENT_BYTES {
        return Err(Error::Profile("Agent executable exceeds 512 MiB profile"));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_AGENT_BYTES {
            return Err(Error::Evidence);
        }
        digest.update(&buffer[..count]);
    }
    if format!("{:x}", digest.finalize()) != native.program_sha256 {
        return Err(Error::Profile("Agent executable changed"));
    }
    Ok(())
}
