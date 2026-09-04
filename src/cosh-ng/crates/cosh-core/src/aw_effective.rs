//! Trusted AW boundary for the bytes COSH will place in model history.
//!
//! Ordinary hooks receive redacted copies. This boundary runs after their
//! decisions have settled and is the only in-process COSH component allowed to
//! submit the resulting unredacted bytes to AW Providers.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use aw_contracts::canonical::canonical_json_v1_bytes;
use aw_contracts::common::{BoundedName, BoundedOpaque, Digest, TargetRef};
use aw_contracts::context::{
    ContextArtifactOrigin, ContextProjectionCandidate, ContextReversibility, ToolResultSubmission,
    CONTEXT_PROJECTION_PREPARE_CAPABILITY_ID,
};
use aw_contracts::ids::{
    ActorId, AgentSessionId, ArtifactId, EnvironmentId, ExecutionContextId, LedgerEventId,
    ToolUseId, TurnId,
};
use aw_contracts::ledger::{
    ContextAdoptionBody, ContextAdoptionDecision, ContextAdoptionReason, LedgerEventKind,
    LedgerInvocationRef, LedgerTraceScope, LEDGER_CONTEXT_ADOPTION_SCHEMA,
    LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
};
use aw_contracts::provider::ProviderReceipt;
use aw_core::{CapabilityPreferences, Core, CoreConfig, SessionContextSpec, ToolResultOutcome};
use aw_ledger::{LedgerSink, LedgerStore};
use aw_provider_host::{ProviderAdmissionOptions, ProviderCatalog, ProviderManifestSource};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

/// Complete, typed input to the trusted effective-bytes boundary.
///
/// `content` is intentionally not serializable or printable through `Debug`:
/// it may contain material that ordinary hooks were not allowed to observe.
#[derive(Clone)]
pub struct EffectiveBytesRequest {
    /// Provisional model-visible bytes after every ordinary post-tool hook.
    pub content: String,
    /// Media type COSH inferred for `content`.
    pub media_type: BoundedName,
    /// Provenance category for the tool result.
    pub origin: ContextArtifactOrigin,
    /// Canonical tool name when COSH can represent it safely.
    pub tool_name: Option<BoundedName>,
    /// Agent Environment serving this session.
    pub environment_id: EnvironmentId,
    /// Governed execution context shared by the session.
    pub execution_context_id: ExecutionContextId,
    /// Caller correlation asserted at the in-process boundary.
    pub actor_id: ActorId,
    /// Logical Agent session that owns this result.
    pub agent_session_id: AgentSessionId,
    /// Prompt turn that produced the tool call.
    pub turn_id: TurnId,
    /// AW identity derived for the native tool call.
    pub tool_use_id: ToolUseId,
}

/// Content and evidence returned by AW before COSH adopts a candidate.
///
/// The candidate remains transient because it may contain model-visible
/// content. The other fields are content-free and form the evidence needed by
/// a later final-adoption Ledger record.
#[derive(Clone, PartialEq, Eq)]
pub struct EffectiveBytesOutcome {
    /// Immutable source artifact allocated by AW Core.
    pub source_artifact_id: ArtifactId,
    /// SHA-256 of the exact provisional bytes submitted by COSH.
    pub source_digest: Digest,
    /// Provider candidate offer; COSH may preserve source instead of selecting it.
    pub candidate: Option<ContextProjectionCandidate>,
    /// SHA-256 of canonical JSON for the complete candidate.
    pub candidate_digest: Option<Digest>,
    /// Closed COSH decision about which offered representation is effective.
    pub selection: EffectiveBytesSelection,
    /// SHA-256 of the exact bytes COSH would place in model history.
    pub effective_digest: Digest,
    /// Content-free Provider facts in deterministic AW plan order.
    pub receipts: Vec<ProviderReceipt>,
    /// State of the optional system-owned plan Ledger append.
    pub ledger: EffectiveBytesLedgerState,
}

/// COSH's closed selection after separately observing a Provider offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveBytesSelection {
    /// Commit the non-empty, lossless Provider candidate.
    Candidate,
    /// Commit source bytes because the Provider offered no candidate.
    SourceNoCandidate,
    /// Commit source bytes because the offered candidate was empty.
    SourceEmptyCandidate,
    /// Commit source bytes because the candidate was not lossless.
    SourceCandidateNotLossless,
}

/// Durability required from the system-owned effective-bytes Ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveBytesLedgerAssurance {
    /// Keep the history decision when a Ledger append fails, with an explicit
    /// runtime degradation diagnostic.
    BestEffort,
    /// Withhold the history decision when its Ledger evidence cannot persist.
    Required,
}

