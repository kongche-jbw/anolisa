//! Optional lifecycle commands use Core claims and preserve native control ownership.

use super::{evidence, hooks, storage, Binding, Error, State};
use aw_contracts::{
    canonical,
    events::{EventName, Notification},
};
use aw_core::{
    journal::FileJournal,
    notifications::NotificationHost,
    ports::{Cancellation, HostError},
    Core,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

pub(super) fn callback(
    root: &Path,
    binding: &Binding,
    payload: &Value,
    cancellation: &dyn Cancellation,
) -> Result<Value, Error> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    if cancellation.is_cancelled() {
        return Err(Error::Profile("callback cancelled"));
    }
    let name = aw_adapters::qoder_events::validate(payload).map_err(evidence)?;
    if payload["cwd"].as_str() != binding.prepared.config.cwd()?.to_str() {
        return Err(Error::Profile("callback workspace mismatch"));
    }
    let session = payload["session_id"].as_str().ok_or(Error::Evidence)?;
    let lock = storage::lock(root)?;
    let mut state: State = storage::read(&root.join("state.json"))?;
    if state.notification_sequence >= 1024 {
        return Err(Error::Profile("notification capacity reached"));
    }
    if name == EventName::SessionStart {
        if let Err(error) = hooks::start(&mut state, session, payload) {
            state.gap = true;
            if payload["source"] == "clear" && state.session_id.as_deref() == Some(session) {
                state.attached = false;
            }
            storage::replace(&root.join("state.json"), &state)?;
            return Err(error);
        }
    } else if name == EventName::InputSubmit && state.session_id.is_none() {
        // First-workspace trust in Qoder 1.1.47 can skip SessionStart. The
        // authenticated main input binds identity, never fabricates that event.
        state.session_id = Some(session.into());
        state.attachment = 1;
        state.attached = true;
        state.gap = true;
    } else if name == EventName::SessionEnd && state.session_id.is_none() {
        // Qoder 1.1.47 emits SessionEnd after login admission failure without
        // SessionStart. Bind only this terminal observation, never Agent readiness.
        state.session_id = Some(session.into());
        state.attachment = 1;
    } else if state.session_id.as_deref() != Some(session) || !state.attached {
        return Err(Error::Profile("callback belongs to an inactive attachment"));
    }
    state.notification_sequence += 1;
    let tool = payload["tool_use_id"].as_str().map(str::to_owned);
    let child = matches!(name, EventName::SubagentStart | EventName::SubagentStop)
        .then(|| payload["agent_id"].as_str().map(str::to_owned))
        .flatten();
    // Tool IDs identify one execution, including a mutually exclusive success or
    // failure result. Other repeated callbacks may lack a stable native ID; do
    // not deduplicate identical prompts or repeated stop/compaction checks.
    let occurrence = if matches!(name, EventName::ToolBefore | EventName::ToolAfter) {
        let key = canonical::document_digest(&json!([session, state.attachment, tool]))
            .map_err(evidence)?;
        let path = root.join("calls").join(key);
        if name == EventName::ToolBefore {
            storage::create(&path, &json!({"tool_name":payload["tool_name"]}))?;
        } else {
            let before: Value = storage::read(&path)?;
            if before["tool_name"] != payload["tool_name"] {
                return Err(Error::Evidence);
            }
            // Reserve either terminal outcome even when there is no notify route
            // or the result is outside the optional replacement profile.
            if binding.prepared.config.tool_response.is_some() {
                storage::create(
                    &path.with_extension("after"),
                    &json!({"native_event":payload["hook_event_name"]}),
                )?;
            }
        }
        canonical::document_digest(&json!([name, session, state.attachment, tool]))
            .map_err(evidence)?
    } else if child.is_some() || name == EventName::SessionEnd {
        canonical::document_digest(&json!([name, session, state.attachment, child]))
            .map_err(evidence)?
    } else {
        format!("callback-{}", state.notification_sequence)
    };
    if name == EventName::SessionEnd {
        state.attached = false;
    }
    storage::replace(&root.join("state.json"), &state)?;
    let event = Notification {
        format: 1,
        event: name,
        source: "native_callback".into(),
        native_event: payload["hook_event_name"].as_str().map(str::to_owned),
        runtime_id: binding.runtime_id.clone(),
        runtime_generation: 1,
        session_id: Some(session.into()),
        session_epoch: Some(state.attachment),
        turn_id: None,
        turn_unknown_reason: Some("native_callback_does_not_prove_task_acceptance".into()),
        tool_call_id: tool,
        subagent_id: child,
        occurrence_id: occurrence,
        config_revision: binding.prepared.revision.clone(),
        payload: payload.clone(),
    };
    event.validate().map_err(evidence)?;
    let routes = binding
        .prepared
        .config
        .notifications
        .as_ref()
        .ok_or(Error::Evidence)?;
    let guarded = name == EventName::ToolBefore
        && payload["tool_name"] == "Bash"
        && binding.prepared.config.tool_guard.is_some();
    let responding =
        name == EventName::InputSubmit && binding.prepared.config.input_response.is_some();
    let stopping = name == EventName::TurnStop && binding.prepared.config.stop_response.is_some();
    let projecting = name == EventName::ToolAfter
        && payload["hook_event_name"] == "PostToolUse"
        && payload["tool_name"] == "Bash"
        && binding.prepared.config.tool_response.is_some();
    let Some(commands) = routes.get(&name) else {
        drop(lock);
        let cancellation = AttachmentCancellation {
            root,
            epoch: state.attachment,
            external: cancellation,
        };
        return if guarded {
            super::tool_guard::check(root, binding, &event, &cancellation, deadline)
        } else if responding {
            super::input_response::respond(
                &Core::new().map_err(evidence)?,
                root,
                binding,
                &event,
                &cancellation,
                deadline,
            )
        } else if stopping {
            super::stop_response::respond(
                &Core::new().map_err(evidence)?,
                root,
                binding,
                &event,
                &cancellation,
                deadline,
            )
        } else if projecting {
            super::tool_response::respond(
                &Core::new().map_err(evidence)?,
                root,
                binding,
                &event,
                &cancellation,
                deadline,
            )
        } else {
            Ok(json!({}))
        };
    };
    let key = event.event_key().map_err(evidence)?;
    let record = root.join("notifications").join(format!("{key}.json"));
    let metadata = event.metadata().map_err(evidence)?;
    storage::create(&record, &json!({"event":metadata,"status":"pending"}))?;
    drop(lock);
    let cancellation = AttachmentCancellation {
        root,
        epoch: state.attachment,
        external: cancellation,
    };
    let core = Core::new().map_err(evidence)?;
    let result = (|| {
        // Admit the complete route before invoking its first command.
        for command in commands {
            command.check_pins().map_err(evidence)?;
        }
        let ids: Vec<_> = commands.iter().map(|c| c.provider_id.clone()).collect();
        let mut host = Host { commands, deadline };
        let mut journal = FileJournal::new(root.join("notification-journal")).map_err(evidence)?;
        core.notify(&event, &ids, &mut host, &mut journal, &cancellation)
            .map_err(evidence)
    })();
    let _lock = storage::lock(root)?;
    let mut current: State = storage::read(&root.join("state.json"))?;
    let stale = current.attachment != state.attachment;
    let failed = result
        .as_ref()
        .map_or(true, |report| report["gap"] != false)
        || stale;
    if failed && !stale {
        current.gap = true;
        storage::replace(&root.join("state.json"), &current)?;
    }
    storage::replace(
        &record,
        &json!({"event":metadata,
        "status":if failed { "failed" } else { "completed" },"stale":stale,
        "execution":result.ok()}),
    )?;
    drop(_lock);
    if responding {
        // Notification acknowledgements never become input decisions. The
        // separately trusted responder still shares the callback deadline.
        return super::input_response::respond(
            &core,
            root,
            binding,
            &event,
            &cancellation,
            deadline,
        );
    }
    if stopping {
        return super::stop_response::respond(
            &core,
            root,
            binding,
            &event,
            &cancellation,
            deadline,
        );
    }
    if projecting {
        return super::tool_response::respond(
            &core,
            root,
            binding,
            &event,
            &cancellation,
            deadline,
        );
    }
    if guarded {
        return if failed {
            Ok(aw_adapters::qoder_tool::denied())
        } else {
            super::tool_guard::check(root, binding, &event, &cancellation, deadline)
        };
    }
    Ok(if failed {
        json!({"systemMessage":"AW notification unavailable or attachment changed; native behavior unchanged."})
    } else {
        json!({})
    })
}

