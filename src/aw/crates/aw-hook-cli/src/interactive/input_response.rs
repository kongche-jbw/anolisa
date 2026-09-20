//! Explicit input response command; observation routes cannot acquire this authority.

use super::{evidence, storage, Binding, Error, State};
use aw_contracts::{canonical, events::Notification};
use aw_core::{
    input_response::{InputResponseCommand, InputResponseHost},
    journal::FileJournal,
    ports::{Cancellation, HostError},
    Core,
};
use serde_json::Value;
use std::{path::Path, time::Instant};

pub(super) fn respond(
    core: &Core,
    root: &Path,
    binding: &Binding,
    event: &Notification,
    cancellation: &dyn Cancellation,
    deadline: Instant,
) -> Result<Value, Error> {
    let config = binding
        .prepared
        .config
        .input_response
        .as_ref()
        .ok_or(Error::Evidence)?;
    let response = core
        .respond_to_input(
            event,
            &InputResponseCommand {
                id: &config.provider_id,
                deadline,
            },
            &mut Host(config),
            &mut FileJournal::new(root.join("input-response-journal")).map_err(evidence)?,
            cancellation,
        )
        .map_err(evidence)?;
    let _lock = storage::lock(root)?;
    let state: State = storage::read(&root.join("state.json"))?;
    if cancellation.is_cancelled()
        || Instant::now() >= deadline
        || !state.attached
        || Some(state.attachment) != event.session_epoch
    {
        return Ok(aw_adapters::qoder_input::unavailable());
    }
    Ok(response.as_ref().map_or_else(
        aw_adapters::qoder_input::unavailable,
        aw_adapters::qoder_input::response,
    ))
}

struct Host<'a>(&'a aw_host_process::Config);
impl InputResponseHost for Host<'_> {
    fn respond(
        &mut self,
        command: &str,
        request: &Value,
        remaining_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<Value, HostError> {
        let failure = || HostError {
            code: "input_response_command_failed".into(),
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
