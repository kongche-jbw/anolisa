//! One-shot Cosh hook binding for the provider-neutral RuntimeAdapter.

use std::env;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_memory::adapter::cosh::{
    CoshAdapterConfig, CoshHookInput, CoshHookOutput, CoshRuntimeAdapter, MAX_COSH_HOOK_INPUT_BYTES,
};
use agent_memory::knowledge::mant::{MantCliConfig, MantCliProvider};
use agent_memory::protocol::{
    KnowledgeProviderBinding, LocalMemoryBackend, default_local_memory_path, local_workspace_id,
};
use anyhow::{Context, Result};
use nix::unistd::Uid;

fn main() -> Result<()> {
    let mut raw = Vec::new();
    io::stdin()
        .take(u64::try_from(MAX_COSH_HOOK_INPUT_BYTES)?.saturating_add(1))
        .read_to_end(&mut raw)
        .context("failed to read the Cosh hook request")?;

    let output = if raw.len() > MAX_COSH_HOOK_INPUT_BYTES {
        allow_output()
    } else {
        run_hook(&raw).unwrap_or_else(|_| allow_output())
    };
    serde_json::to_writer(io::stdout().lock(), &output)
        .context("failed to write the Cosh hook response")?;
    Ok(())
}

fn run_hook(raw: &[u8]) -> Result<CoshHookOutput> {
    let input: CoshHookInput = serde_json::from_slice(raw).context("invalid Cosh hook request")?;
    let workspace = input.workspace_root.as_deref().unwrap_or(&input.cwd);
    let workspace_id =
        local_workspace_id(workspace).context("failed to identify Cosh workspace")?;
    let config = CoshAdapterConfig::local(
        format!("unix:{}", Uid::effective().as_raw()),
        "cosh-ng",
        workspace_id,
    );
    let database = default_local_memory_path()?;
    let backend = match mant_binding() {
        Some(binding) => LocalMemoryBackend::open_with_knowledge(database, binding)?,
        None => LocalMemoryBackend::open(database)?,
    };
    let adapter = CoshRuntimeAdapter::new(backend, config);
    Ok(adapter.handle(input).output)
}

fn mant_binding() -> Option<KnowledgeProviderBinding> {
    if env::var_os("ANOLISA_MEMORY_MANT")
        .is_some_and(|value| matches!(value.to_str(), Some("0" | "off" | "false")))
    {
        return None;
    }
    let executable = env::var_os("ANOLISA_MANT_PATH")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| executable_on_path("mant"))?;
    let document_id = env::var("ANOLISA_MEMORY_MANT_DOCUMENT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "bash".to_string());
    let mut config = MantCliConfig::new(executable);
    // One query performs a protocol probe and a focused request. Keeping each
    // process below 175 ms preserves the adapter's shared 500 ms deadline.
    config.timeout = Duration::from_millis(175);
    config.max_stdout_bytes = 64 * 1024;
    config.max_stderr_bytes = 8 * 1024;
    KnowledgeProviderBinding::new(Arc::new(MantCliProvider::new(config)), document_id).ok()
}

fn executable_on_path(name: &str) -> Option<PathBuf> {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find_map(|candidate| {
            let metadata = fs::metadata(&candidate).ok()?;
            if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
                fs::canonicalize(candidate).ok()
            } else {
                None
            }
        })
}

fn allow_output() -> CoshHookOutput {
    CoshHookOutput {
        should_continue: true,
        hook_specific_output: None,
        system_message: None,
    }
}