struct AttachmentCancellation<'a> {
    root: &'a Path,
    epoch: u64,
    external: &'a dyn Cancellation,
}

impl Cancellation for AttachmentCancellation<'_> {
    fn is_cancelled(&self) -> bool {
        self.external.is_cancelled()
            || storage::read::<State>(&self.root.join("state.json"))
                .map_or(true, |state| state.attachment != self.epoch)
    }
}

pub(super) struct Host<'a> {
    pub(super) commands: &'a [aw_host_process::Config],
    pub(super) deadline: std::time::Instant,
}

impl NotificationHost for Host<'_> {
    fn notify(
        &mut self,
        command: &str,
        event: &Notification,
        remaining_ms: u64,
        cancellation: &dyn Cancellation,
    ) -> Result<Value, HostError> {
        let handler = self
            .commands
            .iter()
            .find(|c| c.provider_id == command)
            .ok_or_else(|| HostError {
                code: "unknown_notification_command".into(),
            })?;
        handler.check_pins().map_err(|_| HostError {
            code: "notification_pin_changed".into(),
        })?;
        let input = serde_json::to_vec(event).map_err(|_| HostError {
            code: "invalid_notification".into(),
        })?;
        let result = aw_host_process::run(
            handler,
            &[],
            &input,
            remaining_ms.min(handler.limits.timeout_ms).min(
                self.deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .as_millis() as u64,
            ),
            cancellation,
        )
        .map_err(|_| HostError {
            code: "notification_process_failed".into(),
        })?;
        handler.check_pins().map_err(|_| HostError {
            code: "notification_pin_changed".into(),
        })?;
        if result.exit_code != 0 {
            return Err(HostError {
                code: "notification_command_failed".into(),
            });
        }
        canonical::parse(&result.stdout).map_err(|_| HostError {
            code: "invalid_notification_ack".into(),
        })
    }
}

