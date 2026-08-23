//! Replaceable, synchronous access to provider-owned knowledge.
//!
//! Providers return bounded excerpts and stable references. They never transfer
//! ownership of full manuals or other source documents to Agent Memory.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::protocol::KnowledgeRef;

pub mod mant;

/// Maximum UTF-8 size accepted for a document identity or focused selector.
pub const MAX_KNOWLEDGE_INPUT_BYTES: usize = 4 * 1024;
/// Maximum number of selectors accepted by one focused excerpt request.
pub const MAX_KNOWLEDGE_SELECTORS: usize = 64;
/// Maximum excerpt size that any provider query may request.
pub const MAX_KNOWLEDGE_EXCERPT_BYTES: usize = 64 * 1024;
/// Maximum number of results that any provider query may request.
pub const MAX_KNOWLEDGE_ITEMS: u16 = 64;

/// Result returned by provider-neutral knowledge operations.
pub type KnowledgeResult<T> = Result<T, KnowledgeError>;

/// Synchronous boundary implemented by optional provider-owned knowledge sources.
pub trait KnowledgeProvider: Send + Sync {
    /// Returns the provider's live identity and supported focused operations.
    ///
    /// # Errors
    ///
    /// Returns a typed availability, compatibility, timeout, or protocol error
    /// when a live descriptor cannot be obtained.
    fn descriptor(&self) -> KnowledgeResult<KnowledgeProviderDescriptor>;

    /// Returns a fail-safe health snapshot without converting degradation into
    /// a transport error.
    fn health(&self) -> KnowledgeProviderHealth {
        match self.descriptor() {
            Ok(descriptor) => KnowledgeProviderHealth::healthy(descriptor),
            Err(error) => KnowledgeProviderHealth::degraded(error),
        }
    }

    /// Resolves one focused query into bounded excerpts and external refs.
    ///
    /// # Errors
    ///
    /// Returns [`KnowledgeErrorCode::InvalidRequest`] for an invalid or
    /// over-budget query and typed provider errors for runtime failures.
    fn query(&self, query: &KnowledgeQuery) -> KnowledgeResult<Vec<KnowledgeItem>>;
}

/// Live provider identity used for capability negotiation and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeProviderDescriptor {
    /// Stable, implementation-neutral provider identity.
    pub provider_id: String,
    /// Human-readable provider name for diagnostics.
    pub display_name: String,
    /// Provider implementation version reported by the live endpoint.
    pub version: Option<String>,
    /// Versioned wire protocol reported by the live endpoint.
    pub protocol: Option<String>,
    /// Focused operations proven by capability negotiation.
    pub capabilities: Vec<KnowledgeCapability>,
}

/// Focused operation a knowledge provider can execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeCapability {
    /// Searches within an explicitly named document.
    Search,
    /// Explains one explicitly named entry.
    Explain,
    /// Extracts explicitly named sections.
    Excerpt,
}

/// Health state suitable for fail-open runtime admission decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeProviderHealth {
    /// Healthy only when live capability negotiation succeeds.
    pub status: KnowledgeHealthStatus,
    /// Millisecond wall-clock time at which this snapshot was produced.
    pub checked_at_ms: u64,
    /// Live descriptor when negotiation succeeded.
    pub descriptor: Option<KnowledgeProviderDescriptor>,
    /// Typed degradation reason when negotiation failed.
    pub error: Option<KnowledgeError>,
}

impl KnowledgeProviderHealth {
    /// Builds a healthy snapshot backed by a live descriptor.
    pub fn healthy(descriptor: KnowledgeProviderDescriptor) -> Self {
        Self {
            status: KnowledgeHealthStatus::Healthy,
            checked_at_ms: now_ms(),
            descriptor: Some(descriptor),
            error: None,
        }
    }

    /// Builds a degraded snapshot while preserving the typed failure reason.
    pub fn degraded(error: KnowledgeError) -> Self {
        Self {
            status: KnowledgeHealthStatus::Degraded,
            checked_at_ms: now_ms(),
            descriptor: None,
            error: Some(error),
        }
    }
}

