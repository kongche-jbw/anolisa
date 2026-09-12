//! Maps the fixed native post-tool transport to explicitly unrecoverable candidates.

use aw_contracts::{canonical, Registry};
use serde::Deserialize;
use serde_json::{json, Value};
use tokenless_protocol::{
    estimate_tokens, AppliedOperation, Attribution, ContentOrigin, Disposition, Operation,
    OutputOptimization, PostToolCapabilities, PostToolRequest, PostToolResponse, Recoverability,
    RecoveryMethod, Request, RequestEnvelope, ResultKind, ToolResultStatus, PROTOCOL_VERSION,
    TOKENIZER_ID,
};

/// Holds exact source bytes and attribution for one bounded native exchange.
pub(crate) struct ProjectionRequest<'a> {
    artifact: &'a Value,
    attribution: Attribution,
    replace_with_text: bool,
    wire: Vec<u8>,
}

// Direct typed decoding preserves duplicate-field rejection inside result and
// attribution. The native envelope parser stages result through Value instead.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostToolEnvelope {
    protocol_version: u32,
    operation: Operation,
    attribution: Attribution,
    result: PostToolResponse,
}

impl<'a> ProjectionRequest<'a> {
    /// Validates the AW source and fixes the native operation without command execution.
    pub(crate) fn new(
        registry: &Registry,
        input: &'a Value,
        scope: &Value,
    ) -> Result<Self, &'static str> {
        registry
            .validate("context-projection-prepare-input-v2", input)
            .map_err(|_| "invalid_input")?;
        if input["boundary"] != "post_tool"
            || input["artifact"]["origin"] != "command_output"
            || input["artifact"]["media_type"] != "text/plain"
        {
            return Err("unsupported_source");
        }
        if !input["constraints"]["accepted_reversibility"]
            .as_array()
            .is_some_and(|values| values.contains(&json!("unrecoverable")))
        {
            return Err("unsupported_recovery_requirement");
        }
        let artifact = &input["artifact"];
        let content = artifact["content"].as_str().ok_or("invalid_content")?;
        if artifact["digest"] != canonical::digest(content.as_bytes()) {
            return Err("source_digest_mismatch");
        }
        let attribution = Attribution {
            agent_id: scope["actor_id"]
                .as_str()
                .ok_or("invalid_attribution")?
                .into(),
            session_id: Some(
                scope["session_id"]
                    .as_str()
                    .ok_or("invalid_attribution")?
                    .into(),
            ),
            tool_use_id: Some(
                scope["tool_use_id"]
                    .as_str()
                    .ok_or("invalid_attribution")?
                    .into(),
            ),
        };
        let replace_with_text = input["constraints"]["allow_text_reencoding"]
            .as_bool()
            .ok_or("invalid_constraints")?;
        let envelope = RequestEnvelope {
            attribution: attribution.clone(),
            request: Request::PostTool(PostToolRequest {
                result_kind: ResultKind::Tool,
                tool_name: artifact["tool_name"]
                    .as_str()
                    .ok_or("missing_tool_name")?
                    .into(),
                content: content.into(),
                status: ToolResultStatus::Success,
                content_origin: ContentOrigin::CommandOutput,
                output_optimization: OutputOptimization::None,
                capabilities: PostToolCapabilities {
                    replace_output: true,
                    recovery: RecoveryMethod::None,
                    replace_with_text,
                },
            }),
        };
        let wire = envelope
            .to_json()
            .map_err(|_| "invalid_native_request")?
            .into_bytes();
        Ok(Self {
            artifact,
            attribution,
            replace_with_text,
            wire,
        })
    }

    /// Uses only the native protocol envelope entry point.
    pub(crate) fn args(&self) -> [&str; 1] {
        ["compress"]
    }

    /// Exact encoded request; the shared runner does not normalize it.
    pub(crate) fn stdin(&self) -> &[u8] {
        &self.wire
    }

    /// Rejects contradictory native claims before emitting a candidate or bypass.
    pub(crate) fn project(
        &self,
        exit_code: i32,
        stdout: &[u8],
        registry: &Registry,
    ) -> Result<Option<Value>, &'static str> {
        if exit_code != 0 {
            return Err("native_failed");
        }
        let envelope: PostToolEnvelope =
            serde_json::from_slice(stdout).map_err(|_| "invalid_native_response")?;
        if envelope.protocol_version != PROTOCOL_VERSION
            || envelope.operation != Operation::PostTool
        {
            return Err("native_protocol_mismatch");
        }
        if envelope.attribution != self.attribution {
            return Err("native_attribution_mismatch");
        }
        let native = envelope.result;
        let original = self.artifact["content"].as_str().ok_or("invalid_content")?;
        if native.recoverability != Recoverability::Lossless || !native.stash_keys.is_empty() {
            return Err("unsupported_native_recovery");
        }
        if native.additional_context.is_some() {
            return Err("unexpected_native_diagnostic");
        }
        if native.tokenizer_id != TOKENIZER_ID
            || native.before_tokens != estimate_tokens(original) as u64
        {
            return Err("invalid_native_measurement");
        }
        match native.disposition {
            Disposition::Timeout => Err("native_timeout"),
            Disposition::ToolError => Err("native_tool_error"),
            Disposition::DryRun
            | Disposition::Passthrough
            | Disposition::NoSavings
            | Disposition::RecoverabilityUnavailable => {
                if native.output != original
                    || !native.applied_operations.is_empty()
                    || (native.disposition != Disposition::DryRun
                        && native.after_tokens != native.before_tokens)
                {
                    return Err("native_preserve_mismatch");
                }
                Ok(None)
            }
            Disposition::Applied => {
                // These are the only operations available to this command-output,
                // no-recovery profile; native lossless is a task-semantic claim.
                let operations_supported =
                    native
                        .applied_operations
                        .iter()
                        .all(|operation| match operation {
                            AppliedOperation::TerminalCleanup | AppliedOperation::JsonCleanup => {
                                true
                            }
                            AppliedOperation::Toon | AppliedOperation::TabularCompaction => {
                                self.replace_with_text
                            }
                            AppliedOperation::SchemaCompression
                            | AppliedOperation::SearchPathSharing
                            | AppliedOperation::BuildLogReduction
                            | AppliedOperation::TabularRowReduction
                            | AppliedOperation::JsonRecordReduction
                            | AppliedOperation::JsonTruncation => false,
                        });
                if native.applied_operations.is_empty()
                    || !operations_supported
                    || self.artifact["tool_name"] == "Grep"
                    || native.output.len() >= original.len()
                    || native.output.chars().count() >= original.chars().count()
                    || native.after_tokens != estimate_tokens(&native.output) as u64
                    || native.after_tokens >= native.before_tokens
                {
                    return Err("invalid_native_candidate");
                }
                let chain: Vec<_> = native
                    .applied_operations
                    .iter()
                    .map(|operation| operation.wire_str())
                    .collect();
                let output = json!({"candidate": {
                    "source_artifact_id":self.artifact["id"],"source_digest":self.artifact["digest"],
                    "content":native.output,"media_type":"text/plain","transform_chain":chain,
                    "reversibility":"unrecoverable","recovery":{"mode":"none"}
                }});
                registry
                    .validate("context-projection-prepare-output-v2", &output)
                    .map_err(|_| "invalid_candidate")?;
                Ok(Some(output))
            }
        }
    }
}
