#![cfg(unix)]

// The parent tests module is itself loaded through a path attribute, so
// directory inference does not apply to its children.
#[path = "tests/providers.rs"]
mod providers;

#[path = "tests/ledger.rs"]
mod ledger;

use aw_contracts::common::{BoundedName, BoundedOpaque, TargetRef};
use aw_contracts::context::{ContextArtifactOrigin, ToolResultSubmission};
use aw_contracts::ids::{
    ActorId, AgentSessionId, AgentWorkId, AttemptId, EnvironmentId, ExecutionContextId, ToolUseId,
    TurnId,
};
use aw_contracts::provider::{ProviderDisposition, ProviderMeasurementKind, ProviderMeter};
use aw_contracts::security::{
    GateDegradation, ObservationGapReason, PendingToolCallSubmission, SecurityCodeLanguage,
    SecurityRuleId,
};
use aw_provider_host::{ProviderAdmissionOptions, ProviderCatalog, ProviderManifestSource};
use serde_json::json;

use super::{
    context_artifact_id, context_projection_input, sha256_digest, validate_meter,
    CapabilityPreferences, Core, CoreConfig, CoreError, MediationFailurePolicy, SessionContextSpec,
    ToolCallGate,
};
use crate::execute::capability_idempotency_key;
use crate::plan::PlanBoundary;
use providers::{write_provider, FixtureKind};

/// Core-side invocation ceiling used by every plan test.
///
/// The effective budget is the smaller of this value and the fixture manifest
/// limit, so both have to be loose. A loaded runner needs well over the 2000ms
/// production default just to spawn the fixture, and a deadline there would
/// report `provider_timeout` instead of the routing under test.
const FIXTURE_WALL_TIME_MS: u64 = 30_000;

#[test]
fn security_meters_must_match_the_canonical_fact() {
    let meter = ProviderMeter {
        meter_id: BoundedName::new("security.scanned_bytes").expect("meter id is valid"),
        unit: BoundedName::new("bytes").expect("meter unit is valid"),
        measurement_kind: ProviderMeasurementKind::Observed,
        method: None,
        value: 38,
    };
    assert!(validate_meter(
        std::slice::from_ref(&meter),
        "security.scanned_bytes",
        "bytes",
        ProviderMeasurementKind::Observed,
        38,
    )
    .is_ok());
    assert!(matches!(
        validate_meter(
            std::slice::from_ref(&meter),
            "security.scanned_bytes",
            "bytes",
            ProviderMeasurementKind::Observed,
            37,
        ),
        Err(CoreError::SecurityMeterMismatch {
            meter_id: "security.scanned_bytes"
        })
    ));
    assert!(matches!(
        validate_meter(
            &[ProviderMeter {
                unit: BoundedName::new("tokens").expect("unit is valid"),
                ..meter
            }],
            "security.scanned_bytes",
            "bytes",
            ProviderMeasurementKind::Observed,
            38,
        ),
        Err(CoreError::SecurityMeterMismatch {
            meter_id: "security.scanned_bytes"
        })
    ));
}

#[test]
fn execution_context_allocates_once_or_preserves_a_propagated_identity() {
    let (_packages, core) = core_fixture(&[("projection-a", FixtureKind::Projection)]);
    let propagated = ExecutionContextId::new();
    let resumed = core
        .establish_execution_context(context_spec(Some(propagated.clone())))
        .expect("a valid propagated execution context is admitted");
    let allocated = core
        .establish_execution_context(context_spec(None))
        .expect("Core allocates a missing execution context");

    assert_eq!(resumed.execution_context_id(), &propagated);
    assert_ne!(allocated.execution_context_id(), &propagated);
}

#[test]
fn attempt_scope_requires_work() {
    let (_packages, core) = core_fixture(&[("projection-a", FixtureKind::Projection)]);
    let mut spec = context_spec(None);
    spec.attempt_id = Some(AttemptId::new());

    assert!(matches!(
        core.establish_execution_context(spec),
        Err(CoreError::AttemptWithoutWork)
    ));
}