pub(super) fn query(root: &Path, binding: &Binding, state: &State) -> Result<Value, Error> {
    let alive = crate::process_identity(binding.agent_pid)
        .is_ok_and(|(_, ticks)| ticks == binding.agent_ticks);
    let mut completed = 0;
    let mut failed = 0;
    let mut pending = 0;
    let runtime = if root.join("runtime.json").try_exists()? {
        storage::read::<Value>(&root.join("runtime.json"))?
    } else if super::runtime_events::configured(&binding.prepared.config) {
        if root.join("owner-registration.json").try_exists()? {
            json!({"status":"registration_failed"})
        } else {
            json!({"status":"awaiting_registration"})
        }
    } else {
        json!({"status":"not_configured"})
    };
    let alive = alive && runtime["root_exited"] != true;
    let owner = root.parent().ok_or(Error::Evidence)?.join("observer.json");
    let owner = if owner.try_exists()? {
        storage::read::<Value>(&owner)?
    } else {
        json!({"status":"not_configured"})
    };
    for (index, entry) in fs::read_dir(root.join("notifications"))?.enumerate() {
        if index >= 1024 {
            return Err(Error::Evidence);
        }
        let value: Value = storage::read(&entry?.path())?;
        if value["event"]["session_epoch"] != state.attachment {
            continue;
        }
        match value["status"].as_str() {
            Some("completed") => completed += 1,
            Some("failed") => failed += 1,
            Some("pending") => pending += 1,
            _ => return Err(Error::Evidence),
        }
    }
    Ok(
        json!({"format":2,"profile":"qoder-1.1.47/lifecycle-notify-v1",
        "runtime_id":binding.runtime_id,"runtime_generation":1,"agent_pid":binding.agent_pid,
        "runtime_alive":alive,"runtime_status":if alive {"observed_running"} else {"exited_or_unavailable"},
        "exit_status":null,"owner_observation":runtime,"runtime_observer":owner,
        "session_id":state.session_id,"attachment":state.attachment,
        "session_start_observed":state.session_start_observed,
        "attached":state.attached && alive,"config_revision":binding.prepared.revision,
        "bridge_status":if state.attachment == 0 {"awaiting_native_callback"} else {"callback_observed"},
        "calls":completed+failed+pending,"observed":completed,"failed":failed,"pending":pending,
        "observation_gap":state.gap,"effect":if binding.prepared.config.tool_guard.is_some() {"experimental_native_bash_guard"} else if binding.prepared.config.input_response.is_some() {"experimental_native_input_response"} else if binding.prepared.config.stop_response.is_some() {"experimental_native_stop_response"} else if binding.prepared.config.tool_response.is_some() {"experimental_native_tool_response"} else {"notify_only"},
        "tool_guard":if binding.prepared.config.tool_guard.is_some() {"configured_not_certified"} else {"not_configured"},
        "input_response":if binding.prepared.config.input_response.is_some() {"experimental_native_response"} else {"not_configured"},
        "stop_response":if binding.prepared.config.stop_response.is_some() {"experimental_native_response"} else {"not_configured"},
        "tool_response":if binding.prepared.config.tool_response.is_some() {"experimental_bash_result_response"} else {"not_configured"},
        "replacement":if binding.prepared.config.tool_response.is_some() {"experimental_bash_success_text"} else {"unsupported"},
        "adoption":"unsupported","os_coverage":"not_attached",
        "required_safety":"unsupported"}),
    )
}
