//! Persistent SQLite backend for the Ledger.
//!
//! The store owns one SQLite connection opened in WAL mode. Each append
//! runs inside an `IMMEDIATE` transaction that inserts one row into the
//! records table and one into the scope side table, then advances the
//! in-memory chain tip. Reopening an existing database reconstructs the
//! chain tip from the highest-sequence row so the next candidate can be
//! validated without replaying the full chain.

use std::path::{Path, PathBuf};

use aw_contracts::ids::LedgerEventId;
use rusqlite::Connection;
use thiserror::Error;

use crate::{scope, AdmittedRecord, Chain, ChainTip};

/// Failure returned by Ledger store operations.
#[derive(Debug, Error)]
pub enum StoreError {
    /// A rusqlite operation failed.
    #[error("ledger database error: {0}")]
    Database(#[from] rusqlite::Error),
    /// The filesystem refused to create the store directory.
    #[error("ledger store directory could not be created: {0}")]
    Io(#[from] std::io::Error),
}

/// Persistent, append-only Ledger store backed by SQLite.
///
/// One process holds the writer connection at a time; concurrent readers
/// use WAL mode so they do not block the writer. The in-memory chain tip
/// tracks the last record so the next candidate can be validated without
/// a round trip to disk.
pub struct LedgerStore {
    conn: Connection,
    chain: Chain,
    path: PathBuf,
}

impl LedgerStore {
    /// Opens the store rooted at `data_root/ledger.db`, creating the
    /// directory and schema on first use. The returned store holds the
    /// writer connection; its chain tip reflects every record already
    /// persisted.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] when the directory cannot be created
    /// and [`StoreError::Database`] when the connection or migration
    /// fails.
    pub fn open(data_root: &Path) -> Result<Self, StoreError> {
        std::fs::create_dir_all(data_root)?;
        let path = data_root.join("ledger.db");
        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        crate::migration::run(&conn)?;
        let chain = load_chain_tip(&conn)?;
        Ok(Self { conn, chain, path })
    }

    /// Persists one admitted record inside an immediate transaction.
    ///
    /// The UNIQUE constraint on `sequence` rejects duplicates; the
    /// in-memory chain tip advances only after the transaction commits,
    /// so a failed append leaves the chain unchanged.
    ///
    /// # Errors
    ///
    /// The scope index is derived only from the committed record header.
    /// Returns [`StoreError::Database`] when the insert or derived scope write
    /// fails (including duplicate-sequence rejection).
    pub(crate) fn append(&mut self, record: &AdmittedRecord) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        insert_record(&tx, record)?;
        scope::insert(&tx, record.header.id.as_str(), record.header.scope.as_ref())?;
        tx.commit()?;
        self.chain.extend(record);
        Ok(())
    }

    /// Read-only snapshot of the current chain tip.
    pub fn tip(&self) -> ChainTip<'_> {
        self.chain.tip()
    }

    /// Filesystem path of the database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read-only access to the underlying connection for crate-internal
    /// modules that need direct SQL access (query, verify).
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}

fn load_chain_tip(conn: &Connection) -> Result<Chain, StoreError> {
    let mut chain = Chain::new();
    let mut stmt = conn.prepare(
        "SELECT id, sequence, record_digest
         FROM ledger_records
         ORDER BY sequence DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query([])?;
    if let Some(row) = rows.next()? {
        let id = LedgerEventId::parse(row.get::<_, String>(0)?)
            .expect("stored IDs are canonical by construction");
        let sequence: i64 = row.get(1)?;
        let digest_str: String = row.get(2)?;
        let digest = aw_contracts::common::Digest::parse(digest_str)
            .expect("stored digests are canonical by construction");

        // Rebuild a minimal admitted record to advance the chain; the
        // body and canonical bytes are not needed for tip tracking.
        let tip_record = AdmittedRecord {
            header: aw_contracts::ledger::LedgerRecordHeader {
                id,
                sequence: sequence as u64,
                timestamp_ms: 0,
                kind: aw_contracts::ledger::LedgerEventKind::EvidenceStored,
                schema: String::new(),
                scope: None,
                parent: None,
                body_digest: aw_contracts::common::Digest::parse(
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                )
                .expect("empty digest parses"),
            },
            body: serde_json::Value::Null,
            body_canonical: Vec::new(),
            body_digest: aw_contracts::common::Digest::parse(
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            )
            .expect("empty digest parses"),
            record_canonical: Vec::new(),
            record_digest: digest,
        };
        chain.extend(&tip_record);
    }
    Ok(chain)
}

fn insert_record(
    tx: &rusqlite::Transaction<'_>,
    record: &AdmittedRecord,
) -> Result<(), StoreError> {
    let header = &record.header;
    tx.execute(
        "INSERT INTO ledger_records
         (id, sequence, timestamp_ms, kind, schema, parent_id, parent_digest,
          body_digest, body_canonical, record_canonical, record_digest)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            header.id.as_str(),
            header.sequence as i64,
            header.timestamp_ms as i64,
            kind_to_str(header.kind),
            header.schema,
            header.parent.as_ref().map(|p| p.id.as_str().to_owned()),
            header.parent.as_ref().map(|p| p.digest.as_str().to_owned()),
            header.body_digest.as_str(),
            record.body_canonical,
            record.record_canonical,
            record.record_digest.as_str(),
        ],
    )?;
    Ok(())
}

