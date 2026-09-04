//! Bounded Ledger queries.
//!
//! Readers filter the Ledger by event kind, trace scope, or record
//! identity. Every query is bounded: it scans at most one table using
//! an indexed column and returns at most the rows the index selects.
//! No query touches the body blob; the returned [`StoredRecord`] carries
//! the header, the body digest, and the trace scope — enough to decide
//! whether to fetch the full canonical bytes through [`Self::record_by_id`].

use aw_contracts::common::Digest;
use aw_contracts::ids::LedgerEventId;
use aw_contracts::ledger::{LedgerEventKind, LedgerParent, LedgerRecordHeader, LedgerTraceScope};
use rusqlite::params;

use crate::store::{kind_to_str, LedgerStore, StoreError};

/// One record read back from the store.
///
/// Carries indexed copies of the committed header fields and trace scope,
/// plus the record digest. Call [`crate::verify_chain`] before treating those
/// copies as authoritative. The body bytes are intentionally absent — a
/// reader that needs them calls
/// [`LedgerStore::record_body_bytes`] with the record ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRecord {
    /// Header reconstructed from denormalized query columns.
    pub header: LedgerRecordHeader,
    /// Scope index copy, also reflected in [`Self::header`].
    pub scope: Option<LedgerTraceScope>,
    /// Digest of the canonical record bytes — the value the next record's
    /// parent link commits to.
    pub record_digest: Digest,
}

impl LedgerStore {
    /// Returns the record with the given identity, or `None` when absent.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] when the query fails.
    pub fn record_by_id(&self, id: &LedgerEventId) -> Result<Option<StoredRecord>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT r.id, r.sequence, r.timestamp_ms, r.kind, r.schema,
                    r.parent_id, r.parent_digest, r.body_digest, r.record_digest,
                    s.record_id, s.attempt_id, s.tool_use_id, s.invocation_id
             FROM ledger_records r
             LEFT JOIN ledger_scope s ON s.record_id = r.id
             WHERE r.id = ?1",
        )?;
        let mut rows = stmt.query(params![id.as_str()])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_stored_record(row)?)),
            None => Ok(None),
        }
    }

    /// Returns every record of the given kind, ordered by sequence ascending.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] when the query fails.
    pub fn events_by_kind(&self, kind: LedgerEventKind) -> Result<Vec<StoredRecord>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT r.id, r.sequence, r.timestamp_ms, r.kind, r.schema,
                    r.parent_id, r.parent_digest, r.body_digest, r.record_digest,
                    s.record_id, s.attempt_id, s.tool_use_id, s.invocation_id
             FROM ledger_records r
             LEFT JOIN ledger_scope s ON s.record_id = r.id
             WHERE r.kind = ?1
             ORDER BY r.sequence ASC",
        )?;
        let rows = stmt.query_map(params![kind_to_str(kind)], row_to_stored_record)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Returns every record scoped to the given attempt, ordered by
    /// sequence ascending.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] when the query fails.
    pub fn events_for_attempt(
        &self,
        attempt_id: &aw_contracts::ids::AttemptId,
    ) -> Result<Vec<StoredRecord>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT r.id, r.sequence, r.timestamp_ms, r.kind, r.schema,
                    r.parent_id, r.parent_digest, r.body_digest, r.record_digest,
                    s.record_id, s.attempt_id, s.tool_use_id, s.invocation_id
             FROM ledger_records r
             INNER JOIN ledger_scope s ON s.record_id = r.id
             WHERE s.attempt_id = ?1
             ORDER BY r.sequence ASC",
        )?;
        let rows = stmt.query_map(params![attempt_id.as_str()], row_to_stored_record)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Returns the canonical body bytes of one record.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] when the query fails or the
    /// record does not exist.
    pub fn record_body_bytes(&self, id: &LedgerEventId) -> Result<Vec<u8>, StoreError> {
        self.conn()
            .query_row(
                "SELECT body_canonical FROM ledger_records WHERE id = ?1",
                params![id.as_str()],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }
}

fn row_to_stored_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRecord> {
    let id = LedgerEventId::parse(row.get::<_, String>(0)?)
        .expect("stored IDs are canonical by construction");
    let sequence: i64 = row.get(1)?;
    let timestamp_ms: i64 = row.get(2)?;
    let kind_str: String = row.get(3)?;
    let schema: String = row.get(4)?;
    let parent_id: Option<String> = row.get(5)?;
    let parent_digest: Option<String> = row.get(6)?;
    let body_digest_str: String = row.get(7)?;
    let record_digest_str: String = row.get(8)?;
    let scope_record_id: Option<String> = row.get(9)?;
    let attempt_id: Option<String> = row.get(10)?;
    let tool_use_id: Option<String> = row.get(11)?;
    let invocation_id: Option<String> = row.get(12)?;

    let kind = parse_kind(&kind_str).expect("stored kind strings are canonical by construction");
    let body_digest =
        Digest::parse(body_digest_str).expect("stored digests are canonical by construction");
    let record_digest =
        Digest::parse(record_digest_str).expect("stored digests are canonical by construction");

    let parent = parent_id.zip(parent_digest).map(|(pid, pd)| LedgerParent {
        id: LedgerEventId::parse(pid).expect("stored parent IDs are canonical by construction"),
        digest: Digest::parse(pd).expect("stored parent digests are canonical by construction"),
    });

    let scope = scope_record_id.map(|_| LedgerTraceScope {
        attempt_id: attempt_id.map(|s| {
            aw_contracts::ids::AttemptId::parse(s)
                .expect("stored attempt IDs are canonical by construction")
        }),
        tool_use_id: tool_use_id.map(|s| {
            aw_contracts::ids::ToolUseId::parse(s)
                .expect("stored tool use IDs are canonical by construction")
        }),
        invocation_id: invocation_id.map(|s| {
            aw_contracts::ids::ProviderInvocationId::parse(s)
                .expect("stored invocation IDs are canonical by construction")
        }),
    });

    Ok(StoredRecord {
        header: LedgerRecordHeader {
            id,
            sequence: sequence as u64,
            timestamp_ms: timestamp_ms as u64,
            kind,
            schema,
            scope: scope.clone(),
            parent,
            body_digest,
        },
        scope,
        record_digest,
    })
}

