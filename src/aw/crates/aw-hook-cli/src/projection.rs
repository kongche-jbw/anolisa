//! Explicit Qoder projection through required inspection and one Core execution.

use crate::{
    bind, identity, owner, records, wall_time, Error, Settings, SharedCancellation, WallClock,
    MAX_INPUT_BYTES,
};
use aw_adapters::{Adapter, CaptureRequest, Host, NativeContext, StepOptions};
use aw_contracts::{canonical, Registry};
use aw_core::{
    journal::FileJournal,
    ports::{Cancellation, HostError, NeverCancel, ProviderHost, ProviderResult},
};
use aw_sec_host::SecHost;
use aw_tokenless_host::{Config, TokenlessHost};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

/// Operator-owned opt-in for retaining and returning nonrecoverable candidates.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionSettings {
    /// Existing owner, scope, inspection provider and Journal configuration.
    pub hook: Settings,
    /// Separately registered native Tokenless provider and its hard limits.
    pub tokenless: Config,
    /// Private records containing explicitly retained source and candidate bytes.
    pub record_directory: PathBuf,
    /// Exact native Qoder history file used for later independent observation.
    pub history_path: PathBuf,
    /// Fixed native history grammar admitted by the record reader.
    pub history_profile: String,
    /// Must explicitly select `source_and_candidate` retention.
    pub retention: String,
    /// Bounded interval in which later native history may establish adoption.
    pub max_observation_delay_ms: u64,
    /// Must explicitly select only `unrecoverable`; native lossless is not AW recovery.
    pub accepted_reversibility: Vec<String>,
    /// Explicit permission for native TOON or tabular text reencoding.
    pub allow_text_reencoding: bool,
}

/// Native response and durable context; delivery alone does not establish adoption.
pub struct ProjectionResult {
    /// Qoder's native hook output; only a verified candidate replaces tool text.
    pub response: Value,
    /// Durable record that the CLI marks only after stdout writes and flushes.
    pub record_path: PathBuf,
    /// Whether this response contains a candidate for the native replacement slot.
    pub projected: bool,
}

struct Providers {
    security: SecHost,
    tokenless: TokenlessHost,
}

impl ProviderHost for Providers {
    fn descriptor(&self, provider_id: &str) -> Option<&Value> {
        self.security
            .descriptor(provider_id)
            .or_else(|| self.tokenless.descriptor(provider_id))
    }

    fn invoke(&mut self, invocation: &Value) -> Result<ProviderResult, HostError> {
        let provider_id = invocation["provider_id"]
            .as_str()
            .ok_or_else(|| HostError {
                code: "invalid_provider_id".into(),
            })?;
        if self.security.descriptor(provider_id).is_some() {
            self.security.invoke(invocation)
        } else if self.tokenless.descriptor(provider_id).is_some() {
            self.tokenless.invoke(invocation)
        } else {
            Err(HostError {
                code: "unregistered_provider".into(),
            })
        }
    }
}

/// Prepares one Qoder candidate after required inspection and durable recording.
///
/// # Errors
/// Rejects missing opt-in, unsupported native results, conflicting owner/context,
/// inadmissible providers and any incomplete recording before returning text.
pub fn project(settings: ProjectionSettings, payload: Value) -> Result<ProjectionResult, Error> {
    project_with_cancellation(settings, payload, Arc::new(NeverCancel))
}

