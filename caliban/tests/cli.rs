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
        stdout.contains("caliban") && stdout.contains(env!("CARGO_PKG_VERSION")),
        "expected --version output to contain 'caliban' and version, got: {stdout:?}",
    );
}

#[test]
fn short_version_flag_prints_version_and_exits_zero() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let output = Command::new(exe)
        .arg("-V")
        .output()
        .expect("failed to invoke caliban binary");

    assert!(output.status.success(), "expected exit 0 for -V");

    let stdout = String::from_utf8(output.stdout).expect("stdout is not UTF-8");
    assert!(
        stdout.contains("caliban") && stdout.contains(env!("CARGO_PKG_VERSION")),
        "expected -V output to contain 'caliban' and version, got: {stdout:?}",
    );
}

#[test]
fn no_args_exits_nonzero() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let output = Command::new(exe)
        .output()
        .expect("failed to invoke caliban binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit for no args, got {:?}",
        output.status,
    );
}

#[test]
fn unknown_arg_exits_two() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let output = Command::new(exe)
        .arg("--foobar-unknown-arg")
        .output()
        .expect("failed to invoke caliban binary");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit code 2 for unknown arg"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr is not UTF-8");
    assert!(
        stderr.contains("--help") || stderr.contains("--foobar-unknown-arg"),
        "expected stderr to mention the unknown argument or --help, got: {stderr:?}",
    );
}
