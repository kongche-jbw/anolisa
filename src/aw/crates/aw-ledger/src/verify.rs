//! Hash-chain verification.
//!
//! [`verify_chain`] walks every record in sequence order and recomputes
//! two digests per row: the body digest over the stored canonical body
//! bytes, and the record digest over the full canonical record bytes.
//! It also verifies that each record's parent link matches the previous
//! row's identity and digest. It also compares every denormalized SQL header
//! and scope-index value with the committed envelope. A passing verification
//! proves internal consistency; detecting a maliciously rewritten suffix
//! still requires an externally retained chain digest.

use aw_contracts::canonical::canonical_json_v1_bytes;
use aw_contracts::ledger::{LedgerRecordHeader, LedgerTraceScope};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::store::{LedgerStore, StoreError};

/// Failure returned when the hash chain is broken.
#[derive(Debug, Error)]
pub enum VerifyError {
    /// Two consecutive records are not adjacent in sequence.
    #[error("sequence gap at {at}: expected {expected}, found {found}")]
    SequenceGap {
        /// Position (0-based row index) where the gap was detected.
        at: usize,
        /// Sequence the chain expected.
        expected: u64,
        /// Sequence the row declared.
        found: u64,
    },
    /// A record's parent link does not match the preceding row.
    #[error("parent link broken at sequence {sequence}")]
    ParentLinkBroken {
        /// Sequence of the record whose parent link is wrong.
        sequence: u64,
    },
    /// A record's stored body digest does not match the digest of its
    /// stored canonical body bytes.
    #[error("body digest mismatch at sequence {sequence}")]
    BodyDigestMismatch {
        /// Sequence of the record whose body digest is wrong.
        sequence: u64,
    },
    /// A record's stored record digest does not match the digest
    /// recomputed from its stored canonical record bytes.
    #[error("record digest mismatch at sequence {sequence}")]
    RecordDigestMismatch {
        /// Sequence of the record whose digest is wrong.
        sequence: u64,
    },
    /// The stored canonical record bytes do not decode to a valid
    /// record envelope.
    #[error("canonical record bytes corrupt at sequence {sequence}")]
    IntegrityBroken {
        /// Sequence of the record whose bytes are corrupt.
        sequence: u64,
    },
    /// An indexed SQL header column differs from the committed header.
    #[error(
        "indexed header field {field} differs from the canonical record at sequence {sequence}"
    )]
    HeaderMismatch {
        /// Sequence of the record whose indexed header differs.
        sequence: u64,
        /// Stable header field name that differs.
        field: &'static str,
    },
    /// The scope query index differs from the scope committed in the header.
    #[error("scope index differs from the canonical record at sequence {sequence}")]
    ScopeMismatch {
        /// Sequence of the record whose scope index differs.
        sequence: u64,
    },
    /// The separately stored body bytes differ from the body in the record.
    #[error("canonical body bytes differ from the canonical record at sequence {sequence}")]
    BodyCanonicalMismatch {
        /// Sequence of the record whose body copies differ.
        sequence: u64,
    },
    /// A database error prevented verification.
    #[error("ledger database error: {0}")]
    Database(#[from] crate::StoreError),
}