/// Coarse provider state; unavailable providers are degraded, never healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeHealthStatus {
    /// Live negotiation succeeded and returned a compatible descriptor.
    Healthy,
    /// The provider cannot safely serve the configured contract.
    Degraded,
}

/// Bounded, focused request against one provider-owned document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeQuery {
    /// Provider-defined canonical document identity.
    pub document_id: String,
    /// Focused operation; a whole-document operation is intentionally absent.
    pub selector: KnowledgeSelector,
    /// Maximum UTF-8 bytes admitted into each returned excerpt.
    pub max_excerpt_bytes: usize,
    /// Maximum result count the provider may return.
    pub max_items: u16,
}

impl KnowledgeQuery {
    /// Validates input and result budgets before any provider is invoked.
    ///
    /// # Errors
    ///
    /// Returns [`KnowledgeErrorCode::InvalidRequest`] when required input is
    /// empty, and [`KnowledgeErrorCode::ResourceExhausted`] when a bound is
    /// exceeded.
    pub fn validate(&self) -> KnowledgeResult<()> {
        validate_nonempty("document identity", &self.document_id)?;
        if self.max_excerpt_bytes == 0 {
            return Err(KnowledgeError::invalid_request(
                "excerpt budget must be greater than zero",
            ));
        }
        if self.max_excerpt_bytes > MAX_KNOWLEDGE_EXCERPT_BYTES {
            return Err(KnowledgeError::resource_exhausted(
                "excerpt budget exceeds the provider boundary",
            ));
        }
        if self.max_items == 0 {
            return Err(KnowledgeError::invalid_request(
                "result budget must be greater than zero",
            ));
        }
        if self.max_items > MAX_KNOWLEDGE_ITEMS {
            return Err(KnowledgeError::resource_exhausted(
                "result budget exceeds the provider boundary",
            ));
        }

        match &self.selector {
            KnowledgeSelector::Search { pattern, .. } => {
                validate_nonempty("search pattern", pattern)?;
            }
            KnowledgeSelector::Explain { entry } => {
                validate_nonempty("explain entry", entry)?;
            }
            KnowledgeSelector::Excerpt { selectors } => {
                if selectors.is_empty() {
                    return Err(KnowledgeError::invalid_request(
                        "at least one excerpt selector is required",
                    ));
                }
                if selectors.len() > MAX_KNOWLEDGE_SELECTORS {
                    return Err(KnowledgeError::resource_exhausted(
                        "too many excerpt selectors",
                    ));
                }
                for selector in selectors {
                    validate_nonempty("excerpt selector", selector)?;
                }
            }
        }
        if self.reference_selector().len() > MAX_KNOWLEDGE_INPUT_BYTES {
            return Err(KnowledgeError::resource_exhausted(
                "combined knowledge selector exceeds the input boundary",
            ));
        }
        Ok(())
    }

    /// Returns the capability required to execute this query.
    pub fn required_capability(&self) -> KnowledgeCapability {
        match self.selector {
            KnowledgeSelector::Search { .. } => KnowledgeCapability::Search,
            KnowledgeSelector::Explain { .. } => KnowledgeCapability::Explain,
            KnowledgeSelector::Excerpt { .. } => KnowledgeCapability::Excerpt,
        }
    }

    /// Returns a bounded provider selector suitable for [`KnowledgeRef`].
    pub fn reference_selector(&self) -> String {
        match &self.selector {
            KnowledgeSelector::Search { pattern, .. } => format!("search:{pattern}"),
            KnowledgeSelector::Explain { entry } => format!("explain:{entry}"),
            KnowledgeSelector::Excerpt { selectors } => {
                format!("excerpt:{}", selectors.join(","))
            }
        }
    }
}

