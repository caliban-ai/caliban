//! Integration tests for the `caliban` binary.

use std::process::Command;

#[test]
fn version_flag_prints_version_and_exits_zero() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let output = Command::new(exe)
        .arg("--version")
        .output()
        .expect("failed to invoke caliban binary");

    assert!(
        output.status.success(),
        "expected exit 0, got {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is not UTF-8");
    assert!(
        stdout.starts_with("caliban "),
        "expected stdout to start with 'caliban ', got: {stdout:?}",
    );

    let expected_version = env!("CARGO_PKG_VERSION");
    assert!(
        stdout.contains(expected_version),
        "expected stdout to contain version {expected_version:?}, got: {stdout:?}",
    );
}