#[test]
fn default_core_refuses_content_provider_without_enforced_controls() {
    let root = tempfile::tempdir().expect("fixture root is created");
    write_provider(root.path(), "projection-a", FixtureKind::Projection);
    let catalog = ProviderCatalog::discover(
        ProviderManifestSource::Directory(root.path().to_path_buf()),
        &ProviderAdmissionOptions::default(),
    )
    .expect("fixture Provider is admitted");
    let mut core = Core::new(catalog);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let error = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission("source"),
            &CapabilityPreferences::default(),
        )
        .expect_err("unenforced Provider requires explicit trust");

    assert!(matches!(error, CoreError::ProviderControlsNotEnforced));
}

#[test]
fn canonical_tool_input_contains_the_source_artifact_and_post_tool_boundary() {
    let artifact_id = aw_contracts::ids::ArtifactId::new();
    let submission = submission("original command output");
    let source_digest = sha256_digest(submission.content.as_bytes()).expect("SHA-256 is canonical");
    let input = context_projection_input(&artifact_id, &source_digest, &submission)
        .expect("typed context input serializes");

    assert_eq!(input.pointer("/artifact/id"), Some(&json!(artifact_id)));
    assert_eq!(
        input.pointer("/artifact/digest"),
        Some(&json!(source_digest))
    );
    assert_eq!(
        input.pointer("/artifact/content"),
        Some(&json!("original command output"))
    );
    assert_eq!(
        input.pointer("/artifact/origin"),
        Some(&json!("command_output"))
    );
    assert_eq!(input.pointer("/artifact/tool_name"), Some(&json!("shell")));
    assert_eq!(input.pointer("/boundary"), Some(&json!("post_tool")));
    assert_eq!(
        input.pointer("/constraints/allow_text_reencoding"),
        Some(&json!(true))
    );
}

#[test]
fn tool_result_route_populates_exact_scope_and_returns_content_free_receipt() {
    let (_packages, mut core) = core_fixture(&[("projection-a", FixtureKind::Projection)]);
    let work_id = AgentWorkId::new();
    let attempt_id = AttemptId::new();
    let propagated = ExecutionContextId::new();
    let mut spec = context_spec(Some(propagated.clone()));
    spec.work_id = Some(work_id.clone());
    spec.attempt_id = Some(attempt_id.clone());
    let context = core
        .establish_execution_context(spec)
        .expect("managed Work scope is valid");
    let turn_id = TurnId::new();
    let tool_use_id = ToolUseId::new();

    let outcome = core
        .observe_tool_result(
            &context,
            turn_id.clone(),
            tool_use_id.clone(),
            submission("sensitive original output"),
            &CapabilityPreferences::default(),
        )
        .expect("the unique exact Provider is invoked");

    let receipt = &outcome.projection.receipt;
    let candidate = outcome
        .projection
        .candidate
        .as_ref()
        .expect("the fixture reports a produced candidate");
    assert_eq!(candidate.content, "projected output");
    assert_eq!(candidate.source_artifact_id, outcome.source_artifact_id);
    assert_eq!(candidate.source_digest, outcome.source_digest);
    assert_eq!(receipt.provider_id.as_str(), "projection-a");
    assert_eq!(receipt.scope.execution_context_id, propagated);
    assert_eq!(receipt.scope.work_id, Some(work_id));
    assert_eq!(receipt.scope.attempt_id, Some(attempt_id));
    assert_eq!(receipt.scope.turn_id, Some(turn_id));
    assert_eq!(receipt.scope.tool_use_id, Some(tool_use_id));
    assert_eq!(outcome.receipts().len(), 1);

    let encoded = serde_json::to_string(receipt).expect("receipt serializes");
    assert!(!encoded.contains("sensitive original output"));
    assert!(!encoded.contains("projected output"));
}