/// Result of attempting to durably record the PostToolUse plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectiveBytesLedgerState {
    /// No first-class Ledger was configured.
    Disabled,
    /// The plan exists and a final adoption record can reference it.
    Ready {
        /// Durable identity of the typed PostToolUse plan.
        plan_event_id: LedgerEventId,
        /// Failure policy COSH must apply to the later adoption append.
        assurance: EffectiveBytesLedgerAssurance,
    },
    /// A best-effort plan append failed, so no adoption claim can be made.
    Degraded,
}

/// System-owned Ledger settings for the effective-bytes boundary.
#[derive(Debug, Clone)]
pub struct EffectiveBytesLedgerConfig {
    /// Absolute directory that owns `ledger.db`.
    pub root: PathBuf,
    /// Whether an unrecorded decision must be withheld from history.
    pub assurance: EffectiveBytesLedgerAssurance,
}

impl EffectiveBytesOutcome {
    /// Verifies that this outcome can be applied to the supplied source bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when source identity, candidate identity, the closed
    /// selection, or either derived digest is inconsistent.
    pub fn validate_for_source(&self, source: &str) -> Result<(), EffectiveBytesError> {
        let actual_source_digest = digest_bytes(source.as_bytes())?;
        if self.source_digest != actual_source_digest {
            return Err(EffectiveBytesError::SourceDigestMismatch);
        }

        let Some(candidate) = &self.candidate else {
            if self.candidate_digest.is_some() {
                return Err(EffectiveBytesError::UnexpectedCandidateDigest);
            }
            if self.selection != EffectiveBytesSelection::SourceNoCandidate {
                return Err(EffectiveBytesError::SelectionMismatch);
            }
            if self.effective_digest != self.source_digest {
                return Err(EffectiveBytesError::EffectiveDigestMismatch);
            }
            return Ok(());
        };

        if candidate.source_artifact_id != self.source_artifact_id {
            return Err(EffectiveBytesError::SourceArtifactMismatch);
        }
        if candidate.source_digest != self.source_digest {
            return Err(EffectiveBytesError::CandidateSourceDigestMismatch);
        }
        let expected_candidate_digest = digest_candidate(candidate)?;
        if self.candidate_digest.as_ref() != Some(&expected_candidate_digest) {
            return Err(EffectiveBytesError::CandidateDigestMismatch);
        }
        let expected_selection = selection_for_candidate(candidate);
        if self.selection != expected_selection {
            return Err(EffectiveBytesError::SelectionMismatch);
        }
        let effective = if self.selection == EffectiveBytesSelection::Candidate {
            candidate.content.as_str()
        } else {
            source
        };
        if self.effective_digest != digest_bytes(effective.as_bytes())? {
            return Err(EffectiveBytesError::EffectiveDigestMismatch);
        }
        Ok(())
    }

    /// Returns the Provider bytes COSH selected, if the offer was adoptable.
    #[must_use]
    pub fn selected_candidate(&self) -> Option<&ContextProjectionCandidate> {
        if self.selection == EffectiveBytesSelection::Candidate {
            self.candidate.as_ref()
        } else {
            None
        }
    }

    /// Builds content-free evidence for bytes already present in the COSH
    /// history slot.
    ///
    /// Returning `None` means the first-class Ledger was disabled or its
    /// best-effort plan append degraded. The caller must not claim adoption in
    /// either case.
    ///
    /// # Errors
    ///
    /// Returns an error when the supplied history bytes differ from the
    /// effective digest or their length cannot be represented by the contract.
    pub fn adoption_body_for_history(
        &self,
        committed: &str,
    ) -> Result<Option<(ContextAdoptionBody, EffectiveBytesLedgerAssurance)>, EffectiveBytesError>
    {
        let EffectiveBytesLedgerState::Ready {
            plan_event_id,
            assurance,
        } = &self.ledger
        else {
            return Ok(None);
        };
        if digest_bytes(committed.as_bytes())? != self.effective_digest {
            return Err(EffectiveBytesError::HistoryDigestMismatch);
        }
        let effective_byte_count =
            u64::try_from(committed.len()).map_err(|_| EffectiveBytesError::HistorySizeOverflow)?;
        let (decision, reason) = match self.selection {
            EffectiveBytesSelection::Candidate => (
                ContextAdoptionDecision::Adopted,
                ContextAdoptionReason::LosslessCandidate,
            ),
            EffectiveBytesSelection::SourceNoCandidate => (
                ContextAdoptionDecision::Preserved,
                ContextAdoptionReason::NoCandidate,
            ),
            EffectiveBytesSelection::SourceEmptyCandidate => (
                ContextAdoptionDecision::Preserved,
                ContextAdoptionReason::EmptyCandidate,
            ),
            EffectiveBytesSelection::SourceCandidateNotLossless => (
                ContextAdoptionDecision::Preserved,
                ContextAdoptionReason::CandidateNotLossless,
            ),
        };
        Ok(Some((
            ContextAdoptionBody {
                plan_event_id: plan_event_id.clone(),
                source_artifact_id: self.source_artifact_id.clone(),
                source_digest: self.source_digest.clone(),
                candidate_envelope_digest: self.candidate_digest.clone(),
                effective_digest: self.effective_digest.clone(),
                effective_byte_count,
                decision,
                reason,
                provider_invocations: self
                    .receipts
                    .iter()
                    .map(LedgerInvocationRef::from_receipt)
                    .collect(),
            },
            *assurance,
        )))
    }