/// Walks every record in sequence order and verifies the hash chain.
///
/// Returns the number of records verified on success.
///
/// # Errors
///
/// Returns [`VerifyError`] at the first invariant violation. A full
/// verification of *n* records performs *n* digest recomputations and
/// *n* − 1 parent link checks.
pub fn verify_chain(store: &LedgerStore) -> Result<usize, VerifyError> {
    let mut stmt = store
        .conn()
        .prepare(
            "SELECT r.id, r.sequence, r.timestamp_ms, r.kind, r.schema,
                    r.parent_id, r.parent_digest, r.body_digest,
                    r.body_canonical, r.record_canonical, r.record_digest,
                    s.record_id, s.attempt_id, s.tool_use_id, s.invocation_id
             FROM ledger_records r
             LEFT JOIN ledger_scope s ON s.record_id = r.id
             ORDER BY sequence ASC",
        )
        .map_err(StoreError::from)?;
    let rows = stmt
        .query_map([], |row| {
            let scope_record_id: Option<String> = row.get(11)?;
            let scope_attempt_id: Option<String> = row.get(12)?;
            let scope_tool_use_id: Option<String> = row.get(13)?;
            let scope_invocation_id: Option<String> = row.get(14)?;
            Ok(RawRow {
                id: row.get(0)?,
                sequence: row.get::<_, i64>(1)? as u64,
                timestamp_ms: row.get::<_, i64>(2)?,
                kind: row.get(3)?,
                schema: row.get(4)?,
                parent_id: row.get(5)?,
                parent_digest: row.get(6)?,
                body_digest: row.get(7)?,
                body_canonical: row.get(8)?,
                record_canonical: row.get(9)?,
                record_digest: row.get(10)?,
                scope: scope_record_id.map(|record_id| RawScope {
                    record_id,
                    attempt_id: scope_attempt_id,
                    tool_use_id: scope_tool_use_id,
                    invocation_id: scope_invocation_id,
                }),
            })
        })
        .map_err(StoreError::from)?;

    let mut prev_id: Option<String> = None;
    let mut prev_digest: Option<String> = None;
    let mut expected_sequence: u64 = 0;
    let mut count = 0;

    for row in rows {
        let row = row.map_err(StoreError::from)?;

        // 1. Sequence continuity.
        if row.sequence != expected_sequence {
            return Err(VerifyError::SequenceGap {
                at: count,
                expected: expected_sequence,
                found: row.sequence,
            });
        }

        // 2. Parent link (skipped for genesis).
        if expected_sequence > 0 {
            let parent_ok = match (&row.parent_id, &row.parent_digest, &prev_id, &prev_digest) {
                (Some(pid), Some(pd), Some(prev_id), Some(prev_d)) => {
                    pid == prev_id && pd == prev_d
                }
                _ => false,
            };
            if !parent_ok {
                return Err(VerifyError::ParentLinkBroken {
                    sequence: row.sequence,
                });
            }
        } else if row.parent_id.is_some() || row.parent_digest.is_some() {
            // Genesis must not have a parent.
            return Err(VerifyError::ParentLinkBroken { sequence: 0 });
        }

        // 3. Body digest over canonical body bytes.
        let body_digest_computed = digest_hex(&row.body_canonical);
        if body_digest_computed != row.body_digest {
            return Err(VerifyError::BodyDigestMismatch {
                sequence: row.sequence,
            });
        }

        // 4. Record digest over canonical record bytes.
        let record_digest_computed = digest_hex(&row.record_canonical);
        if record_digest_computed != row.record_digest {
            return Err(VerifyError::RecordDigestMismatch {
                sequence: row.sequence,
            });
        }

        // 5. The committed envelope is canonical and agrees with every
        //    duplicated SQL header, body, and scope-index column.
        verify_committed_envelope(&row)?;

        prev_id = Some(row.id);
        prev_digest = Some(row.record_digest);
        expected_sequence = row.sequence + 1;
        count += 1;
    }

    Ok(count)
}

fn digest_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Verifies the committed envelope and every denormalized query column.
fn verify_committed_envelope(row: &RawRow) -> Result<(), VerifyError> {
    let value: serde_json::Value = serde_json::from_slice(&row.record_canonical).map_err(|_| {
        VerifyError::IntegrityBroken {
            sequence: row.sequence,
        }
    })?;
    let canonical = canonical_json_v1_bytes(&value).map_err(|_| VerifyError::IntegrityBroken {
        sequence: row.sequence,
    })?;
    if canonical != row.record_canonical {
        return Err(VerifyError::IntegrityBroken {
            sequence: row.sequence,
        });
    }
    let envelope: Envelope =
        serde_json::from_value(value).map_err(|_| VerifyError::IntegrityBroken {
            sequence: row.sequence,
        })?;

    let body_canonical =
        canonical_json_v1_bytes(&envelope.body).map_err(|_| VerifyError::IntegrityBroken {
            sequence: row.sequence,
        })?;
    if body_canonical != row.body_canonical {
        return Err(VerifyError::BodyCanonicalMismatch {
            sequence: row.sequence,
        });
    }
    let body_digest_computed = digest_hex(&body_canonical);
    if body_digest_computed != row.body_digest {
        return Err(VerifyError::BodyDigestMismatch {
            sequence: row.sequence,
        });
    }

    compare_header(row, &envelope.header)?;
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    header: LedgerRecordHeader,
    body: serde_json::Value,
}

