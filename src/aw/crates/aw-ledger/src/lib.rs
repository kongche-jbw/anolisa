#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Append-only Ledger for the AW boundary events.
//!
//! The Ledger is the durable, tamper-evident record of every AW boundary
//! event worth auditing: plan snapshots, Observe evidence, Mediate
//! credentials, Provider receipts, and final COSH history adoption. Every
//! record commits to its body digest (canonical JSON v1) and to the previous
//! record's digest, producing a hash chain that any reader can recompute from
//! the stored bytes.
//!
//! This crate owns the admission boundary, the hash chain state, the
//! content-freedom invariants, the SQLite store, append orchestration,
//! bounded queries, and chain verification. Boundary adapters in the
//! hook layer consume these primitives to record what they decided.

mod admission;
mod chain;
mod migration;
mod query;
mod scope;
mod sink;
mod store;
mod verify;

pub use admission::{admit, AdmissionError, AdmittedRecord, CandidateRecord};
pub use chain::{Chain, ChainTip};
pub use query::StoredRecord;
pub use sink::{LedgerSink, SinkError};
pub use store::{LedgerStore, StoreError};
pub use verify::{verify_chain, VerifyError};

#[cfg(test)]
mod tests;