    fn from_aw_outcome(
        outcome: ToolResultOutcome,
        source: &str,
    ) -> Result<Self, EffectiveBytesError> {
        let receipts = outcome.receipts().into_iter().cloned().collect();
        let source_artifact_id = outcome.source_artifact_id;
        let source_digest = outcome.source_digest;
        let receipt_output_digest = outcome.projection.receipt.output_digest.clone();
        let candidate = outcome.projection.candidate;
        let candidate_digest = match candidate.as_ref() {
            Some(candidate) => {
                let computed = digest_candidate(candidate)?;
                if receipt_output_digest.as_ref() != Some(&computed) {
                    return Err(EffectiveBytesError::CandidateDigestMismatch);
                }
                receipt_output_digest
            }
            None => None,
        };
        let selection = match candidate.as_ref() {
            Some(candidate) => selection_for_candidate(candidate),
            None => EffectiveBytesSelection::SourceNoCandidate,
        };
        let effective_digest = match selection {
            EffectiveBytesSelection::Candidate => digest_bytes(
                candidate
                    .as_ref()
                    .ok_or(EffectiveBytesError::SelectionMismatch)?
                    .content
                    .as_bytes(),
            )?,
            EffectiveBytesSelection::SourceNoCandidate
            | EffectiveBytesSelection::SourceEmptyCandidate
            | EffectiveBytesSelection::SourceCandidateNotLossless => source_digest.clone(),
        };
        let result = Self {
            source_artifact_id,
            source_digest,
            candidate,
            candidate_digest,
            selection,
            effective_digest,
            receipts,
            ledger: EffectiveBytesLedgerState::Disabled,
        };
        result.validate_for_source(source)?;
        Ok(result)
    }
}

/// Async boundary used by COSH to prepare final model-visible tool bytes.
#[async_trait]
pub trait EffectiveBytesBoundary: Send + Sync {
    /// Runs the trusted preparation pipeline for one provisional tool result.
    async fn prepare(
        &self,
        request: EffectiveBytesRequest,
    ) -> Result<EffectiveBytesOutcome, EffectiveBytesError>;

    /// Persists evidence for bytes COSH has already committed to history.
    ///
    /// Implementations that never return a ready Ledger state may retain this
    /// default. COSH calls it only after [`EffectiveBytesLedgerState::Ready`].
    async fn record_adoption(
        &self,
        _body: ContextAdoptionBody,
        _scope: LedgerTraceScope,
    ) -> Result<LedgerEventId, EffectiveBytesError> {
        Err(EffectiveBytesError::LedgerNotConfigured)
    }
}

/// System-owned configuration for the in-process AW implementation.
#[derive(Debug, Clone)]
pub struct AwEffectiveBytesConfig {
    /// Directory whose direct children are AW Provider packages.
    pub provider_root: PathBuf,
    /// Trusted directories used to resolve bare Provider executable names.
    pub executable_roots: Vec<PathBuf>,
    /// Opaque local host identity placed in the AW target scope.
    pub target_identifier: String,
    /// Preferred context projection Provider when several are admitted.
    pub preferred_projection_provider: Option<String>,
    /// Maximum wall time granted to one Provider invocation.
    pub provider_wall_time_ms: u64,
    /// Explicit PoC opt-in for Providers whose declared controls are not yet
    /// enforced by an OS isolation backend.
    pub allow_unenforced_providers: bool,
    /// Optional first-class Ledger; absent keeps durable recording disabled.
    pub ledger: Option<EffectiveBytesLedgerConfig>,
}

