//! CLI failures must not emit a success-shaped adoption response.

use std::process::Command;

#[test]
fn adoption_cli_requires_an_explicit_binding_and_event() {
    for args in [vec![], vec!["record"], vec!["verify"]] {
        let result = Command::new(env!("CARGO_BIN_EXE_aw-adoption-cli"))
            .args(args)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8(result.stderr)
            .unwrap()
            .contains("AW adoption unavailable"));
    }
}

#[test]
fn adoption_cli_never_treats_missing_evidence_as_zero_savings_success() {
    let result = Command::new(env!("CARGO_BIN_EXE_aw-adoption-cli"))
        .args([
            "verify",
            "/nonexistent-aw-synthetic-binding.json",
            &"a".repeat(64),
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
}
