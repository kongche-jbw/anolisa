//! Bind native text observations to independently supplied runtime identities.

use crate::{native, profiles, Adapter, Error, Host};
use aw_contracts::canonical;
use serde_json::{json, Value};

/// Trusted context captured by the native integration owner, not a model argument.
pub struct NativeContext {
    /// Existing AW scope with explicit turn/tool identities for tool boundaries.
    pub scope: Value,
    /// Current binding authenticated against the original runtime owner.
    pub runtime: Value,
    /// Unique native boundary occurrence, retained across retries of that occurrence.
    pub event_id: String,
}

/// Native event data; no universal Agent wire envelope is introduced.
pub struct CaptureRequest {
    /// Exact host event name as registered by the native plugin.
    pub native_event: String,
    /// Already decoded native data; Hermes/OpenClaw use documented local containers.
    pub payload: Value,
    /// Independent identity and authority context from the embedding integration.
    pub context: NativeContext,
}

/// Immutable snapshot of one supported native text slot and its original payload.
pub struct CapturedEvent {
    pub(crate) host: Host,
    pub(crate) native_event: String,
    pub(crate) payload: Value,
    pub(crate) scope: Value,
    pub(crate) runtime: Value,
    pub(crate) event_id: String,
    pub(crate) boundary: Value,
    pub(crate) artifact: Value,
}

impl CapturedEvent {
    /// Original data, preserved without removing structured or non-text fields.
    pub fn native_payload(&self) -> &Value {
        &self.payload
    }

    /// The original host vocabulary, not a renamed global hook name.
    pub fn native_event(&self) -> &str {
        &self.native_event
    }

    /// Exact bundled descriptor for the captured native boundary.
    pub fn boundary(&self) -> &Value {
        &self.boundary
    }

    /// Independently supplied scope after native ID cross-checks.
    pub fn scope(&self) -> &Value {
        &self.scope
    }

    /// Stable occurrence identity for policy plan construction.
    pub fn event_id(&self) -> &str {
        &self.event_id
    }

    /// UTF-8 artifact using the existing common/v1 definition and exact byte digest.
    pub fn artifact(&self) -> &Value {
        &self.artifact
    }
}

impl Adapter {
    /// Extracts native text and checks every observable ID against trusted context.
    ///
    /// Missing native IDs are never invented from a shell PID or another session;
    /// the embedding owner must supply the real scope through NativeContext.
    ///
    /// # Errors
    /// Rejects unknown events, unsupported payloads, stale/mismatched bindings,
    /// missing tool identities and content outside the artifact contract.
    pub fn capture(&self, request: CaptureRequest) -> Result<CapturedEvent, Error> {
        let CaptureRequest {
            native_event,
            payload,
            context,
        } = request;
        let boundary = profiles::boundary(self.host, &native_event)?;
        let extracted = native::extract(self.host, &native_event, &payload)?;
        let NativeContext {
            scope,
            runtime,
            event_id,
        } = context;
        self.registry.validate("runtime-binding-v1", &runtime)?;
        for field in ["runtime_id", "binding_revision", "environment_id"] {
            if scope[field] != runtime[field] {
                return Err(Error::IdentityMismatch("runtime scope"));
            }
        }
        if scope["runtime_generation"] != runtime["generation"] || runtime["state"] != "running" {
            return Err(Error::IdentityMismatch("runtime incarnation"));
        }
        if runtime.get("session_id").is_some() && scope["session_id"] != runtime["session_id"] {
            return Err(Error::IdentityMismatch("runtime session"));
        }
        for field in ["turn_id", "tool_use_id"] {
            check_name(scope[field].as_str())?;
        }
        check_name(Some(&event_id))?;
        if let Some(id) = extracted.session_id {
            if scope["session_id"].as_str() != Some(id.as_str()) {
                return Err(Error::IdentityMismatch("native session"));
            }
        }
        if let Some(id) = extracted.tool_use_id {
            if scope["tool_use_id"].as_str() != Some(id.as_str()) {
                return Err(Error::IdentityMismatch("native tool call"));
            }
        }
        if let Some(id) = extracted.turn_id {
            if scope["turn_id"].as_str() != Some(id.as_str()) {
                return Err(Error::IdentityMismatch("native turn"));
            }
        }
        let artifact_id = canonical::document_digest(&json!({
            "scope": scope, "event_id": event_id,
            "host": self.host.as_str(), "native_event": native_event
        }))?;
        let artifact = json!({
            "id": format!("native-{artifact_id}"),
            "digest": canonical::digest(extracted.text.as_bytes()),
            "content": extracted.text,
            "media_type": "text/plain",
            "origin": extracted.origin,
            "tool_name": extracted.tool_name
        });
        // Both tool phases share this artifact contract. This validates the
        // capture shape only; invoking any plan still requires Core admission.
        self.registry.validate(
            "security-content-inspect-input-v2",
            &json!({
                "artifact": artifact, "boundary": boundary["boundary"],
                "constraints": {"include_low_confidence": false}
            }),
        )?;
        Ok(CapturedEvent {
            host: self.host,
            native_event,
            payload,
            scope,
            runtime,
            event_id,
            boundary,
            artifact,
        })
    }
}

fn check_name(name: Option<&str>) -> Result<(), Error> {
    if !name.is_some_and(|s| {
        !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| (b'!'..=b'~').contains(&b))
    }) {
        return Err(Error::IdentityMismatch(
            "missing or invalid boundary occurrence ID",
        ));
    }
    Ok(())
}
