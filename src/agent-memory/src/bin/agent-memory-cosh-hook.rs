//! One-shot Cosh hook binding for the provider-neutral RuntimeAdapter.

use std::fs;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;

use agent_memory::adapter::cosh::{
    CoshAdapterConfig, CoshHookInput, CoshHookOutput, CoshRuntimeAdapter, MAX_COSH_HOOK_INPUT_BYTES,
};
use agent_memory::protocol::{LocalMemoryBackend, default_local_memory_path};
use anyhow::{Context, Result};
use git2::{ObjectType, Oid};
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
    let canonical_workspace = fs::canonicalize(&input.cwd)
        .with_context(|| format!("failed to resolve Cosh workspace {}", input.cwd))?;
    let workspace_digest =
        Oid::hash_object(ObjectType::Blob, canonical_workspace.as_os_str().as_bytes())
            .context("failed to fingerprint the Cosh workspace")?;
    let config = CoshAdapterConfig::local(
        format!("unix:{}", Uid::effective().as_raw()),
        "cosh-ng",
        format!("local-path-sha1:{workspace_digest}"),
    );
    let backend = LocalMemoryBackend::open(default_local_memory_path()?)?;
    let adapter = CoshRuntimeAdapter::new(backend, config);
    Ok(adapter.handle(input).output)
}

fn allow_output() -> CoshHookOutput {
    CoshHookOutput {
        should_continue: true,
        hook_specific_output: None,
    }
}
