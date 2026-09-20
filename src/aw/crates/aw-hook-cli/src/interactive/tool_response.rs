//! Optional result projection bound to the interactive owner, never a synthetic Turn.

use super::{evidence, storage, Binding, Error, State};
use aw_contracts::{canonical, events::Notification, tool_response::ToolResponse};
use aw_core::{
    journal::FileJournal,
    ports::{Cancellation, HostError},
    tool_response::{ToolResponseCommand, ToolResponseHost},
    Core,
};
use aw_host_process::Config;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::Path, time::Instant};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    pub command: Config,
    pub accepted_reversibility: Vec<String>,
}

pub(super) fn respond(
    core: &Core,
    root: &Path,
    binding: &Binding,
    event: &Notification,
    cancellation: &dyn Cancellation,
    deadline: Instant,
) -> Result<Value, Error> {
    let settings = binding
        .prepared
        .config
        .tool_response
        .as_ref()
        .ok_or(Error::Evidence)?;
    let source = aw_adapters::qoder_result::source(&event.payload).map_err(evidence)?;
    let response = core
        .respond_to_tool(
            event,
            &ToolResponseCommand {
                id: &settings.command.provider_id,
                source: &source,
                deadline,
            },
            &mut Host(&settings.command),
            &mut FileJournal::new(root.join("tool-response-journal")).map_err(evidence)?,
            cancellation,
        )
        .map_err(evidence)?;
    let _lock = storage::lock(root)?;
    let mut state: State = storage::read(&root.join("state.json"))?;
    if cancellation.is_cancelled()
        || Instant::now() >= deadline
        || !state.attached
        || Some(state.attachment) != event.session_epoch
    {
        return Ok(aw_adapters::qoder_result::unavailable());
    }
    Ok(match response {
        Some(ToolResponse::Preserve) => json!({}),
        Some(ToolResponse::Replace(text)) => aw_adapters::qoder_result::replacement(&text),
        None => {
            state.gap = true;
            storage::replace(&root.join("state.json"), &state)?;
            aw_adapters::qoder_result::unavailable()
        }
    })
}

struct Host<'a>(&'a Config);
impl ToolResponseHost for Host<'_> {
    fn respond(
        &mut self,
        command: &str,
        request: &Value,
        remaining_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<Value, HostError> {
        let failure = || HostError {
            code: "tool_response_command_failed".into(),
        };
        if command != self.0.provider_id {
            return Err(failure());
        }
        self.0.check_pins().map_err(|_| failure())?;
        let output = aw_host_process::run(
            self.0,
            &[],
            &canonical::bytes(request).map_err(|_| failure())?,
            remaining_ms.min(self.0.limits.timeout_ms),
            cancellation,
        )
        .map_err(|_| failure())?;
        self.0.check_pins().map_err(|_| failure())?;
        if output.exit_code != 0 {
            return Err(failure());
        }
        canonical::parse(&output.stdout).map_err(|_| failure())
    }
}
