//! Natural native launch exercises the installed binary and a persistent PTY.

#[test]
fn aw_natural_qoder_persistent_pty() {
    let output = std::process::Command::new("timeout")
        .args(["--kill-after=5", "60", "python3", "-B"])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/shell_host/aw_fixture.py"
        ))
        .arg(env!("CARGO_BIN_EXE_cosh-shell"))
        .output()
        .expect("start bounded PTY fixture");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