fn compare_header(row: &RawRow, header: &LedgerRecordHeader) -> Result<(), VerifyError> {
    compare_field(row, "id", header.id.as_str() == row.id)?;
    compare_field(row, "sequence", header.sequence == row.sequence)?;
    compare_field(
        row,
        "timestamp_ms",
        u64::try_from(row.timestamp_ms) == Ok(header.timestamp_ms),
    )?;
    compare_field(
        row,
        "kind",
        crate::store::kind_to_str(header.kind) == row.kind,
    )?;
    compare_field(row, "schema", header.schema == row.schema)?;
    compare_field(
        row,
        "parent",
        parent_matches(
            header.parent.as_ref(),
            row.parent_id.as_deref(),
            row.parent_digest.as_deref(),
        ),
    )?;
    compare_field(
        row,
        "body_digest",
        header.body_digest.as_str() == row.body_digest,
    )?;

    if !scope_matches(header.scope.as_ref(), row.scope.as_ref(), &row.id) {
        return Err(VerifyError::ScopeMismatch {
            sequence: row.sequence,
        });
    }
    Ok(())
}

fn parent_matches(
    committed: Option<&aw_contracts::ledger::LedgerParent>,
    indexed_id: Option<&str>,
    indexed_digest: Option<&str>,
) -> bool {
    match (committed, indexed_id, indexed_digest) {
        (None, None, None) => true,
        (Some(committed), Some(indexed_id), Some(indexed_digest)) => {
            committed.id.as_str() == indexed_id && committed.digest.as_str() == indexed_digest
        }
        _ => false,
    }
}

fn compare_field(row: &RawRow, field: &'static str, matches: bool) -> Result<(), VerifyError> {
    if matches {
        Ok(())
    } else {
        Err(VerifyError::HeaderMismatch {
            sequence: row.sequence,
            field,
        })
    }
}

fn scope_matches(
    committed: Option<&LedgerTraceScope>,
    indexed: Option<&RawScope>,
    id: &str,
) -> bool {
    match (committed, indexed) {
        (None, None) => true,
        (Some(committed), Some(indexed)) => {
            indexed.record_id == id
                && committed.attempt_id.as_ref().map(|value| value.as_str())
                    == indexed.attempt_id.as_deref()
                && committed.tool_use_id.as_ref().map(|value| value.as_str())
                    == indexed.tool_use_id.as_deref()
                && committed.invocation_id.as_ref().map(|value| value.as_str())
                    == indexed.invocation_id.as_deref()
        }
        _ => false,
    }
}

struct RawRow {
    id: String,
    sequence: u64,
    timestamp_ms: i64,
    kind: String,
    schema: String,
    parent_id: Option<String>,
    parent_digest: Option<String>,
    body_digest: String,
    body_canonical: Vec<u8>,
    record_canonical: Vec<u8>,
    record_digest: String,
    scope: Option<RawScope>,
}

struct RawScope {
    record_id: String,
    attempt_id: Option<String>,
    tool_use_id: Option<String>,
    invocation_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{admit, Chain, LedgerStore};
    use aw_contracts::ids::{AttemptId, ToolUseId};
    use aw_contracts::ledger::LedgerTraceScope;
    use serde_json::json;
    use tempfile::tempdir;

    fn append_n(store: &mut LedgerStore, chain: &mut Chain, n: usize) {
        for _ in 0..n {
            let body = json!({
                "projection": {
                    "id": "prj_00000000-0000-0000-0000-000000000000",
                    "digest": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                }
            });
            let tip = chain.tip();
            let candidate = crate::tests::candidate(&tip, body);
            let admitted = admit(&tip, candidate).expect("admit");
            store.append(&admitted).expect("append");
            chain.extend(&admitted);
        }
    }

    #[test]
    fn empty_chain_verifies_as_zero_records() {
        let dir = tempdir().expect("temp dir");
        let store = LedgerStore::open(dir.path()).expect("store opens");
        assert_eq!(verify_chain(&store).expect("verify"), 0);
    }