#[test]
fn projection_cannot_change_media_type_when_reencoding_is_forbidden() {
    let (_packages, mut core) = core_fixture(&[("projection-a", FixtureKind::Projection)]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");
    let mut input = submission(r#"{"status":"ready"}"#);
    input.media_type = BoundedName::new("application/json").expect("fixture type is bounded");
    input.allow_text_reencoding = false;

    let error = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            input,
            &CapabilityPreferences::default(),
        )
        .expect_err("text output cannot override a structured representation constraint");

    assert!(matches!(error, CoreError::TextReencodingForbidden));
}

#[test]
fn ambiguous_routes_require_an_explicit_provider_preference() {
    let (_packages, mut core) = core_fixture(&[
        ("projection-a", FixtureKind::Projection),
        ("projection-b", FixtureKind::Projection),
    ]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let error = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission("source"),
            &CapabilityPreferences::default(),
        )
        .expect_err("Core must not pick arbitrarily between eligible Providers");
    assert!(matches!(
        error,
        CoreError::AmbiguousCapabilityRoute { ref capability, ref provider_ids }
            if capability == "context.projection.prepare/v1"
                && provider_ids == "projection-a, projection-b"
    ));

    let preferences = CapabilityPreferences::for_capability(
        "context.projection.prepare",
        BoundedName::new("projection-b").expect("fixture name is bounded"),
    )
    .expect("fixture preference is bounded");
    let outcome = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission("source"),
            &preferences,
        )
        .expect("an eligible explicit preference resolves the route");
    assert_eq!(
        outcome.projection.receipt.provider_id.as_str(),
        "projection-b"
    );
}

#[test]
fn tool_result_idempotency_is_stable_across_invocation_retries() {
    let tool_use_id = ToolUseId::new();
    let input_digest = sha256_digest(b"same canonical input").expect("SHA-256 is canonical");
    let capability = aw_contracts::context::context_projection_prepare_capability()
        .expect("compiled-in Capability is canonical");

    let first = capability_idempotency_key(
        PlanBoundary::PostToolUse,
        &capability,
        tool_use_id.as_str(),
        &input_digest,
    )
    .expect("derived replay key is bounded");
    let second = capability_idempotency_key(
        PlanBoundary::PostToolUse,
        &capability,
        tool_use_id.as_str(),
        &input_digest,
    )
    .expect("derived replay key is bounded");

    assert_eq!(first, second);
    assert!(first.as_str().starts_with("tool-result:tol_"));
    assert!(first.as_str().ends_with(input_digest.as_str()));
    assert!(
        first.as_str().len() <= aw_contracts::common::MAX_IDEMPOTENCY_KEY_BYTES,
        "replay key must fit the bounded contract: {} bytes",
        first.as_str().len()
    );
}

#[test]
fn capability_keys_do_not_collide_across_capabilities() {
    let tool_use_id = ToolUseId::new();
    let input_digest = sha256_digest(b"one canonical input").expect("SHA-256 is canonical");
    let projection = aw_contracts::context::context_projection_prepare_capability()
        .expect("compiled-in Capability is canonical");
    let inspection = aw_contracts::security::security_content_inspect_capability()
        .expect("compiled-in Capability is canonical");

    let projection_key = capability_idempotency_key(
        PlanBoundary::PostToolUse,
        &projection,
        tool_use_id.as_str(),
        &input_digest,
    )
    .expect("derived replay key is bounded");
    let inspection_key = capability_idempotency_key(
        PlanBoundary::PostToolUse,
        &inspection,
        tool_use_id.as_str(),
        &input_digest,
    )
    .expect("derived replay key is bounded");

    assert_ne!(projection_key, inspection_key);
}

#[test]
fn a_preference_for_an_unplanned_capability_is_rejected() {
    let (_packages, mut core) = core_fixture(&[("projection-a", FixtureKind::Projection)]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");
    let preferences = CapabilityPreferences::for_capability(
        "security.content.inspect",
        BoundedName::new("projection-a").expect("fixture name is bounded"),
    )
    .expect("fixture preference is bounded");

    let error = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission("source"),
            &preferences,
        )
        .expect_err("an inapplicable preference must not be silently ignored");

    assert!(matches!(
        error,
        CoreError::PreferenceNotApplicable { ref capability, .. }
            if capability == "security.content.inspect"
    ));
}

