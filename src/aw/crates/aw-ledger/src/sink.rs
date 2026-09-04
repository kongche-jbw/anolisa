//! Atomic Ledger append orchestration.
//!
//! The `LedgerSink` coordinates the four steps that every boundary
//! recorder (B6–B8) needs: allocate a record ID, read the wall clock,
//! build a candidate record from the current chain tip, admit it, and
//! append the admitted bytes to the store. Callers provide the event
//! kind, schema, body, and optional trace scope; the sink handles
//! sequencing, parent linking, body digest computation, and admission
//! validation.

use std::time::{SystemTime, UNIX_EPOCH};

use aw_contracts::ids::LedgerEventId;
use aw_contracts::ledger::{
    LedgerEventKind, LedgerParent, LedgerRecordHeader, LedgerTraceScope, PostToolUsePlanBody,
    PreToolUseGateBody, LEDGER_POST_TOOL_USE_PLAN_SCHEMA, LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
};
use serde_json::Value;
use thiserror::Error;

use crate::admission::AdmissionError;
use crate::store::StoreError;
use crate::{admit, AdmittedRecord, CandidateRecord, ChainTip, LedgerStore};

/// Failure returned by [`LedgerSink::record`].
#[derive(Debug, Error)]
pub enum SinkError {
    /// Admission rejected the candidate.
    #[error("ledger admission rejected: {0}")]
    Admission(#[from] AdmissionError),
    /// The generic sink has no typed writer for this taxonomy entry yet.
    #[error("ledger event kind {kind:?} has no implemented typed writer")]
    UnsupportedEventKind {
        /// Event kind for which no writer contract exists.
        kind: LedgerEventKind,
    },
    /// The event kind was paired with a different body schema.
    #[error("ledger event kind {kind:?} requires schema {expected}, got {actual}")]
    SchemaMismatch {
        /// Event kind whose schema did not match.
        kind: LedgerEventKind,
        /// Schema implemented by this writer.
        expected: &'static str,
        /// Schema supplied by the caller.
        actual: String,
    },
    /// The body did not conform to the implemented typed schema.
    #[error("ledger event kind {kind:?} has an invalid typed body: {source}")]
    InvalidBody {
        /// Event kind whose body failed decoding.
        kind: LedgerEventKind,
        /// Strict typed decoding failure.
        #[source]
        source: serde_json::Error,
    },
    /// The backing store could not persist the record.
    #[error("ledger store error: {0}")]
    Store(#[from] StoreError),
}

/// Coordinates admission and persistence for one Ledger append.
///
/// A boundary recorder allocates one sink per logical writer (e.g. one
/// per hook invocation) and calls [`Self::record`] for each event. The
/// sink tracks the chain tip internally so successive calls produce a
/// continuous hash chain without the caller managing sequence numbers
/// or parent links.
pub struct LedgerSink {
    store: LedgerStore,
}

impl LedgerSink {
    /// Wraps `store` as the persistence backend. The sink reads the
    /// current chain tip from the store so the first call to
    /// [`Self::record`] produces the correct next record.
    pub fn new(store: LedgerStore) -> Self {
        Self { store }
    }

    /// Admits and persists one record, returning the admitted bytes and
    /// digests.
    ///
    /// # Errors
    ///
    /// Returns [`SinkError::Admission`] when the body violates
    /// content-freedom or any other admission invariant, and
    /// [`SinkError::Store`] when the database refuses the write.
    pub fn record(
        &mut self,
        kind: LedgerEventKind,
        schema: &str,
        body: Value,
        scope: Option<&LedgerTraceScope>,
    ) -> Result<AdmittedRecord, SinkError> {
        let tip = self.store.tip();
        let candidate = build_candidate(&tip, kind, schema, body, scope);
        let admitted = admit(&tip, candidate)?;
        validate_writer_body(kind, schema, &admitted.body)?;
        self.store.append(&admitted)?;
        Ok(admitted)
    }

    /// Read-only snapshot of the current chain tip.
    pub fn tip(&self) -> ChainTip<'_> {
        self.store.tip()
    }
}

fn build_candidate(
    tip: &ChainTip<'_>,
    kind: LedgerEventKind,
    schema: &str,
    body: Value,
    scope: Option<&LedgerTraceScope>,
) -> CandidateRecord {
    use aw_contracts::canonical::canonical_json_v1_bytes;
    use sha2::{Digest as _, Sha256};

    let body_canonical = canonical_json_v1_bytes(&body).expect("body canonical");
    let body_digest_hex = format!("{:x}", Sha256::digest(&body_canonical));
    let body_digest = aw_contracts::common::Digest::parse(body_digest_hex)
        .expect("sha2 output is always a valid digest");

    let sequence = if tip.id.is_none() {
        0
    } else {
        tip.sequence + 1
    };
    let parent = tip.id.zip(tip.digest).map(|(id, digest)| LedgerParent {
        id: id.clone(),
        digest: digest.clone(),
    });

    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis() as u64;

    CandidateRecord {
        header: LedgerRecordHeader {
            id: LedgerEventId::new(),
            sequence,
            timestamp_ms,
            kind,
            schema: schema.to_owned(),
            scope: scope.cloned(),
            parent,
            body_digest,
        },
        body,
    }
}

fn validate_writer_body(
    kind: LedgerEventKind,
    schema: &str,
    body: &Value,
) -> Result<(), SinkError> {
    let expected = match kind {
        LedgerEventKind::PostToolUsePlan => LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
        LedgerEventKind::PreToolUseGate => LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
        kind => return Err(SinkError::UnsupportedEventKind { kind }),
    };
    if schema != expected {
        return Err(SinkError::SchemaMismatch {
            kind,
            expected,
            actual: schema.to_owned(),
        });
    }

    match kind {
        LedgerEventKind::PostToolUsePlan => {
            serde_json::from_value::<PostToolUsePlanBody>(body.clone())
                .map_err(|source| SinkError::InvalidBody { kind, source })?;
        }
        LedgerEventKind::PreToolUseGate => {
            serde_json::from_value::<PreToolUseGateBody>(body.clone())
                .map_err(|source| SinkError::InvalidBody { kind, source })?;
        }
        _ => unreachable!("unsupported event kinds returned above"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aw_contracts::ids::AttemptId;
    use serde_json::json;
    use tempfile::tempdir;

    fn open_sink() -> (LedgerSink, tempfile::TempDir) {
        let dir = tempdir().expect("temp dir");
        let store = LedgerStore::open(dir.path()).expect("store opens");
        (LedgerSink::new(store), dir)
    }

    fn clean_body() -> Value {
        json!({
            "gate": "not_mediated",
            "reasons": [],
            "degradation": "no_implementation"
        })
    }

    #[test]
    fn record_produces_a_genesis_event() {
        let (mut sink, _dir) = open_sink();
        let admitted = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
                clean_body(),
                None,
            )
            .expect("genesis recorded");
        assert_eq!(admitted.header.sequence, 0);
        assert_eq!(admitted.header.kind, LedgerEventKind::PreToolUseGate);
        assert!(admitted.header.parent.is_none());
    }

    #[test]
    fn successive_records_form_a_continuous_chain() {
        let (mut sink, _dir) = open_sink();
        let first = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
                clean_body(),
                None,
            )
            .expect("first recorded");
        let second = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
                clean_body(),
                None,
            )
            .expect("second recorded");

        assert_eq!(second.header.sequence, 1);
        let parent = second.header.parent.as_ref().expect("parent present");
        assert_eq!(parent.id, first.header.id);
        assert_eq!(parent.digest, first.record_digest);
    }