/// Real effective-bytes boundary backed by AW Core and Provider Host.
///
/// Provider discovery and synchronous process invocation execute on Tokio's
/// blocking pool. One cached Core is serialized because its plan API owns
/// invocation state mutably.
pub struct AwEffectiveBytesBoundary {
    runtime: Arc<Mutex<EffectiveBytesRuntime>>,
    provider_source: ProviderManifestSource,
    provider_admission: ProviderAdmissionOptions,
    target: TargetRef,
    preferences: CapabilityPreferences,
    core_config: CoreConfig,
    ledger_config: Option<EffectiveBytesLedgerConfig>,
}

#[derive(Default)]
struct EffectiveBytesRuntime {
    core: Option<Core>,
    ledger: Option<LedgerSink>,
}

impl AwEffectiveBytesBoundary {
    /// Validates a system-owned configuration without reading Provider files.
    ///
    /// # Errors
    ///
    /// Returns an error for relative roots, invalid bounded target/provider
    /// names, or a zero Provider deadline.
    pub fn new(config: AwEffectiveBytesConfig) -> Result<Self, EffectiveBytesError> {
        if !config.provider_root.is_absolute() {
            return Err(EffectiveBytesError::InvalidConfiguration(
                "AW Provider root must be absolute".to_owned(),
            ));
        }
        if config
            .executable_roots
            .iter()
            .any(|root| !root.is_absolute())
        {
            return Err(EffectiveBytesError::InvalidConfiguration(
                "AW executable roots must be absolute".to_owned(),
            ));
        }
        if config.provider_wall_time_ms == 0 {
            return Err(EffectiveBytesError::InvalidConfiguration(
                "AW Provider wall time must be non-zero".to_owned(),
            ));
        }
        if config
            .ledger
            .as_ref()
            .is_some_and(|ledger| !ledger.root.is_absolute())
        {
            return Err(EffectiveBytesError::InvalidConfiguration(
                "AW Ledger root must be absolute".to_owned(),
            ));
        }

        let target = TargetRef {
            kind: BoundedName::new("host")?,
            authority: BoundedName::new("local")?,
            identifier: BoundedOpaque::new(config.target_identifier)?,
        };
        let preferences = match config.preferred_projection_provider {
            Some(provider_id) => CapabilityPreferences::for_capability(
                CONTEXT_PROJECTION_PREPARE_CAPABILITY_ID,
                BoundedName::new(provider_id)?,
            )?,
            None => CapabilityPreferences::default(),
        };
        let defaults = CoreConfig::default();
        let core_config = CoreConfig {
            provider_wall_time_ms: config.provider_wall_time_ms,
            allow_unenforced_providers: config.allow_unenforced_providers,
            ..defaults
        };

        Ok(Self {
            runtime: Arc::new(Mutex::new(EffectiveBytesRuntime::default())),
            provider_source: ProviderManifestSource::Directory(config.provider_root),
            provider_admission: ProviderAdmissionOptions {
                executable_roots: config.executable_roots,
            },
            target,
            preferences,
            core_config,
            ledger_config: config.ledger,
        })
    }

