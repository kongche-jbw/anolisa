//! Admission invariant tests.

use aw_contracts::common::Digest;
use aw_contracts::ids::{AttemptId, LedgerEventId};
use aw_contracts::ledger::{LedgerEventKind, LedgerParent, LedgerTraceScope};
use serde_json::json;

use crate::{admit, AdmissionError, Chain};

use super::{admit_and_extend, candidate, clean_body, TEST_SCHEMA};

#[test]
fn genesis_is_accepted_at_sequence_zero() {
    let chain = Chain::new();
    let tip = chain.tip();
    let candidate = candidate(&tip, clean_body());
    let admitted = admit(&tip, candidate).expect("genesis admitted");
    assert_eq!(admitted.header.sequence, 0);
    assert!(admitted.header.parent.is_none());
}

#[test]
fn second_record_links_to_genesis() {
    let mut chain = Chain::new();
    let genesis = admit_and_extend(&mut chain, clean_body());
    let tip = chain.tip();
    let candidate = candidate(&tip, clean_body());
    let admitted = admit(&tip, candidate).expect("second admitted");

    assert_eq!(admitted.header.sequence, 1);
    let parent = admitted.header.parent.as_ref().expect("parent present");
    assert_eq!(parent.id, genesis.header.id);
    assert_eq!(parent.digest, genesis.record_digest);
}

#[test]
fn sequence_mismatch_is_rejected() {
    let chain = Chain::new();
    let tip = chain.tip();
    let mut candidate = candidate(&tip, clean_body());
    candidate.header.sequence = 99;
    let error = admit(&tip, candidate).unwrap_err();
    assert_eq!(
        error,
        AdmissionError::SequenceMismatch {
            expected: 0,
            actual: 99,
        }
    );
}

#[test]
fn sequence_mismatch_after_genesis() {
    let mut chain = Chain::new();
    admit_and_extend(&mut chain, clean_body());
    let tip = chain.tip();
    let mut candidate = candidate(&tip, clean_body());
    candidate.header.sequence = 5;
    let error = admit(&tip, candidate).unwrap_err();
    assert_eq!(
        error,
        AdmissionError::SequenceMismatch {
            expected: 1,
            actual: 5,
        }
    );
}

#[test]
fn parent_mismatch_wrong_id() {
    let mut chain = Chain::new();
    admit_and_extend(&mut chain, clean_body());
    let tip = chain.tip();
    let mut candidate = candidate(&tip, clean_body());
    // Replace the parent ID with a random one, keeping the digest.
    let old_digest = candidate
        .header
        .parent
        .as_ref()
        .expect("parent present")
        .digest
        .clone();
    candidate.header.parent = Some(LedgerParent {
        id: LedgerEventId::new(),
        digest: old_digest,
    });
    assert_eq!(
        admit(&tip, candidate).unwrap_err(),
        AdmissionError::ParentMismatch
    );
}

#[test]
fn parent_mismatch_wrong_digest() {
    let mut chain = Chain::new();
    admit_and_extend(&mut chain, clean_body());
    let tip = chain.tip();
    let mut candidate = candidate(&tip, clean_body());
    // Replace the parent digest with a bogus one, keeping the ID.
    let old_id = candidate
        .header
        .parent
        .as_ref()
        .expect("parent present")
        .id
        .clone();
    candidate.header.parent = Some(LedgerParent {
        id: old_id,
        digest: Digest::parse("0000000000000000000000000000000000000000000000000000000000000000")
            .expect("zero digest parses"),
    });
    assert_eq!(
        admit(&tip, candidate).unwrap_err(),
        AdmissionError::ParentMismatch
    );
}

#[test]
fn parent_required_after_genesis() {
    let mut chain = Chain::new();
    admit_and_extend(&mut chain, clean_body());
    let tip = chain.tip();
    let mut candidate = candidate(&tip, clean_body());
    candidate.header.parent = None;
    assert_eq!(
        admit(&tip, candidate).unwrap_err(),
        AdmissionError::ParentMismatch
    );
}

#[test]
fn body_digest_mismatch_is_rejected() {
    let chain = Chain::new();
    let tip = chain.tip();
    let mut candidate = candidate(&tip, clean_body());
    candidate.header.body_digest =
        Digest::parse("0000000000000000000000000000000000000000000000000000000000000000")
            .expect("zero digest parses");
    assert_eq!(
        admit(&tip, candidate).unwrap_err(),
        AdmissionError::BodyDigestMismatch
    );
}

#[test]
fn empty_schema_is_rejected() {
    let chain = Chain::new();
    let tip = chain.tip();
    let mut candidate = candidate(&tip, clean_body());
    candidate.header.schema = String::new();
    assert_eq!(
        admit(&tip, candidate).unwrap_err(),
        AdmissionError::EmptySchema
    );
}

#[test]
fn admitted_record_carries_canonical_bytes_and_digests() {
    let chain = Chain::new();
    let tip = chain.tip();
    let candidate = candidate(&tip, clean_body());
    let admitted = admit(&tip, candidate).expect("genesis admitted");

    assert!(!admitted.body_canonical.is_empty());
    assert!(!admitted.record_canonical.is_empty());
    // Body digest commits to the body bytes.
    assert_eq!(admitted.body_digest, admitted.header.body_digest);
    // Record digest commits to the record bytes (header + body).
    assert_ne!(admitted.body_digest, admitted.record_digest);
}

#[test]
fn trace_scope_changes_the_canonical_record_commitment() {
    let chain = Chain::new();
    let tip = chain.tip();
    let unscoped = candidate(&tip, clean_body());
    let mut scoped = unscoped.clone();
    scoped.header.scope = Some(LedgerTraceScope {
        attempt_id: Some(AttemptId::new()),
        tool_use_id: None,
        invocation_id: None,
    });

    let unscoped = admit(&tip, unscoped).expect("unscoped record admitted");
    let scoped = admit(&tip, scoped).expect("scoped record admitted");
    assert_ne!(unscoped.record_digest, scoped.record_digest);
    assert!(String::from_utf8(scoped.record_canonical)
        .expect("canonical JSON is UTF-8")
        .contains("\"scope\""));
}

#[test]
fn event_kind_is_preserved_through_admission() {
    let chain = Chain::new();
    let tip = chain.tip();
    let mut candidate = candidate(&tip, clean_body());
    candidate.header.kind = LedgerEventKind::PreToolUseGate;
    candidate.header.schema = "aw.ledger.pre_tool_use_gate/v1".to_owned();
    let admitted = admit(&tip, candidate).expect("admitted");
    assert_eq!(admitted.header.kind, LedgerEventKind::PreToolUseGate);
}

#[test]
fn body_modified_after_candidate_is_rejected() {
    // If the caller swaps the body after computing the digest, admission
    // must catch the mismatch.
    let chain = Chain::new();
    let tip = chain.tip();
    let mut candidate = candidate(&tip, clean_body());
    candidate.body = json!({"different": "body"});
    assert_eq!(
        admit(&tip, candidate).unwrap_err(),
        AdmissionError::BodyDigestMismatch
    );
}

#[test]
fn schema_string_is_used_verbatim() {
    let chain = Chain::new();
    let tip = chain.tip();
    let mut candidate = candidate(&tip, clean_body());
    candidate.header.schema = "aw.ledger.custom/v42".to_owned();
    let admitted = admit(&tip, candidate).expect("admitted");
    assert_eq!(admitted.header.schema, "aw.ledger.custom/v42");
    assert_ne!(admitted.header.schema, TEST_SCHEMA);
}