    #[test]
    fn content_freedom_is_enforced_at_the_sink() {
        let (mut sink, _dir) = open_sink();
        let bad_body = json!({"command": "rm -rf /"});
        let error = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                "aw.ledger.pre_tool_use_gate/v1",
                bad_body,
                None,
            )
            .unwrap_err();
        assert!(
            matches!(
                error,
                SinkError::Admission(AdmissionError::ContentForbidden { .. })
            ),
            "expected ContentForbidden, got {error:?}"
        );
    }

    #[test]
    fn scope_travels_with_the_record() {
        let (mut sink, _dir) = open_sink();
        let attempt_id = AttemptId::new();
        let scope = LedgerTraceScope {
            attempt_id: Some(attempt_id.clone()),
            tool_use_id: None,
            invocation_id: None,
        };
        sink.record(
            LedgerEventKind::PreToolUseGate,
            LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
            clean_body(),
            Some(&scope),
        )
        .expect("recorded with scope");

        // Verify the tip advanced.
        assert_eq!(sink.tip().sequence, 0);
        assert!(sink.tip().id.is_some());
    }

    #[test]
    fn writer_rejects_kind_schema_mismatch() {
        let (mut sink, _dir) = open_sink();
        let error = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                LEDGER_POST_TOOL_USE_PLAN_SCHEMA,
                clean_body(),
                None,
            )
            .unwrap_err();
        assert!(matches!(error, SinkError::SchemaMismatch { .. }));
        assert!(sink.tip().id.is_none());
    }

    #[test]
    fn writer_rejects_unknown_body_fields() {
        let (mut sink, _dir) = open_sink();
        let mut body = clean_body();
        body.as_object_mut()
            .expect("fixture is an object")
            .insert("note".to_owned(), json!("provider text"));
        let error = sink
            .record(
                LedgerEventKind::PreToolUseGate,
                LEDGER_PRE_TOOL_USE_GATE_SCHEMA,
                body,
                None,
            )
            .unwrap_err();
        assert!(matches!(error, SinkError::InvalidBody { .. }));
        assert!(sink.tip().id.is_none());
    }

    #[test]
    fn writer_rejects_taxonomy_without_an_implemented_contract() {
        let (mut sink, _dir) = open_sink();
        let error = sink
            .record(
                LedgerEventKind::EvidenceStored,
                "aw.ledger.evidence_stored/v1",
                clean_body(),
                None,
            )
            .unwrap_err();
        assert!(matches!(error, SinkError::UnsupportedEventKind { .. }));
        assert!(sink.tip().id.is_none());
    }
}
