//! A single Core path for every supported native host.

use crate::{Adapter, CapturedEvent, Error, Host};
use aw_core::{
    ports::{Cancellation, Clock, Journal, ProviderHost},
    Execution, PrepareRequest, PreparedPlan, StepInput,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Policy-owned capability constraints and Host budgets, keyed by plan step.
pub struct StepOptions {
    /// Existing capability constraints; validated against the selected schema.
    pub constraints: Value,
    /// Existing input/output byte and wall-time budget object.
    pub budget: Value,
    /// Absolute deadline retained by Core without extension.
    pub deadline_at_ms: u64,
}

/// Core-admitted work, retaining the original native snapshot for the caller.
pub struct PreparedEvent {
    captured: CapturedEvent,
    plan: PreparedPlan,
}

impl PreparedEvent {
    /// Core's scope/event reservation key, useful for journal lookup after failure.
    pub fn event_key(&self) -> &str {
        self.plan.event_key()
    }
}

/// Correlated Core output alongside unchanged native data.
///
/// Returning this object does not alter a tool result, apply a security policy,
/// emit a native block instruction or assert adoption. The native plugin retains
/// those responsibilities and its host-specific fallback/approval semantics.
pub struct AdaptedExecution {
    captured: CapturedEvent,
    execution: Execution,
}

impl AdaptedExecution {
    /// Native host associated with this execution.
    pub fn host(&self) -> Host {
        self.captured.host
    }

    /// Original payload, including fields unrelated to the extracted text slot.
    pub fn native_payload(&self) -> &Value {
        &self.captured.payload
    }

    /// Actual Core calls, receipts, decision and journal acknowledgement.
    pub fn execution(&self) -> &Execution {
        &self.execution
    }
}

impl Adapter {
    /// Builds every capability input from the same captured source and enters Core.
    ///
    /// The policy owner supplies the resolved plan; this method checks its event,
    /// scope, boundary and source rather than rewriting a mismatched plan. Profiles
    /// do not promise final guards, so current pre-tool execution is rejected by
    /// the unchanged Core contract. Capturing a pre-tool event is still supported.
    ///
    /// # Errors
    /// Rejects cross-host snapshots, changed identities/source, missing or extra
    /// options, unsupported capabilities and any Core admission failure.
    pub fn prepare(
        &self,
        captured: CapturedEvent,
        plan: Value,
        mut options: BTreeMap<String, StepOptions>,
        host: &impl ProviderHost,
        now_ms: u64,
    ) -> Result<PreparedEvent, Error> {
        if captured.host != self.host {
            return Err(Error::IdentityMismatch("adapter host"));
        }
        if plan["scope"] != captured.scope || plan["event_id"] != captured.event_id {
            return Err(Error::IdentityMismatch("plan occurrence"));
        }
        if plan["source_digest"] != captured.artifact["digest"] {
            return Err(Error::IdentityMismatch("plan source"));
        }
        self.registry.validate_plan(&plan, &captured.boundary)?;
        let mut inputs = BTreeMap::new();
        for step in plan["steps"]
            .as_array()
            .ok_or(Error::UnsupportedCapability)?
        {
            let id = step["step_id"]
                .as_str()
                .ok_or(Error::UnsupportedCapability)?;
            let config = options
                .remove(id)
                .ok_or(Error::UnsupportedPayload("missing step options"))?;
            let schema = match step["capability"].as_str() {
                Some("security.content.inspect/v2") => "security-content-inspect-input-v2",
                Some("security.code.inspect/v2") => "security-code-inspect-input-v2",
                _ => return Err(Error::UnsupportedCapability),
            };
            let input = json!({
                "artifact": captured.artifact,
                "boundary": captured.boundary["boundary"],
                "constraints": config.constraints
            });
            self.registry.validate(schema, &input)?;
            inputs.insert(
                id.to_owned(),
                StepInput {
                    input,
                    budget: config.budget,
                    deadline_at_ms: config.deadline_at_ms,
                },
            );
        }
        if !options.is_empty() {
            return Err(Error::UnsupportedPayload("unplanned step options"));
        }
        let prepared = self.core.prepare(
            PrepareRequest {
                plan,
                boundary: captured.boundary.clone(),
                runtime: captured.runtime.clone(),
                inputs,
            },
            host,
            now_ms,
        )?;
        Ok(PreparedEvent {
            captured,
            plan: prepared,
        })
    }

    /// Executes through the real Core/Host/Journal ports and preserves native data.
    ///
    /// # Errors
    /// Returns Core failures without fabricating a native allow, denial or adoption.
    pub fn execute(
        &self,
        prepared: PreparedEvent,
        host: &mut impl ProviderHost,
        journal: &mut impl Journal,
        clock: &impl Clock,
        cancellation: &impl Cancellation,
    ) -> Result<AdaptedExecution, Error> {
        if prepared.captured.host != self.host {
            return Err(Error::IdentityMismatch("adapter host"));
        }
        let execution = self
            .core
            .execute(prepared.plan, host, journal, clock, cancellation)?;
        Ok(AdaptedExecution {
            captured: prepared.captured,
            execution,
        })
    }
}