    #[test]
    fn single_record_verifies() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        append_n(&mut store, &mut chain, 1);
        assert_eq!(verify_chain(&store).expect("verify"), 1);
    }

    #[test]
    fn multi_record_chain_verifies() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        append_n(&mut store, &mut chain, 5);
        assert_eq!(verify_chain(&store).expect("verify"), 5);
    }

    #[test]
    fn tampered_body_digest_is_detected() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        append_n(&mut store, &mut chain, 1);

        // Corrupt the body_digest column.
        store
            .conn()
            .execute(
                "UPDATE ledger_records SET body_digest = ?1 WHERE sequence = 0",
                ["0000000000000000000000000000000000000000000000000000000000000000"],
            )
            .expect("corrupt");

        let result = verify_chain(&store);
        assert!(
            matches!(result, Err(VerifyError::BodyDigestMismatch { sequence: 0 })),
            "expected BodyDigestMismatch, got {result:?}"
        );
    }

    #[test]
    fn tampered_record_digest_is_detected() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        append_n(&mut store, &mut chain, 1);

        store
            .conn()
            .execute(
                "UPDATE ledger_records SET record_digest = ?1 WHERE sequence = 0",
                ["0000000000000000000000000000000000000000000000000000000000000000"],
            )
            .expect("corrupt");

        let result = verify_chain(&store);
        assert!(
            matches!(
                result,
                Err(VerifyError::RecordDigestMismatch { sequence: 0 })
            ),
            "expected RecordDigestMismatch, got {result:?}"
        );
    }

    #[test]
    fn body_copy_that_diverges_from_the_envelope_is_detected() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        append_n(&mut store, &mut chain, 1);

        let replacement = br#"{"evidence":{}}"#;
        let replacement_digest = digest_hex(replacement);
        store
            .conn()
            .execute(
                "UPDATE ledger_records
                 SET body_canonical = ?1, body_digest = ?2
                 WHERE sequence = 0",
                rusqlite::params![replacement, replacement_digest],
            )
            .expect("replace denormalized body copy");

        let result = verify_chain(&store);
        assert!(
            matches!(
                result,
                Err(VerifyError::BodyCanonicalMismatch { sequence: 0 })
            ),
            "expected BodyCanonicalMismatch, got {result:?}"
        );
    }

    #[test]
    fn broken_parent_link_is_detected() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let mut chain = Chain::new();
        append_n(&mut store, &mut chain, 2);

        // Corrupt the parent_id of the second record.
        store
            .conn()
            .execute(
                "UPDATE ledger_records SET parent_id = ?1 WHERE sequence = 1",
                ["evt_00000000-0000-0000-0000-000000000000"],
            )
            .expect("corrupt");

        let result = verify_chain(&store);
        assert!(
            matches!(result, Err(VerifyError::ParentLinkBroken { sequence: 1 })),
            "expected ParentLinkBroken, got {result:?}"
        );
    }

    #[test]
    fn tampered_denormalized_header_columns_are_detected() {
        let cases = [
            (
                "UPDATE ledger_records SET id = 'evt_00000000-0000-0000-0000-000000000000' WHERE sequence = 0",
                "id",
            ),
            (
                "UPDATE ledger_records SET timestamp_ms = timestamp_ms + 1 WHERE sequence = 0",
                "timestamp_ms",
            ),
            (
                "UPDATE ledger_records SET kind = 'receipt_stored' WHERE sequence = 0",
                "kind",
            ),
            (
                "UPDATE ledger_records SET schema = 'aw.ledger.changed/v1' WHERE sequence = 0",
                "schema",
            ),
        ];

        for (sql, expected_field) in cases {
            let dir = tempdir().expect("temp dir");
            let mut store = LedgerStore::open(dir.path()).expect("store opens");
            let mut chain = Chain::new();
            append_n(&mut store, &mut chain, 1);
            store.conn().execute(sql, []).expect("tamper index column");

            let result = verify_chain(&store);
            assert!(
                matches!(
                    result,
                    Err(VerifyError::HeaderMismatch {
                        sequence: 0,
                        field,
                    }) if field == expected_field
                ),
                "expected HeaderMismatch for {expected_field}, got {result:?}"
            );
        }
    }

    #[test]
    fn tampered_scope_index_is_detected() {
        let dir = tempdir().expect("temp dir");
        let mut store = LedgerStore::open(dir.path()).expect("store opens");
        let chain = Chain::new();
        let tip = chain.tip();
        let mut candidate = crate::tests::candidate(&tip, json!({"evidence": {}}));
        candidate.header.scope = Some(LedgerTraceScope {
            attempt_id: Some(AttemptId::new()),
            tool_use_id: Some(ToolUseId::new()),
            invocation_id: None,
        });
        let admitted = admit(&tip, candidate).expect("admit");
        store.append(&admitted).expect("append");

        store
            .conn()
            .execute(
                "UPDATE ledger_scope SET attempt_id = ?1 WHERE record_id = ?2",
                rusqlite::params![AttemptId::new().as_str(), admitted.header.id.as_str()],
            )
            .expect("tamper scope index");

        let result = verify_chain(&store);
        assert!(
            matches!(result, Err(VerifyError::ScopeMismatch { sequence: 0 })),
            "expected ScopeMismatch, got {result:?}"
        );
    }
}
