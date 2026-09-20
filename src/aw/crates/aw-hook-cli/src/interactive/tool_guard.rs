//! Experimental native guard; required/final admission remains unavailable.

use super::{evidence, storage, Binding, Error, State};
use aw_contracts::{canonical, events::Notification};
use aw_core::{
    journal::FileJournal,
    ports::{Cancellation, HostError},
    tool_chain::{ToolChain, ToolHost},
    Core,
};
use aw_host_process::Config;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    pub transforms: Vec<Config>,
    pub scanner: Config,
}

pub(super) fn validate(config: &super::Config, settings: &Settings) -> Result<(), Error> {
    let mut ids = std::collections::HashSet::new();
    if settings.transforms.len() > 3 {
        return Err(Error::Profile("at most three tool transforms"));
    }
    // Version verification and scanning share the caller's two-second deadline.
    // Python CLI startup can consume most of a one-second provider budget.
    for (command, limit_ms) in settings
        .transforms
        .iter()
        .map(|command| (command, 1000))
        .chain(std::iter::once((&settings.scanner, 2000)))
    {
        command.validate().map_err(evidence)?;
        if command.cwd != config.cwd()?
            || command.limits.timeout_ms > limit_ms
            || !ids.insert(&command.provider_id)
            || command.provider_id.is_empty()
            || command.provider_id.len() > 256
            || command.program == config.qoder.program
        {
            return Err(Error::Profile("invalid tool guard command"));
        }
    }
    Ok(())
}

pub(super) fn check(
    root: &Path,
    binding: &Binding,
    event: &Notification,
    cancellation: &dyn Cancellation,
    deadline: std::time::Instant,
) -> Result<Value, Error> {
    let settings = binding
        .prepared
        .config
        .tool_guard
        .as_ref()
        .ok_or(Error::Evidence)?;
    let chain = ToolChain {
        deadline,
        transforms: settings
            .transforms
            .iter()
            .map(|c| c.provider_id.clone())
            .collect(),
        guard: settings.scanner.provider_id.clone(),
    };
    let mut host = Host { settings };
    let candidate = aw_adapters::qoder_tool::candidate(&event.payload).map_err(evidence)?;
    let result = Core::new()
        .map_err(evidence)?
        .check_tool(
            event,
            candidate,
            &chain,
            &mut host,
            &mut FileJournal::new(root.join("tool-check-journal")).map_err(evidence)?,
            cancellation,
        )
        .map_err(evidence)?;
    let _lock = storage::lock(root)?;
    let state: State = storage::read(&root.join("state.json"))?;
    if cancellation.is_cancelled()
        || std::time::Instant::now() >= deadline
        || !state.attached
        || Some(state.attachment) != event.session_epoch
    {
        return Ok(aw_adapters::qoder_tool::denied());
    }
    match result.candidate() {
        Some(candidate) => aw_adapters::qoder_tool::checked(candidate).map_err(evidence),
        None => Ok(aw_adapters::qoder_tool::denied()),
    }
}

struct Host<'a> {
    settings: &'a Settings,
}
fn failure() -> HostError {
    HostError {
        code: "tool_guard_command_failed".into(),
    }
}
impl ToolHost for Host<'_> {
    fn admit(&self, _: &ToolChain) -> Result<(), HostError> {
        for config in self
            .settings
            .transforms
            .iter()
            .chain(std::iter::once(&self.settings.scanner))
        {
            config.check_pins().map_err(|_| failure())?;
        }
        Ok(())
    }
    fn validate(&self, candidate: &Value) -> Result<(), HostError> {
        aw_adapters::qoder_tool::validate(candidate).map_err(|_| failure())
    }
    fn transform(
        &mut self,
        command: &str,
        candidate: &Value,
        remaining_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<Value, HostError> {
        let config = self
            .settings
            .transforms
            .iter()
            .find(|c| c.provider_id == command)
            .ok_or_else(failure)?;
        config.check_pins().map_err(|_| failure())?;
        let input =
            canonical::bytes(&json!({"format":1,"event":"tool.before","candidate":candidate}))
                .map_err(|_| failure())?;
        let output = aw_host_process::run(
            config,
            &[],
            &input,
            remaining_ms.min(config.limits.timeout_ms),
            cancellation,
        )
        .map_err(|_| failure())?;
        config.check_pins().map_err(|_| failure())?;
        let value = canonical::parse(&output.stdout).map_err(|_| failure())?;
        if output.exit_code != 0
            || value.as_object().map(|o| o.len()) != Some(2)
            || value["format"] != 1
        {
            return Err(failure());
        }
        // Only command text is writable in this slice. Preserve other native fields.
        let text = value["command"].as_str().ok_or_else(failure)?;
        let mut next = candidate.clone();
        next["tool_input"]["command"] = json!(text);
        Ok(next)
    }
    fn guard(
        &mut self,
        command: &str,
        candidate: &Value,
        remaining_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<bool, HostError> {
        if command != self.settings.scanner.provider_id {
            return Err(failure());
        }
        aw_sec_host::code::check(
            &self.settings.scanner,
            candidate["tool_input"]["command"]
                .as_str()
                .ok_or_else(failure)?,
            remaining_ms,
            cancellation,
        )
    }
}
