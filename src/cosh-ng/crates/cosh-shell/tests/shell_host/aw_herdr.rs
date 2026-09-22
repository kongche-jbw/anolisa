//! Explicit fixed-Herdr acceptance; no real model, network, or downloaded tools.

#[test]
#[ignore = "requires COSH_TEST_HERDR pointing to the pinned Herdr binary"]
fn aw_herdr_on_demand_persistent_shell() {
    run_fixture(None);
}

#[test]
#[ignore = "requires COSH_TEST_HERDR pointing to the pinned Herdr binary"]
fn aw_herdr_login_shell_uses_non_login_pane() {
    run_fixture(Some("login"));
}

#[test]
#[ignore = "requires COSH_TEST_HERDR pointing to the pinned Herdr binary"]
fn aw_herdr_split_and_tab_have_independent_owners() {
    run_fixture(Some("multipane"));
}

fn run_fixture(case: Option<&str>) {
    let herdr = std::env::var_os("COSH_TEST_HERDR")
        .expect("set COSH_TEST_HERDR to the pinned Herdr binary");
    let output = std::process::Command::new("timeout")
        .args(["--kill-after=5", "180", "python3", "-B"])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/shell_host/aw_herdr_fixture.py"
        ))
        .arg(env!("CARGO_BIN_EXE_cosh-shell"))
        .arg(herdr)
        .args(case)
        .output()
        .expect("start bounded fixed-Herdr PTY fixture");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
}
