use super::*;
use serde_json::json;

#[test]
fn profile_format_and_mapping_are_closed() {
    let registry = Registry::new().unwrap();
    let original = load(Host::Qoder, &registry).unwrap();
    for (pointer, replacement) in [
        ("/format", json!(2)),
        ("/host", json!("codex")),
        ("/boundaries/0/native_event", json!("before_tool_call")),
        ("/boundaries/0/descriptor/boundary", json!("post_tool")),
        (
            "/boundaries/0/descriptor/boundary_id",
            json!("qoder.post_tool"),
        ),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            validate_profile(Host::Qoder, &changed, &registry).is_err(),
            "{pointer}"
        );
    }
    let mut changed = original.clone();
    changed["boundaries"][1] = changed["boundaries"][0].clone();
    assert!(validate_profile(Host::Qoder, &changed, &registry).is_err());
    let mut changed = original.clone();
    changed["unexpected"] = json!(true);
    assert!(validate_profile(Host::Qoder, &changed, &registry).is_err());
    let mut changed = original;
    changed["boundaries"][0]["priority"] = json!(0);
    assert!(validate_profile(Host::Qoder, &changed, &registry).is_err());
}

#[test]
fn schema_valid_claims_cannot_elevate_native_authority() {
    let registry = Registry::new().unwrap();
    let original = load(Host::Qoder, &registry).unwrap();
    let mut changed = original.clone();
    let descriptor = &mut changed["boundaries"][0]["descriptor"];
    descriptor["can_deny_dispatch"] = json!(true);
    descriptor["has_final_input_guard"] = json!(true);
    descriptor["composition"]["input_finality"] = json!("immutable_until_dispatch");
    descriptor["composition"]["gate"] = json!("required_final_guard");
    registry.validate_boundary(descriptor).unwrap();
    assert!(validate_profile(Host::Qoder, &changed, &registry).is_err());

    let mut changed = original;
    let descriptor = &mut changed["boundaries"][1]["descriptor"];
    descriptor["proof_boundaries"] = json!(["local_history"]);
    descriptor["ledger_policy"] = json!("required_before_delivery");
    registry.validate_boundary(descriptor).unwrap();
    assert!(validate_profile(Host::Qoder, &changed, &registry).is_err());
}