/// Shares cancellation across both providers and suppresses late candidate release.
///
/// # Errors
/// Includes all [`project`] failures; cancellation after recording leaves durable
/// execution facts without returning a replacement or claiming delivery.
pub fn project_with_cancellation(
    mut settings: ProjectionSettings,
    payload: Value,
    cancellation: Arc<dyn Cancellation + Send + Sync>,
) -> Result<ProjectionResult, Error> {
    if settings.accepted_reversibility != ["unrecoverable"]
        || settings.hook.provider.provider_id == settings.tokenless.provider_id
        || payload["hook_event_name"] != "PostToolUse"
        || payload["tool_name"] != "Bash"
        || !payload["tool_response"].is_object()
        || !settings.hook.scope.is_object()
        || !settings.hook.journal.is_absolute()
    {
        return Err(Error::Input);
    }
    owner::verify(&settings.hook)?;
    for field in ["session_id", "tool_use_id"] {
        bind(&mut settings.hook.scope, field, identity(&payload, field)?)?;
    }
    let turn = settings
        .hook
        .qoder_single_turn_id
        .clone()
        .ok_or(Error::Identity)?;
    bind(&mut settings.hook.scope, "turn_id", turn)?;
    let event_id = canonical::document_digest(&json!({
        "scope":settings.hook.scope,"host":"qoder","event":"PostToolUse"
    }))
    .map_err(|_| Error::Input)?;
    let adapter = Adapter::new(Host::Qoder).map_err(|_| Error::Execution)?;
    let captured = adapter
        .capture(CaptureRequest {
            native_event: "PostToolUse".into(),
            payload: payload.clone(),
            context: NativeContext {
                scope: settings.hook.scope.clone(),
                runtime: settings.hook.runtime.clone(),
                event_id,
            },
        })
        .map_err(|_| Error::Input)?;
    // Validate retention/history before even a native version probe can run.
    let pending = records::prepare(&settings, &payload)?;
    let mut providers = Providers {
        security: SecHost::new_cancellable(settings.hook.provider.clone(), cancellation.clone())
            .map_err(|_| Error::Provider)?,
        tokenless: TokenlessHost::new_cancellable(settings.tokenless.clone(), cancellation.clone())
            .map_err(|_| Error::Provider)?,
    };
    let registry = Registry::new().map_err(|_| Error::Execution)?;
    let reference = |name| registry.reference(name).map_err(|_| Error::Execution);
    let mut steps = Vec::new();
    for (id, capability, input_schema, output_schema, provider_id, required) in [
        (
            "inspect",
            "security.content.inspect/v2",
            "security-content-inspect-input-v2",
            "security-content-inspect-output-v2",
            settings.hook.provider.provider_id.as_str(),
            true,
        ),
        (
            "project",
            "context.projection.prepare/v2",
            "context-projection-prepare-input-v2",
            "context-projection-prepare-output-v2",
            settings.tokenless.provider_id.as_str(),
            false,
        ),
    ] {
        let descriptor = providers.descriptor(provider_id).ok_or(Error::Provider)?;
        steps.push(json!({"step_id":id,"capability":capability,"input_schema":reference(input_schema)?,"output_schema":reference(output_schema)?,
            "selection":"exactly_one","providers":[{"provider_id":descriptor["provider_id"],"provider_version":descriptor["provider_version"],"manifest_digest":descriptor["manifest_digest"]}],
            "required":required,"on_failure":"reject_plan","input_source":"boundary_source"}));
    }
    let plan = json!({"plan_id":captured.event_id(),"revision":1,"event_id":captured.event_id(),"scope":captured.scope(),
        "boundary_id":captured.boundary()["boundary_id"],"boundary_revision":captured.boundary()["revision"],"boundary":"post_tool",
        "policy_revision":1,"source_digest":captured.artifact()["digest"],"steps":steps});
    let now = wall_time()?;
    let inspection_deadline = now
        .checked_add(settings.hook.provider.limits.timeout_ms)
        .ok_or(Error::Clock)?;
    let projection_deadline = inspection_deadline
        .checked_add(settings.tokenless.limits.timeout_ms)
        .ok_or(Error::Clock)?;
    let mut options = BTreeMap::new();
    for (id, config, constraints, deadline_at_ms) in [
        (
            "inspect",
            &settings.hook.provider,
            json!({"include_low_confidence":settings.hook.include_low_confidence}),
            inspection_deadline,
        ),
        (
            "project",
            &settings.tokenless,
            json!({"accepted_reversibility":settings.accepted_reversibility,"allow_text_reencoding":settings.allow_text_reencoding}),
            projection_deadline,
        ),
    ] {
        options.insert(id.into(), StepOptions { constraints,
            budget:json!({"input_bytes":MAX_INPUT_BYTES,"output_bytes":config.limits.output_bytes,"wall_time_ms":config.limits.timeout_ms}), deadline_at_ms });
    }
    let prepared = adapter
        .prepare(captured, plan, options, &providers, now)
        .map_err(|_| Error::Execution)?;
    let event_key = prepared.event_key().to_owned();
    let mut journal = FileJournal::new(&settings.hook.journal).map_err(|_| Error::Execution)?;
    owner::verify(&settings.hook)?;
    let adapted = adapter
        .execute(
            prepared,
            &mut providers,
            &mut journal,
            &WallClock,
            &SharedCancellation(cancellation.clone()),
        )
        .map_err(|_| Error::Execution)?;
    let execution = adapted.execution();
    journal
        .read_verified(&event_key, execution.journal_ack())
        .map_err(|_| Error::Execution)?;
    let record_path = pending.persist(execution)?;
    owner::verify(&settings.hook)?;
    if cancellation.is_cancelled() {
        return Err(Error::Execution);
    }
    let verdict = execution
        .calls()
        .first()
        .and_then(|call| call.result().output.as_ref())
        .and_then(|output| output["inspection"]["verdict"].as_str());
    let candidate = execution
        .calls()
        .last()
        .filter(|call| {
            execution.record()["decision"] == "proceed"
                && verdict.is_some()
                && call.invocation()["capability"] == "context.projection.prepare/v2"
        })
        .and_then(|call| call.result().output.as_ref())
        .and_then(|output| output["candidate"]["content"].as_str());
    let mut response = match candidate {
        Some(text) => {
            json!({"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":text}})
        }
        None => json!({}),
    };
    match verdict {
        Some("sensitive" | "suspicious") => response["systemMessage"] = json!("AW found sensitive or suspicious content. Security inspection is observational; no denial was applied."),
        Some("clean") => {},
        _ => response = crate::unavailable_response(),
    }
    Ok(ProjectionResult {
        response,
        record_path,
        projected: candidate.is_some(),
    })
}
