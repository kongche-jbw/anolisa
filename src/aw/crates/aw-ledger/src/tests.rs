//! Shared helpers for Ledger tests.
//!
//! `candidate` builds a valid [`CandidateRecord`] whose header digests and
//! parent link match the supplied chain tip, so a test can mutate one
//! field at a time to isolate each invariant.

use aw_contracts::canonical::canonical_json_v1_bytes;
use aw_contracts::common::Digest;
use aw_contracts::ids::LedgerEventId;
use aw_contracts::ledger::{LedgerEventKind, LedgerParent, LedgerRecordHeader};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

use crate::{admit, AdmittedRecord, CandidateRecord, Chain, ChainTip};

/// Schema string used by test candidates. Any non-empty value satisfies
/// admission; the Ledger treats it as opaque.
pub const TEST_SCHEMA: &str = "aw.ledger.test/v1";

/// Builds a candidate record whose sequence, parent link, and body digest
/// are consistent with `tip`. Tests can mutate one field afterwards to
/// isolate each admission invariant.
pub fn candidate(tip: &ChainTip<'_>, body: Value) -> CandidateRecord {
    let body_canonical = canonical_json_v1_bytes(&body).expect("body canonical");
    let body_digest = digest_bytes(&body_canonical);

    let sequence = if tip.id.is_none() {
        0
    } else {
        tip.sequence + 1
    };
    let parent = tip.id.zip(tip.digest).map(|(id, digest)| LedgerParent {
        id: id.clone(),
        digest: digest.clone(),
    });

    CandidateRecord {
        header: LedgerRecordHeader {
            id: LedgerEventId::new(),
            sequence,
            timestamp_ms: 1_725_300_000_000 + sequence * 1_000,
            kind: LedgerEventKind::EvidenceStored,
            schema: TEST_SCHEMA.to_owned(),
            scope: None,
            parent,
            body_digest,
        },
        body,
    }
}

/// Admits a record against `tip` and applies it to `chain`.
/// Panics when admission fails so call sites stay short.
pub fn admit_and_extend(chain: &mut Chain, body: Value) -> AdmittedRecord {
    let tip = chain.tip();
    let candidate = candidate(&tip, body);
    let admitted = admit(&tip, candidate).expect("admission succeeds");
    chain.extend(&admitted);
    admitted
}

/// Builds a minimal valid body — one reference entry carrying a digest.
pub fn clean_body() -> Value {
    json!({
        "evidence": {
            "id": "evd_00000000-0000-0000-0000-000000000000",
            "digest": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        }
    })
}

fn digest_bytes(bytes: &[u8]) -> Digest {
    let hex = format!("{:x}", Sha256::digest(bytes));
    Digest::parse(hex).expect("sha2 output is always a valid lowercase hex digest")
}

mod admission;
mod chain;
mod content_freedom;