    fn prepare_blocking(
        runtime: &Mutex<EffectiveBytesRuntime>,
        provider_source: ProviderManifestSource,
        provider_admission: ProviderAdmissionOptions,
        target: TargetRef,
        preferences: CapabilityPreferences,
        core_config: CoreConfig,
        ledger_config: Option<EffectiveBytesLedgerConfig>,
        request: EffectiveBytesRequest,
    ) -> Result<EffectiveBytesOutcome, EffectiveBytesError> {
        let source = request.content.clone();
        let ledger_scope = LedgerTraceScope {
            attempt_id: None,
            tool_use_id: Some(request.tool_use_id.clone()),
            invocation_id: None,
        };
        let mut runtime = runtime
            .lock()
            .map_err(|_| EffectiveBytesError::RuntimePoisoned)?;
        if runtime.core.is_none() {
            let catalog = ProviderCatalog::discover(provider_source, &provider_admission)?;
            runtime.core = Some(Core::with_config(catalog, core_config)?);
        }
        let core = runtime
            .core
            .as_mut()
            .ok_or(EffectiveBytesError::RuntimeUnavailable)?;
        let context = core.establish_execution_context(SessionContextSpec {
            target,
            environment_id: request.environment_id,
            actor_id: request.actor_id,
            agent_session_id: Some(request.agent_session_id),
            work_id: None,
            attempt_id: None,
            execution_context_id: Some(request.execution_context_id),
        })?;
        let outcome = core.observe_tool_result(
            &context,
            request.turn_id,
            request.tool_use_id,
            ToolResultSubmission {
                content: request.content,
                media_type: request.media_type,
                origin: request.origin,
                tool_name: request.tool_name,
                allow_text_reencoding: true,
            },
            &preferences,
        )?;
        let plan_body = outcome.ledger_body();
        let mut effective = EffectiveBytesOutcome::from_aw_outcome(outcome, &source)?;
        if let Some(ledger_config) = ledger_config.as_ref() {
            match append_ledger_record(
                &mut runtime,
                ledger_config,
                LedgerEventKind::PostToolUsePlan,
                LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
                &plan_body,
                &ledger_scope,
            ) {
                Ok(plan_event_id) => {
                    effective.ledger = EffectiveBytesLedgerState::Ready {
                        plan_event_id,
                        assurance: ledger_config.assurance,
                    };
                }
                Err(error)
                    if ledger_config.assurance == EffectiveBytesLedgerAssurance::BestEffort =>
                {
                    tracing::warn!(error = %error, "AW plan Ledger append degraded; adoption will remain unclaimed");
                    effective.ledger = EffectiveBytesLedgerState::Degraded;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(effective)
    }
}

#[async_trait]
impl EffectiveBytesBoundary for AwEffectiveBytesBoundary {
    async fn prepare(
        &self,
        request: EffectiveBytesRequest,
    ) -> Result<EffectiveBytesOutcome, EffectiveBytesError> {
        let runtime = Arc::clone(&self.runtime);
        let provider_source = self.provider_source.clone();
        let provider_admission = self.provider_admission.clone();
        let target = self.target.clone();
        let preferences = self.preferences.clone();
        let core_config = self.core_config;
        let ledger_config = self.ledger_config.clone();
        tokio::task::spawn_blocking(move || {
            Self::prepare_blocking(
                &runtime,
                provider_source,
                provider_admission,
                target,
                preferences,
                core_config,
                ledger_config,
                request,
            )
        })
        .await
        .map_err(EffectiveBytesError::Join)?
    }

    async fn record_adoption(
        &self,
        body: ContextAdoptionBody,
        scope: LedgerTraceScope,
    ) -> Result<LedgerEventId, EffectiveBytesError> {
        let runtime = Arc::clone(&self.runtime);
        let ledger_config = self
            .ledger_config
            .clone()
            .ok_or(EffectiveBytesError::LedgerNotConfigured)?;
        tokio::task::spawn_blocking(move || {
            let mut runtime = runtime
                .lock()
                .map_err(|_| EffectiveBytesError::RuntimePoisoned)?;
            append_ledger_record(
                &mut runtime,
                &ledger_config,
                LedgerEventKind::ContextAdoption,
                LEDGER_CONTEXT_ADOPTION_SCHEMA,
                &body,
                &scope,
            )
        })
        .await
        .map_err(EffectiveBytesError::Join)?
    }
}

fn append_ledger_record<T: serde::Serialize>(
    runtime: &mut EffectiveBytesRuntime,
    config: &EffectiveBytesLedgerConfig,
    kind: LedgerEventKind,
    schema: &str,
    body: &T,
    scope: &LedgerTraceScope,
) -> Result<LedgerEventId, EffectiveBytesError> {
    if runtime.ledger.is_none() {
        let store =
            LedgerStore::open(&config.root).map_err(|source| EffectiveBytesError::LedgerStore {
                assurance: config.assurance,
                source,
            })?;
        runtime.ledger = Some(LedgerSink::new(store));
    }
    let body =
        serde_json::to_value(body).map_err(|source| EffectiveBytesError::LedgerEncoding {
            assurance: config.assurance,
            source,
        })?;
    let record = runtime
        .ledger
        .as_mut()
        .ok_or(EffectiveBytesError::LedgerNotConfigured)?
        .record(kind, schema, body, Some(scope))
        .map_err(|source| EffectiveBytesError::LedgerAppend {
            assurance: config.assurance,
            source,
        })?;
    Ok(record.header.id)
}

/// Failure returned before COSH can adopt an AW projection.
#[derive(Debug, Error)]
pub enum EffectiveBytesError {
    /// A system-owned setting cannot form a valid AW runtime.
    #[error("invalid AW effective-bytes configuration: {0}")]
    InvalidConfiguration(String),
    /// The system enabled AW, but its trusted boundary could not initialize.
    #[error("system-required AW effective-bytes boundary is unavailable")]
    BoundaryUnavailable,
    /// A bounded AW identity is invalid.
    #[error(transparent)]
    BoundedValue(#[from] aw_contracts::common::BoundedStringError),
    /// Provider discovery, admission, or invocation failed.
    #[error(transparent)]
    ProviderHost(#[from] aw_provider_host::ProviderHostError),
    /// AW Core could not resolve or execute its plan.
    #[error(transparent)]
    Core(#[from] aw_core::CoreError),
    /// Tokio could not complete the blocking AW invocation task.
    #[error("AW effective-bytes worker failed: {0}")]
    Join(#[source] tokio::task::JoinError),
    /// The cached AW runtime lock was poisoned.
    #[error("AW effective-bytes runtime lock was poisoned")]
    RuntimePoisoned,
    /// The cached AW runtime was not available after initialization.
    #[error("AW effective-bytes runtime is unavailable")]
    RuntimeUnavailable,
    /// Candidate canonicalization failed.
    #[error("AW candidate canonicalization failed: {0}")]
    CandidateEncoding(#[source] serde_json::Error),
    /// A typed Ledger body could not be converted to JSON.
    #[error("AW Ledger body encoding failed under {assurance:?} assurance: {source}")]
    LedgerEncoding {
        /// Failure policy attached to the configured writer.
        assurance: EffectiveBytesLedgerAssurance,
        /// Serialization failure.
        #[source]
        source: serde_json::Error,
    },
    /// The configured Ledger store could not be opened.
    #[error("AW Ledger store failed under {assurance:?} assurance: {source}")]
    LedgerStore {
        /// Failure policy attached to the configured writer.
        assurance: EffectiveBytesLedgerAssurance,
        /// Store failure without any model-visible content.
        #[source]
        source: aw_ledger::StoreError,
    },
    /// A typed Ledger record could not be admitted or persisted.
    #[error("AW Ledger append failed under {assurance:?} assurance: {source}")]
    LedgerAppend {
        /// Failure policy attached to the configured writer.
        assurance: EffectiveBytesLedgerAssurance,
        /// Typed writer failure without any model-visible content.
        #[source]
        source: aw_ledger::SinkError,
    },
    /// Adoption recording was requested without a configured Ledger.
    #[error("AW effective-bytes Ledger is not configured")]
    LedgerNotConfigured,
    /// An internally generated SHA-256 value violated the digest type.
    #[error("AW generated a non-canonical digest")]
    InvalidDigest,
    /// AW identified source bytes other than those submitted by COSH.
    #[error("AW source digest does not match COSH provisional bytes")]
    SourceDigestMismatch,
    /// A candidate named another source artifact.
    #[error("AW candidate source artifact does not match the outcome")]
    SourceArtifactMismatch,
    /// A candidate named another source digest.
    #[error("AW candidate source digest does not match the outcome")]
    CandidateSourceDigestMismatch,
    /// The selected representation contradicted the Provider offer.
    #[error("AW effective-byte selection contradicts the Provider candidate")]
    SelectionMismatch,
    /// A passthrough outcome unexpectedly claimed a candidate digest.
    #[error("AW passthrough outcome unexpectedly carries a candidate digest")]
    UnexpectedCandidateDigest,
    /// Candidate identity does not match its canonical body.
    #[error("AW candidate digest does not match the candidate body")]
    CandidateDigestMismatch,
    /// Effective identity does not match the bytes selected for history.
    #[error("AW effective digest does not match the selected bytes")]
    EffectiveDigestMismatch,
    /// The bytes in the COSH history slot differ from the selected bytes.
    #[error("COSH history bytes do not match the AW effective digest")]
    HistoryDigestMismatch,
    /// The history byte count cannot be represented by the Ledger contract.
    #[error("COSH history byte count cannot be represented as u64")]
    HistorySizeOverflow,
}

impl EffectiveBytesError {
    /// Whether this error represents failure of a required durable append.
    #[must_use]
    pub fn is_required_ledger_failure(&self) -> bool {
        matches!(
            self,
            Self::LedgerStore {
                assurance: EffectiveBytesLedgerAssurance::Required,
                ..
            } | Self::LedgerAppend {
                assurance: EffectiveBytesLedgerAssurance::Required,
                ..
            } | Self::LedgerEncoding {
                assurance: EffectiveBytesLedgerAssurance::Required,
                ..
            }
        )
    }
}

fn digest_candidate(candidate: &ContextProjectionCandidate) -> Result<Digest, EffectiveBytesError> {
    #[derive(serde::Serialize)]
    #[serde(deny_unknown_fields)]
    struct CandidateEnvelope<'a> {
        candidate: &'a ContextProjectionCandidate,
    }

    let value = serde_json::to_value(CandidateEnvelope { candidate })
        .map_err(EffectiveBytesError::CandidateEncoding)?;
    let canonical =
        canonical_json_v1_bytes(&value).map_err(EffectiveBytesError::CandidateEncoding)?;
    digest_bytes(&canonical)
}

fn selection_for_candidate(candidate: &ContextProjectionCandidate) -> EffectiveBytesSelection {
    if candidate.content.is_empty() {
        EffectiveBytesSelection::SourceEmptyCandidate
    } else if candidate.reversibility != ContextReversibility::Lossless {
        EffectiveBytesSelection::SourceCandidateNotLossless
    } else {
        EffectiveBytesSelection::Candidate
    }
}

fn digest_bytes(bytes: &[u8]) -> Result<Digest, EffectiveBytesError> {
    Digest::parse(format!("{:x}", Sha256::digest(bytes)))
        .map_err(|_| EffectiveBytesError::InvalidDigest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate_for(source: &str, reversibility: ContextReversibility) -> EffectiveBytesOutcome {
        let source_artifact_id = ArtifactId::new();
        let source_digest = digest_bytes(source.as_bytes()).unwrap();
        let candidate = ContextProjectionCandidate {
            source_artifact_id: source_artifact_id.clone(),
            source_digest: source_digest.clone(),
            content: "prepared".to_owned(),
            media_type: BoundedName::new("text/plain").unwrap(),
            content_type: None,
            transform_chain: vec![BoundedName::new("fixture").unwrap()],
            reversibility,
        };
        EffectiveBytesOutcome {
            source_artifact_id,
            source_digest,
            candidate_digest: Some(digest_candidate(&candidate).unwrap()),
            selection: selection_for_candidate(&candidate),
            effective_digest: if reversibility == ContextReversibility::Lossless {
                digest_bytes(candidate.content.as_bytes()).unwrap()
            } else {
                digest_bytes(source.as_bytes()).unwrap()
            },
            candidate: Some(candidate),
            receipts: Vec::new(),
            ledger: EffectiveBytesLedgerState::Disabled,
        }
    }

    #[test]
    fn selects_only_a_non_empty_lossless_candidate_for_the_exact_source() {
        let source = "provisional result";
        let accepted = candidate_for(source, ContextReversibility::Lossless);
        accepted.validate_for_source(source).unwrap();
        assert_eq!(accepted.selection, EffectiveBytesSelection::Candidate);

        let lossy = candidate_for(source, ContextReversibility::Unrecoverable);
        lossy.validate_for_source(source).unwrap();
        assert_eq!(
            lossy.selection,
            EffectiveBytesSelection::SourceCandidateNotLossless
        );
        assert_eq!(lossy.effective_digest, lossy.source_digest);
        assert!(lossy.selected_candidate().is_none());

        let mut empty = candidate_for(source, ContextReversibility::Lossless);
        empty.candidate.as_mut().unwrap().content.clear();
        empty.candidate_digest = Some(digest_candidate(empty.candidate.as_ref().unwrap()).unwrap());
        empty.selection = EffectiveBytesSelection::SourceEmptyCandidate;
        empty.effective_digest = empty.source_digest.clone();
        empty.validate_for_source(source).unwrap();
        assert!(empty.selected_candidate().is_none());

        assert!(matches!(
            accepted.validate_for_source("different source"),
            Err(EffectiveBytesError::SourceDigestMismatch)
        ));
    }

    #[test]
    fn passthrough_binds_effective_bytes_to_the_source() {
        let source = "unchanged";
        let source_digest = digest_bytes(source.as_bytes()).unwrap();
        let outcome = EffectiveBytesOutcome {
            source_artifact_id: ArtifactId::new(),
            source_digest: source_digest.clone(),
            candidate: None,
            candidate_digest: None,
            selection: EffectiveBytesSelection::SourceNoCandidate,
            effective_digest: source_digest,
            receipts: Vec::new(),
            ledger: EffectiveBytesLedgerState::Disabled,
        };

        outcome.validate_for_source(source).unwrap();
    }

    #[test]
    fn rejected_offers_record_preserved_source_with_candidate_identity() {
        let source = "provisional result";
        let cases = [
            (
                candidate_for(source, ContextReversibility::Retrievable),
                ContextAdoptionReason::CandidateNotLossless,
            ),
            {
                let mut empty = candidate_for(source, ContextReversibility::Lossless);
                empty.candidate.as_mut().unwrap().content.clear();
                empty.candidate_digest =
                    Some(digest_candidate(empty.candidate.as_ref().unwrap()).unwrap());
                empty.selection = EffectiveBytesSelection::SourceEmptyCandidate;
                empty.effective_digest = empty.source_digest.clone();
                (empty, ContextAdoptionReason::EmptyCandidate)
            },
        ];

        for (mut outcome, reason) in cases {
            outcome.ledger = EffectiveBytesLedgerState::Ready {
                plan_event_id: LedgerEventId::new(),
                assurance: EffectiveBytesLedgerAssurance::Required,
            };
            outcome.validate_for_source(source).unwrap();

            let (body, assurance) = outcome
                .adoption_body_for_history(source)
                .unwrap()
                .expect("ready plan produces adoption evidence");

            assert_eq!(assurance, EffectiveBytesLedgerAssurance::Required);
            assert_eq!(body.decision, ContextAdoptionDecision::Preserved);
            assert_eq!(body.reason, reason);
            assert_eq!(body.effective_digest, body.source_digest);
            assert_eq!(
                body.candidate_envelope_digest, outcome.candidate_digest,
                "an unselected offer still retains its content-free identity"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires built Tokenless and agent-sec-core Provider executables"]
    async fn real_aw_catalog_prepares_cosh_effective_bytes() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(4)
            .unwrap()
            .to_path_buf();
        let tokenless = repository.join("src/tokenless/target/debug/tokenless");
        let agent_sec = repository.join("src/agent-sec-core/agent-sec-cli/.venv/bin/agent-sec-cli");
        assert!(
            tokenless.is_file(),
            "build Tokenless before running this test"
        );
        assert!(
            agent_sec.is_file(),
            "create the agent-sec-core virtual environment before running this test"
        );
        let fixture: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                repository
                    .join("providers/tokenless/fixtures/context-projection-prepare-lossless.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let source = fixture
            .pointer("/artifact/content")
            .and_then(serde_json::Value::as_str)
            .unwrap()
            .to_owned();
        let ledger_dir = tempfile::tempdir().unwrap();
        let boundary = AwEffectiveBytesBoundary::new(AwEffectiveBytesConfig {
            provider_root: repository.join("providers"),
            executable_roots: vec![
                repository.join("src/tokenless/target/debug"),
                repository.join("src/agent-sec-core/agent-sec-cli/.venv/bin"),
            ],
            target_identifier: "cosh-effective-bytes-test".to_owned(),
            preferred_projection_provider: Some("tokenless".to_owned()),
            provider_wall_time_ms: 10_000,
            allow_unenforced_providers: true,
            ledger: Some(EffectiveBytesLedgerConfig {
                root: ledger_dir.path().to_path_buf(),
                assurance: EffectiveBytesLedgerAssurance::Required,
            }),
        })
        .unwrap();

        let tool_use_id = ToolUseId::new();
        let outcome = boundary
            .prepare(EffectiveBytesRequest {
                content: source.clone(),
                media_type: BoundedName::new("application/json").unwrap(),
                origin: ContextArtifactOrigin::ApiResponse,
                tool_name: Some(BoundedName::new("list_recent_builds").unwrap()),
                environment_id: EnvironmentId::new(),
                execution_context_id: ExecutionContextId::new(),
                actor_id: ActorId::new(),
                agent_session_id: AgentSessionId::new(),
                turn_id: TurnId::new(),
                tool_use_id: tool_use_id.clone(),
            })
            .await
            .unwrap();

        outcome.validate_for_source(&source).unwrap();
        let candidate = outcome.candidate.as_ref().expect("Tokenless candidate");
        assert_ne!(candidate.content, source);
        assert_eq!(outcome.receipts.len(), 3);
        assert_eq!(
            outcome.receipts.last().unwrap().provider_id.as_str(),
            "tokenless"
        );

        let (body, assurance) = outcome
            .adoption_body_for_history(&candidate.content)
            .unwrap()
            .expect("required Ledger plan exists");
        assert_eq!(assurance, EffectiveBytesLedgerAssurance::Required);
        boundary
            .record_adoption(
                body,
                LedgerTraceScope {
                    attempt_id: None,
                    tool_use_id: Some(tool_use_id),
                    invocation_id: None,
                },
            )
            .await
            .unwrap();

        let store = LedgerStore::open(ledger_dir.path()).unwrap();
        assert_eq!(aw_ledger::verify_chain(&store).unwrap(), 2);
        let plans = store
            .events_by_kind(LedgerEventKind::PostToolUsePlan)
            .unwrap();
        let adoptions = store
            .events_by_kind(LedgerEventKind::ContextAdoption)
            .unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(adoptions.len(), 1);
        let plan_bytes = store.record_body_bytes(&plans[0].header.id).unwrap();
        let plan_json = String::from_utf8(plan_bytes).unwrap();
        let adoption_bytes = store.record_body_bytes(&adoptions[0].header.id).unwrap();
        let adoption_json = String::from_utf8(adoption_bytes).unwrap();
        assert!(!plan_json.contains(&source));
        assert!(!plan_json.contains(&candidate.content));
        assert!(!adoption_json.contains(&source));
        assert!(!adoption_json.contains(&candidate.content));
    }
}
