//! Ledger trace scope indexing.
//!
//! Each canonical record header commits to an optional trace scope. The same
//! values are copied into a side table for bounded queries; verification treats
//! the committed header as authoritative and rejects divergence.

use aw_contracts::ledger::LedgerTraceScope;
use rusqlite::Transaction;

/// Inserts one scope row linked to the record just inserted.
///
/// `scope` may be `None` when the caller has no trace axes to record;
/// in that case no row is written and the partial indexes stay empty
/// for this record.
pub(crate) fn insert(
    tx: &Transaction<'_>,
    record_id: &str,
    scope: Option<&LedgerTraceScope>,
) -> rusqlite::Result<()> {
    let Some(scope) = scope else { return Ok(()) };
    tx.execute(
        "INSERT INTO ledger_scope (record_id, attempt_id, tool_use_id, invocation_id)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            record_id,
            scope.attempt_id.as_ref().map(|id| id.as_str()),
            scope.tool_use_id.as_ref().map(|id| id.as_str()),
            scope.invocation_id.as_ref().map(|id| id.as_str()),
        ],
    )?;
    Ok(())
}