fn parse_kind(s: &str) -> Option<LedgerEventKind> {
    match s {
        "post_tool_use_plan" => Some(LedgerEventKind::PostToolUsePlan),
        "pre_tool_use_gate" => Some(LedgerEventKind::PreToolUseGate),
        "provider_invoked" => Some(LedgerEventKind::ProviderInvoked),
        "evidence_stored" => Some(LedgerEventKind::EvidenceStored),
        "receipt_stored" => Some(LedgerEventKind::ReceiptStored),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Chain, LedgerStore};
    use aw_contracts::ids::{AttemptId, ToolUseId};
    use serde_json::json;
    use tempfile::tempdir;

    fn append_records(
        store: &mut LedgerStore,
        chain: &mut Chain,
        count: usize,
    ) -> Vec<crate::AdmittedRecord> {
        let mut records = Vec::new();
        for _ in 0..count {
            let body = json!({
                "projection": {
                    "id": "prj_00000000-0000-0000-0000-000000000000",
                    "digest": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                }
            });
            let tip = chain.tip();
            let candidate = crate::tests::candidate(&tip, body);
            let admitted = crate::admit(&tip, candidate).expect("admit");
            store.append(&admitted).expect("append");
            chain.extend(&admitted);
            records.push(admitted);
        }
        records
    }

    #[test]
    fn record_by_id_returns_stored_record() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        let records = append_records(&mut store, &mut chain, 1);

        let stored = store
            .record_by_id(&records[0].header.id)
            .expect("query succeeds")
            .expect("record present");
        assert_eq!(stored.header.id, records[0].header.id);
        assert_eq!(stored.header.sequence, 0);
        assert_eq!(stored.record_digest, records[0].record_digest);
    }

    #[test]
    fn record_by_id_returns_none_when_absent() {
        let dir = tempdir().expect("temp dir");
        let store = LedgerStore::open(dir.path()).expect("store opens");
        let missing = LedgerEventId::new();
        assert!(store
            .record_by_id(&missing)
            .expect("query succeeds")
            .is_none());
    }

    #[test]
    fn events_by_kind_filters_correctly() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        let _records = append_records(&mut store, &mut chain, 3);

        // The candidate helper uses EvidenceStored as the default kind.
        let evidence_events = store
            .events_by_kind(LedgerEventKind::EvidenceStored)
            .expect("query succeeds");
        assert_eq!(evidence_events.len(), 3);
        for (i, stored) in evidence_events.iter().enumerate() {
            assert_eq!(stored.header.sequence, i as u64);
        }

        let plan_events = store
            .events_by_kind(LedgerEventKind::PostToolUsePlan)
            .expect("query succeeds");
        assert!(plan_events.is_empty());
    }

    #[test]
    fn events_for_attempt_uses_scope_index() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        let attempt = AttemptId::new();
        let tool_use = ToolUseId::new();

        // First record: scoped to `attempt`.
        let body = json!({"evidence": {"id": "evd_00000000-0000-0000-0000-000000000000", "digest": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}});
        let tip = chain.tip();
        let scope = LedgerTraceScope {
            attempt_id: Some(attempt.clone()),
            tool_use_id: Some(tool_use),
            invocation_id: None,
        };
        let mut candidate = crate::tests::candidate(&tip, body);
        candidate.header.scope = Some(scope);
        let admitted = crate::admit(&tip, candidate).expect("admit");
        store.append(&admitted).expect("append");
        chain.extend(&admitted);

        // Second record: no scope.
        let body2 = json!({"note": "unscoped"});
        let tip2 = chain.tip();
        let candidate2 = crate::tests::candidate(&tip2, body2);
        let admitted2 = crate::admit(&tip2, candidate2).expect("admit");
        store.append(&admitted2).expect("append");
        chain.extend(&admitted2);

        let results = store.events_for_attempt(&attempt).expect("query succeeds");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].header.id, admitted.header.id);
        let scope_out = results[0].scope.as_ref().expect("scope present");
        assert_eq!(scope_out.attempt_id.as_ref(), Some(&attempt));
    }

    #[test]
    fn record_body_bytes_returns_canonical_body() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        let records = append_records(&mut store, &mut chain, 1);

        let body_bytes = store
            .record_body_bytes(&records[0].header.id)
            .expect("body bytes");
        assert_eq!(body_bytes, records[0].body_canonical);
    }
}