/// Focused view supported by the provider-neutral query boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum KnowledgeSelector {
    /// Searches for a bounded pattern within one document.
    Search {
        /// Literal search text; adapters must not interpret it as shell input.
        pattern: String,
        /// Number of adjacent source lines requested for each match.
        context_lines: u8,
    },
    /// Explains one named entry such as a command option.
    Explain {
        /// Provider-defined entry identity.
        entry: String,
    },
    /// Extracts one or more named sections.
    Excerpt {
        /// Provider-defined section selectors.
        selectors: Vec<String>,
    },
}

/// One bounded knowledge result with attribution and a change fingerprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeItem {
    /// External ownership and retrieval provenance.
    pub reference: KnowledgeRef,
    /// Optional bounded display metadata supplied by the provider.
    pub title: Option<String>,
    /// Focused, bounded UTF-8 excerpt; never a complete source document.
    pub excerpt: String,
    /// Stable change detector for the focused provider response.
    pub fingerprint: String,
    /// Optional provider relevance score in the inclusive range zero to one.
    pub score: Option<f32>,
}

/// Machine-actionable error returned by a knowledge provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeError {
    /// Stable category used for health and retry policy.
    pub code: KnowledgeErrorCode,
    /// Bounded, content-free diagnostic safe for logs.
    pub safe_message: String,
    /// Whether retrying without changing the request can reasonably succeed.
    pub retryable: bool,
}

impl KnowledgeError {
    /// Creates a typed error with a caller-supplied safe diagnostic.
    pub fn new(code: KnowledgeErrorCode, safe_message: impl Into<String>, retryable: bool) -> Self {
        let mut safe_message = safe_message.into();
        if safe_message.len() > 512 {
            safe_message.truncate(floor_char_boundary(&safe_message, 512));
        }
        Self {
            code,
            safe_message,
            retryable,
        }
    }

    /// Creates an invalid request error.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(KnowledgeErrorCode::InvalidRequest, message, false)
    }

    /// Creates a resource boundary error.
    pub fn resource_exhausted(message: impl Into<String>) -> Self {
        Self::new(KnowledgeErrorCode::ResourceExhausted, message, false)
    }
}

impl fmt::Display for KnowledgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.safe_message)
    }
}

impl std::error::Error for KnowledgeError {}

/// Stable error categories independent of a particular provider transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeErrorCode {
    /// The optional provider executable or service is absent.
    Unavailable,
    /// The live provider does not implement the required protocol contract.
    Incompatible,
    /// The provider exceeded its hard execution deadline.
    Timeout,
    /// The focused query is empty or otherwise invalid.
    InvalidRequest,
    /// Input, output, result, or excerpt bounds were exceeded.
    ResourceExhausted,
    /// The provider reported an operational failure.
    ProviderFailed,
    /// Provider output was not valid for the negotiated schema.
    MalformedResponse,
    /// The adapter itself encountered an invariant or I/O failure.
    Internal,
}

impl fmt::Display for KnowledgeErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Unavailable => "unavailable",
            Self::Incompatible => "incompatible",
            Self::Timeout => "timeout",
            Self::InvalidRequest => "invalid_request",
            Self::ResourceExhausted => "resource_exhausted",
            Self::ProviderFailed => "provider_failed",
            Self::MalformedResponse => "malformed_response",
            Self::Internal => "internal",
        };
        formatter.write_str(value)
    }
}

fn validate_nonempty(label: &str, value: &str) -> KnowledgeResult<()> {
    if value.trim().is_empty() {
        return Err(KnowledgeError::invalid_request(format!(
            "{label} must not be empty"
        )));
    }
    if value.len() > MAX_KNOWLEDGE_INPUT_BYTES {
        return Err(KnowledgeError::resource_exhausted(format!(
            "{label} exceeds the input boundary"
        )));
    }
    Ok(())
}

pub(crate) fn floor_char_boundary(value: &str, maximum: usize) -> usize {
    if value.len() <= maximum {
        return value.len();
    }
    let mut boundary = maximum;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}
