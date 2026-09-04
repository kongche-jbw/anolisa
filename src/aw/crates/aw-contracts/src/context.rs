//! Provider-independent contracts for context artifacts and projections.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    common::{BoundedName, BoundedStringError, Digest, DigestError},
    ids::ArtifactId,
    provider::{SchemaReference, VersionedSchema},
};

/// Stable identity of the context-projection Capability.
pub const CONTEXT_PROJECTION_PREPARE_CAPABILITY_ID: &str = "context.projection.prepare";
/// Current revision of the context-projection Capability.
pub const CONTEXT_PROJECTION_PREPARE_CAPABILITY_VERSION: u16 = 1;
/// Stable identity of the canonical context-projection input schema.
pub const CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_ID: &str = "context.projection.prepare.input";
/// Current revision of the canonical context-projection input schema.
pub const CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_VERSION: u16 = 1;
/// SHA-256 of the current canonical context-projection input schema resource.
pub const CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_SHA256: &str =
    "07d62b903479892f45620173c1ce7804176cfd8b97350e6035b52823f15053cb";
/// Stable identity of the canonical context-projection output schema.
pub const CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_ID: &str = "context.projection.prepare.output";
/// Current revision of the canonical context-projection output schema.
pub const CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_VERSION: u16 = 1;
/// SHA-256 of the current canonical context-projection output schema resource.
pub const CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_SHA256: &str =
    "4a171682511cfad0611626ab942c31d33928140b8eccd99a7e4d3c2e63997f37";

/// Describes where model-visible context entered the governed execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextArtifactOrigin {
    /// No more precise origin was supplied by the Agent Environment.
    Unspecified,
    /// Output captured from a local or remote command.
    CommandOutput,
    /// Content read from a file.
    FileContent,
    /// Structured or textual content returned by an API.
    ApiResponse,
}

/// Tool output submitted by an Agent Environment for context preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResultSubmission {
    /// Model-visible tool output before any projection is adopted.
    pub content: String,
    /// Media type of [`Self::content`].
    pub media_type: BoundedName,
    /// Provenance category attached by the Agent Environment.
    pub origin: ContextArtifactOrigin,
    /// Tool name when the Environment can provide one safely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<BoundedName>,
    /// Whether a Provider may replace the original representation with text.
    pub allow_text_reencoding: bool,
}

/// Recoverability guarantee declared for a projected representation.
///
/// This guarantee concerns source information, not only meaning that a
/// Provider considers relevant to the current task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextReversibility {
    /// Returned representation alone retains enough information to reconstruct
    /// the exact source; preserving only task-relevant meaning is not lossless.
    Lossless,
    /// Source information can be recovered through separately governed state.
    Retrievable,
    /// The transformation cannot reconstruct all source information.
    Unrecoverable,
}

/// Provider-produced context representation that Core may adopt or ignore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextProjectionCandidate {
    /// Source artifact to which this candidate applies.
    pub source_artifact_id: ArtifactId,
    /// Digest of the exact source content used to derive the candidate.
    pub source_digest: Digest,
    /// Candidate model-visible representation.
    pub content: String,
    /// Media type of [`Self::content`].
    pub media_type: BoundedName,
    /// More specific content type when the Provider distinguishes one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<BoundedName>,
    /// Ordered, bounded names of transformations applied by the Provider.
    pub transform_chain: Vec<BoundedName>,
    /// Recoverability guarantee for this representation.
    pub reversibility: ContextReversibility,
}

/// Failure returned while constructing a built-in context Contract reference.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ContextContractBuildError {
    /// A built-in schema name violates the bounded-name invariant.
    #[error(transparent)]
    Name(#[from] BoundedStringError),
    /// A built-in schema digest is not canonical SHA-256 text.
    #[error(transparent)]
    Digest(#[from] DigestError),
}

/// Returns the current context-projection Capability identity.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant violates its bounded
/// representation. Such a failure indicates a build-time defect.
pub fn context_projection_prepare_capability() -> Result<VersionedSchema, ContextContractBuildError>
{
    versioned_schema(
        CONTEXT_PROJECTION_PREPARE_CAPABILITY_ID,
        CONTEXT_PROJECTION_PREPARE_CAPABILITY_VERSION,
    )
}

/// Returns the exact current canonical context-projection input Contract.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant is invalid.
pub fn context_projection_prepare_input_contract(
) -> Result<SchemaReference, ContextContractBuildError> {
    schema_reference(
        CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_ID,
        CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_VERSION,
        CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_SHA256,
    )
}

/// Returns the exact current canonical context-projection output Contract.
///
/// # Errors
///
/// Returns an error if a compiled-in Contract constant is invalid.
pub fn context_projection_prepare_output_contract(
) -> Result<SchemaReference, ContextContractBuildError> {
    schema_reference(
        CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_ID,
        CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_VERSION,
        CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_SHA256,
    )
}

fn schema_reference(
    id: &str,
    version: u16,
    digest: &str,
) -> Result<SchemaReference, ContextContractBuildError> {
    Ok(SchemaReference {
        schema: versioned_schema(id, version)?,
        digest: Digest::parse(digest)?,
    })
}

fn versioned_schema(id: &str, version: u16) -> Result<VersionedSchema, ContextContractBuildError> {
    Ok(VersionedSchema {
        id: BoundedName::new(id)?,
        version,
    })
}

#[cfg(test)]
mod tests {
    use sha2::{Digest as _, Sha256};

    use super::{
        context_projection_prepare_capability, context_projection_prepare_input_contract,
        context_projection_prepare_output_contract, CONTEXT_PROJECTION_PREPARE_CAPABILITY_ID,
        CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_SHA256,
        CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_SHA256,
    };

    #[test]
    fn built_in_context_contracts_are_canonical() {
        let capability = context_projection_prepare_capability()
            .expect("compiled-in Capability identity is canonical");
        let input = context_projection_prepare_input_contract()
            .expect("compiled-in input Contract is canonical");
        let output = context_projection_prepare_output_contract()
            .expect("compiled-in output Contract is canonical");

        assert_eq!(
            capability.id.as_str(),
            CONTEXT_PROJECTION_PREPARE_CAPABILITY_ID
        );
        assert_eq!(
            input.digest.as_str(),
            CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_SHA256
        );
        assert_eq!(
            output.digest.as_str(),
            CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_SHA256
        );
    }

    #[test]
    fn canonical_schema_resources_match_their_contract_digests() {
        let input = include_bytes!("../schemas/context-projection-prepare-input-v1.schema.json");
        let output = include_bytes!("../schemas/context-projection-prepare-output-v1.schema.json");

        assert_eq!(
            format!("{:x}", Sha256::digest(input)),
            CONTEXT_PROJECTION_PREPARE_INPUT_SCHEMA_SHA256
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(output)),
            CONTEXT_PROJECTION_PREPARE_OUTPUT_SCHEMA_SHA256
        );
    }
}