pub(crate) fn kind_to_str(kind: aw_contracts::ledger::LedgerEventKind) -> &'static str {
    match kind {
        aw_contracts::ledger::LedgerEventKind::PostToolUsePlan => "post_tool_use_plan",
        aw_contracts::ledger::LedgerEventKind::PreToolUseGate => "pre_tool_use_gate",
        aw_contracts::ledger::LedgerEventKind::ContextAdoption => "context_adoption",
        aw_contracts::ledger::LedgerEventKind::ProviderInvoked => "provider_invoked",
        aw_contracts::ledger::LedgerEventKind::EvidenceStored => "evidence_stored",
        aw_contracts::ledger::LedgerEventKind::ReceiptStored => "receipt_stored",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{admit_and_extend, clean_body};
    use crate::Chain;
    use aw_contracts::ids::{AttemptId, ToolUseId};
    use aw_contracts::ledger::LedgerTraceScope;
    use tempfile::tempdir;

    fn admit_one(chain: &mut Chain, body: serde_json::Value) -> AdmittedRecord {
        admit_and_extend(chain, body)
    }

    #[test]
    fn open_creates_database_with_empty_chain() {
        let dir = tempdir().expect("temp dir");
        let store = LedgerStore::open(dir.path()).expect("store opens");
        let tip = store.tip();
        assert_eq!(tip.sequence, 0);
        assert!(tip.id.is_none());
        assert!(tip.digest.is_none());
    }

    #[test]
    fn append_advances_chain_tip() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        let record = admit_one(&mut chain, clean_body());
        store.append(&record).expect("append succeeds");

        let tip = store.tip();
        assert_eq!(tip.sequence, 0);
        assert_eq!(tip.id, Some(&record.header.id));
        assert_eq!(tip.digest, Some(&record.record_digest));
    }

    #[test]
    fn reopen_recovers_chain_tip() {
        let dir = tempdir().expect("temp dir");
        let record_id;
        let record_digest;
        {
            let mut store = LedgerStore::open(dir.path()).expect("store opens");
            let mut chain = Chain::new();
            let record = admit_one(&mut chain, clean_body());
            store.append(&record).expect("append succeeds");
            record_id = record.header.id.clone();
            record_digest = record.record_digest.clone();
        }

        let store = LedgerStore::open(dir.path()).expect("store reopens");
        let tip = store.tip();
        assert_eq!(tip.sequence, 0);
        assert_eq!(tip.id, Some(&record_id));
        assert_eq!(tip.digest, Some(&record_digest));
    }

    #[test]
    fn two_records_persist_in_order() {
        let dir = tempdir().expect("temp dir");
        let mut chain = Chain::new();
        {
            let mut store = LedgerStore::open(dir.path()).expect("store opens");
            let r0 = admit_one(&mut chain, clean_body());
            store.append(&r0).expect("first append");
            let r1 = admit_one(&mut chain, clean_body());
            store.append(&r1).expect("second append");
        }

        let store = LedgerStore::open(dir.path()).expect("store reopens");
        assert_eq!(store.tip().sequence, 1);
    }

    #[test]
    fn duplicate_sequence_is_rejected() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        let record = admit_one(&mut chain, clean_body());
        store.append(&record).expect("first append");

        // Build another record at the same sequence using a fresh chain
        // that still thinks it is at genesis.
        let mut duplicate_chain = Chain::new();
        let duplicate = admit_one(&mut duplicate_chain, clean_body());
        assert!(store.append(&duplicate).is_err());
    }

    #[test]
    fn scope_row_is_written_and_queryable() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let chain = Chain::new();
        let attempt_id = AttemptId::new();
        let tool_use_id = ToolUseId::new();
        let scope = LedgerTraceScope {
            attempt_id: Some(attempt_id.clone()),
            tool_use_id: Some(tool_use_id.clone()),
            invocation_id: None,
        };
        let tip = chain.tip();
        let mut candidate = crate::tests::candidate(&tip, clean_body());
        candidate.header.scope = Some(scope.clone());
        let record = crate::admit(&tip, candidate).expect("admission succeeds");
        store.append(&record).expect("append with scope");

        let found: String = store
            .conn
            .query_row(
                "SELECT attempt_id FROM ledger_scope WHERE record_id = ?1",
                [record.header.id.as_str()],
                |row| row.get(0),
            )
            .expect("scope row present");
        assert_eq!(found, attempt_id.as_str());
    }

    #[test]
    fn record_canonical_bytes_round_trip() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        let record = admit_one(&mut chain, clean_body());
        let expected_canonical = record.record_canonical.clone();
        let expected_digest = record.record_digest.as_str().to_owned();
        store.append(&record).expect("append succeeds");

        let (stored_canonical, stored_digest): (Vec<u8>, String) = store
            .conn
            .query_row(
                "SELECT record_canonical, record_digest FROM ledger_records WHERE id = ?1",
                [record.header.id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("record present");
        assert_eq!(stored_canonical, expected_canonical);
        assert_eq!(stored_digest, expected_digest);
    }
}