#[test]
fn one_observed_tool_result_has_a_stable_artifact_identity() {
    let context_id = ExecutionContextId::new();
    let turn_id = TurnId::new();
    let tool_use_id = ToolUseId::new();
    let source_digest = sha256_digest(b"same source").expect("SHA-256 is canonical");

    let first = context_artifact_id(&context_id, &turn_id, &tool_use_id, &source_digest)
        .expect("derived artifact ID is canonical");
    let second = context_artifact_id(&context_id, &turn_id, &tool_use_id, &source_digest)
        .expect("derived artifact ID is canonical");
    let other_tool = context_artifact_id(&context_id, &turn_id, &ToolUseId::new(), &source_digest)
        .expect("derived artifact ID is canonical");

    assert_eq!(first, second);
    assert_ne!(first, other_tool);
}

#[test]
fn repeated_preparation_reuses_the_observed_artifact() {
    let (_packages, mut core) = core_fixture(&[("projection-a", FixtureKind::Projection)]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");
    let turn_id = TurnId::new();
    let tool_use_id = ToolUseId::new();

    let first = core
        .observe_tool_result(
            &context,
            turn_id.clone(),
            tool_use_id.clone(),
            submission("same source"),
            &CapabilityPreferences::default(),
        )
        .expect("first preparation succeeds");
    let retried = core
        .observe_tool_result(
            &context,
            turn_id,
            tool_use_id,
            submission("same source"),
            &CapabilityPreferences::default(),
        )
        .expect("retry preparation succeeds");

    assert_eq!(first.source_artifact_id, retried.source_artifact_id);
    assert_eq!(first.source_digest, retried.source_digest);
    assert_ne!(
        first.projection.receipt.invocation_id, retried.projection.receipt.invocation_id,
        "each local attempt keeps its own invocation fact"
    );
}

#[test]
fn observe_steps_reach_every_distinct_provider() {
    let (_packages, mut core) = core_fixture(&[
        ("projection-a", FixtureKind::Projection),
        ("scanner-a", FixtureKind::ContentInspect),
        ("scanner-b", FixtureKind::ContentInspect),
        ("code-a", FixtureKind::CodeInspect),
    ]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let outcome = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission("source"),
            &CapabilityPreferences::default(),
        )
        .expect("a fan-out plan does not require a unique Observe implementation");

    assert_eq!(
        outcome.observations.len(),
        3,
        "both content scanners and the code scanner report facts"
    );
    assert!(outcome.observation_gaps.is_empty());
    assert_eq!(
        outcome.receipts().len(),
        4,
        "three Observe receipts plus one Advise receipt"
    );
    assert!(outcome.projection.candidate.is_some());

    let providers: Vec<_> = outcome
        .observations
        .iter()
        .map(|observation| observation.receipt.provider_id.as_str().to_owned())
        .collect();
    assert!(providers.contains(&"scanner-a".to_owned()));
    assert!(providers.contains(&"scanner-b".to_owned()));
    assert!(providers.contains(&"code-a".to_owned()));
}

#[test]
fn a_partial_risk_fact_keeps_its_verified_coverage() {
    let (_packages, mut core) = core_fixture(&[
        ("projection-a", FixtureKind::Projection),
        ("scanner-a", FixtureKind::ContentInspectPartial),
    ]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let outcome = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission("source"),
            &CapabilityPreferences::default(),
        )
        .expect("a partial fact is still usable when its coverage is explicit");

    assert_eq!(outcome.observations.len(), 1);
    assert_eq!(outcome.observations[0].scanned_bytes, 3);
    assert!(outcome.observations[0].truncated);
    assert!(outcome.observation_gaps.iter().all(|gap| {
        gap.capability.id.as_str() == "security.code.inspect"
            && gap.reason == ObservationGapReason::NoImplementation
    }));
}

#[test]
fn a_complete_observation_without_input_coverage_becomes_a_gap() {
    let (_packages, mut core) = core_fixture(&[
        ("projection-a", FixtureKind::Projection),
        ("scanner-a", FixtureKind::ContentInspectWrongCoverage),
    ]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let outcome = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission("source"),
            &CapabilityPreferences::default(),
        )
        .expect("an invalid Observe result degrades only its own step");

    assert!(outcome.observations.is_empty());
    assert!(outcome.observation_gaps.iter().any(|gap| {
        gap.capability.id.as_str() == "security.content.inspect"
            && gap.reason == ObservationGapReason::InvalidOutput
    }));
}

#[test]
fn an_advise_candidate_survives_a_failed_observation() {
    let (_packages, mut core) = core_fixture(&[
        ("projection-a", FixtureKind::Projection),
        ("scanner-a", FixtureKind::ContentInspectFailing),
    ]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let outcome = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission("source"),
            &CapabilityPreferences::default(),
        )
        .expect("an Observe failure must not fail the plan");

    assert!(
        outcome.projection.candidate.is_some(),
        "an Observe Capability cannot decide whether the Advise result stands"
    );
    let reasons: Vec<_> = outcome
        .observation_gaps
        .iter()
        .map(|gap| gap.reason)
        .collect();
    assert!(reasons.contains(&ObservationGapReason::NotProduced));
    assert!(reasons.contains(&ObservationGapReason::NoImplementation));
}

#[test]
fn an_absent_observe_capability_is_a_gap_not_an_error() {
    let (_packages, mut core) = core_fixture(&[("projection-a", FixtureKind::Projection)]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let outcome = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission("source"),
            &CapabilityPreferences::default(),
        )
        .expect("a missing Observe implementation is a recorded fact, not a failure");

    assert!(outcome.observations.is_empty());
    assert_eq!(outcome.observation_gaps.len(), 2);
    assert!(outcome
        .observation_gaps
        .iter()
        .all(|gap| gap.reason == ObservationGapReason::NoImplementation));
    assert!(outcome
        .observation_gaps
        .iter()
        .all(|gap| gap.receipt.is_none()));
}

#[test]
fn an_observation_carrying_a_matched_value_is_rejected() {
    let (_packages, mut core) = core_fixture(&[
        ("projection-a", FixtureKind::Projection),
        ("scanner-a", FixtureKind::ContentInspectLeaking),
    ]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let outcome = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission("source"),
            &CapabilityPreferences::default(),
        )
        .expect("a rejected observation degrades its own step only");

    assert!(outcome.observations.is_empty());
    assert!(outcome
        .observation_gaps
        .iter()
        .any(|gap| gap.reason == ObservationGapReason::InvalidOutput));
    assert!(outcome.projection.candidate.is_some());

    let encoded = serde_json::to_string(&outcome).expect("outcome serializes");
    assert!(
        !encoded.contains("LTAI5tFixtureLeakedSecret"),
        "a rejected finding must not leak its matched value into the outcome"
    );
}

#[test]
fn advise_routing_fails_before_any_observation_is_collected() {
    let (_packages, mut core) = core_fixture(&[
        ("projection-a", FixtureKind::Projection),
        ("projection-b", FixtureKind::Projection),
        ("scanner-a", FixtureKind::ContentInspect),
    ]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let error = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission("source"),
            &CapabilityPreferences::default(),
        )
        .expect_err("an ambiguous Advise route rejects the whole plan");

    assert!(matches!(error, CoreError::AmbiguousCapabilityRoute { .. }));
}

#[test]
fn observation_facts_never_carry_the_source_content() {
    let (_packages, mut core) = core_fixture(&[
        ("projection-a", FixtureKind::Projection),
        ("scanner-a", FixtureKind::ContentInspect),
        ("code-a", FixtureKind::CodeInspect),
    ]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");
    let marker = "MARKER-e1f2a3b4-source-only";

    let outcome = core
        .observe_tool_result(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            submission(marker),
            &CapabilityPreferences::default(),
        )
        .expect("the plan runs");

    let observations =
        serde_json::to_string(&outcome.observations).expect("observations serialize");
    let gaps = serde_json::to_string(&outcome.observation_gaps).expect("gaps serialize");
    assert!(!observations.contains(marker));
    assert!(!gaps.contains(marker));
    assert!(!serde_json::to_string(&outcome.projection.receipt)
        .expect("receipt serializes")
        .contains(marker));
}

#[test]
fn a_deny_verdict_becomes_a_block_gate() {
    let (_packages, mut core) = core_fixture(&[("gate-a", FixtureKind::CommandInspectDeny)]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let decision = core
        .mediate_tool_call(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            pending_call("rm -rf / --no-preserve-root"),
            &CapabilityPreferences::default(),
        )
        .expect("a mediated route resolves a gate");

    assert_eq!(decision.gate, ToolCallGate::Block, "{decision:#?}");
    assert_eq!(decision.degradation, None);
    assert_eq!(
        decision
            .reasons
            .iter()
            .map(SecurityRuleId::as_str)
            .collect::<Vec<_>>(),
        vec!["fixture.recursive_delete"]
    );
    let receipt = decision.receipt.expect("a mediated gate carries a receipt");
    assert_eq!(receipt.provider_id.as_str(), "gate-a");
    assert_eq!(receipt.disposition, ProviderDisposition::Produced);
}

#[test]
fn an_allow_verdict_carries_no_reason_codes() {
    let (_packages, mut core) = core_fixture(&[("gate-a", FixtureKind::CommandInspectAllow)]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let decision = core
        .mediate_tool_call(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            pending_call("ls -la"),
            &CapabilityPreferences::default(),
        )
        .expect("a mediated route resolves a gate");

    assert_eq!(decision.gate, ToolCallGate::Allow, "{decision:#?}");
    assert!(decision.reasons.is_empty());
    assert_eq!(decision.degradation, None);
}

#[test]
fn an_allow_without_input_coverage_uses_the_failure_gate() {
    let (_packages, mut core) =
        core_fixture(&[("gate-a", FixtureKind::CommandInspectWrongCoverage)]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let decision = core
        .mediate_tool_call(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            pending_call("ls -la"),
            &CapabilityPreferences::default(),
        )
        .expect("a contradictory Provider result settles to policy");

    assert_eq!(decision.gate, ToolCallGate::Ask);
    assert!(decision.reasons.is_empty());
    assert_eq!(decision.degradation, Some(GateDegradation::InvalidOutput));
}

#[test]
fn an_absent_mediate_implementation_is_not_mediated() {
    let (_packages, mut core) = core_fixture(&[("projection-a", FixtureKind::Projection)]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");

    let decision = core
        .mediate_tool_call(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            pending_call("ls -la"),
            &CapabilityPreferences::default(),
        )
        .expect("a missing Mediate implementation is a recorded fact, not a failure");

    assert_eq!(
        decision.gate,
        ToolCallGate::NotMediated,
        "an absent Capability holds no opinion, so Core must not present one"
    );
    assert_eq!(
        decision.degradation,
        Some(GateDegradation::NoImplementation)
    );
    assert!(decision.receipt.is_none());
}

#[test]
fn a_failed_mediation_takes_the_configured_default() {
    for (policy, expected) in [
        (MediationFailurePolicy::Ask, ToolCallGate::Ask),
        (MediationFailurePolicy::Block, ToolCallGate::Block),
    ] {
        let root = tempfile::tempdir().expect("fixture root is created");
        write_provider(root.path(), "gate-a", FixtureKind::CommandInspectFailing);
        let catalog = ProviderCatalog::discover(
            ProviderManifestSource::Directory(root.path().to_path_buf()),
            &ProviderAdmissionOptions::default(),
        )
        .expect("fixture Provider is admitted");
        let mut core = Core::with_config(
            catalog,
            CoreConfig {
                allow_unenforced_providers: true,
                mediation_failure: policy,
                provider_wall_time_ms: FIXTURE_WALL_TIME_MS,
                ..CoreConfig::default()
            },
        )
        .expect("fixture Core configuration is valid");
        let context = core
            .establish_execution_context(context_spec(None))
            .expect("session scope is valid");

        let decision = core
            .mediate_tool_call(
                &context,
                TurnId::new(),
                ToolUseId::new(),
                pending_call("ls -la"),
                &CapabilityPreferences::default(),
            )
            .expect("a Provider failure resolves the gate instead of failing the call");

        assert_eq!(
            decision.gate, expected,
            "a broken scanner must never resolve to an approval"
        );
        assert_eq!(decision.degradation, Some(GateDegradation::NotProduced));
        assert!(decision.receipt.is_some());
    }
}

#[test]
fn a_gate_decision_never_carries_the_command_text() {
    let (_packages, mut core) = core_fixture(&[("gate-a", FixtureKind::CommandInspectDeny)]);
    let context = core
        .establish_execution_context(context_spec(None))
        .expect("session scope is valid");
    let marker = "MARKER-9c8d7e6f-command-only";

    let decision = core
        .mediate_tool_call(
            &context,
            TurnId::new(),
            ToolUseId::new(),
            pending_call(&format!("rm -rf {marker}")),
            &CapabilityPreferences::default(),
        )
        .expect("a mediated route resolves a gate");

    let encoded = serde_json::to_string(&decision).expect("decision serializes");
    assert!(
        !encoded.contains(marker),
        "a gate notice must be renderable to an operator without echoing the command"
    );
}

#[test]
fn the_two_boundaries_do_not_share_a_replay_key() {
    let tool_use_id = ToolUseId::new();
    let input_digest = sha256_digest(b"one canonical input").expect("SHA-256 is canonical");
    let capability = aw_contracts::security::security_command_inspect_capability()
        .expect("compiled-in Capability is canonical");

    let gate_key = capability_idempotency_key(
        PlanBoundary::PreToolUse,
        &capability,
        tool_use_id.as_str(),
        &input_digest,
    )
    .expect("derived replay key is bounded");
    let result_key = capability_idempotency_key(
        PlanBoundary::PostToolUse,
        &capability,
        tool_use_id.as_str(),
        &input_digest,
    )
    .expect("derived replay key is bounded");

    assert_ne!(gate_key, result_key);
    assert!(gate_key.as_str().starts_with("tool-call:tol_"));
    assert!(
        gate_key.as_str().len() <= aw_contracts::common::MAX_IDEMPOTENCY_KEY_BYTES,
        "replay key must fit the bounded contract: {} bytes",
        gate_key.as_str().len()
    );
}

fn core_fixture(packages: &[(&str, FixtureKind)]) -> (tempfile::TempDir, Core) {
    let root = tempfile::tempdir().expect("fixture root is created");
    for (provider_id, kind) in packages {
        write_provider(root.path(), provider_id, *kind);
    }
    let catalog = ProviderCatalog::discover(
        ProviderManifestSource::Directory(root.path().to_path_buf()),
        &ProviderAdmissionOptions::default(),
    )
    .expect("fixture Providers are admitted");
    (
        root,
        Core::with_config(
            catalog,
            CoreConfig {
                allow_unenforced_providers: true,
                provider_wall_time_ms: FIXTURE_WALL_TIME_MS,
                ..CoreConfig::default()
            },
        )
        .expect("fixture Core configuration is valid"),
    )
}

fn context_spec(execution_context_id: Option<ExecutionContextId>) -> SessionContextSpec {
    SessionContextSpec {
        target: TargetRef {
            kind: BoundedName::new("host").expect("fixture target kind is bounded"),
            authority: BoundedName::new("local").expect("fixture target authority is bounded"),
            identifier: BoundedOpaque::new("fixture-host")
                .expect("fixture target identifier is bounded"),
        },
        environment_id: EnvironmentId::new(),
        actor_id: ActorId::new(),
        agent_session_id: Some(AgentSessionId::new()),
        work_id: None,
        attempt_id: None,
        execution_context_id,
    }
}

fn submission(content: &str) -> ToolResultSubmission {
    ToolResultSubmission {
        content: content.to_owned(),
        media_type: BoundedName::new("text/plain").expect("fixture media type is bounded"),
        origin: ContextArtifactOrigin::CommandOutput,
        tool_name: Some(BoundedName::new("shell").expect("fixture tool name is bounded")),
        allow_text_reencoding: true,
    }
}

fn pending_call(command: &str) -> PendingToolCallSubmission {
    PendingToolCallSubmission {
        command: command.to_owned(),
        language: SecurityCodeLanguage::Bash,
        tool_name: Some(BoundedName::new("shell").expect("fixture tool name is bounded")),
    }
}
