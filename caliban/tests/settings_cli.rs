//! Integration tests for `caliban settings <verb>` (`settings_cli.rs`).
//!
//! Hermetic: `current_dir` and `HOME`/XDG point at a fresh tempdir so the
//! layered settings loader and scope writers stay inside the sandbox.

use std::path::Path;
use std::process::{Command, Output};

fn caliban(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_caliban"));
    c.current_dir(dir)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("cfg"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CACHE_HOME", dir.join("cache"));
    c
}

fn run(dir: &Path, args: &[&str]) -> Output {
    let mut c = caliban(dir);
    c.arg("settings");
    c.args(args);
    c.output().expect("failed to invoke caliban settings")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("terminated by signal")
}

#[test]
fn print_default_scope_exits_zero_with_json() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["print"]);
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str::<serde_json::Value>(stdout.trim())
        .expect("settings print should emit valid JSON");
}

#[test]
fn print_each_known_scope_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    for scope in ["managed", "user", "project", "local"] {
        let out = run(dir.path(), &["print", "--scope", scope]);
        assert_eq!(code(&out), 0, "scope {scope} should print");
    }
}

#[test]
fn print_unknown_scope_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["print", "--scope", "bogus"]);
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown scope"));
}

#[test]
fn import_dry_run_reports_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("incoming.json");
    std::fs::write(&src, r#"{"model":"claude-sonnet-4-6"}"#).unwrap();
    let out = run(
        dir.path(),
        &["import", "--from", src.to_str().unwrap(), "--dry-run"],
    );
    assert_eq!(code(&out), 0);
    assert!(String::from_utf8_lossy(&out.stdout).contains("would import"));
    // Dry-run must not create the destination settings file.
    assert!(!dir.path().join(".caliban/settings.toml").exists());
}

#[test]
fn import_unknown_scope_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("incoming.json");
    std::fs::write(&src, "{}").unwrap();
    let out = run(
        dir.path(),
        &["import", "--from", src.to_str().unwrap(), "--scope", "nope"],
    );
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown scope"));
}

#[test]
fn import_real_settings_writes_destination() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("incoming.json");
    std::fs::write(&src, r#"{"model":"claude-sonnet-4-6"}"#).unwrap();
    let out = run(
        dir.path(),
        &[
            "import",
            "--from",
            src.to_str().unwrap(),
            "--scope",
            "project",
        ],
    );
    assert_eq!(
        code(&out),
        0,
        "import should succeed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("imported settings to"));
}

#[test]
fn import_missing_source_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(
        dir.path(),
        &[
            "import",
            "--from",
            dir.path().join("nope.json").to_str().unwrap(),
            "--scope",
            "project",
        ],
    );
    assert_eq!(code(&out), 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("import failed"));
}
