//! Integration tests for `caliban config <verb>` (`subcommands::run_config`).
//!
//! Hermetic: `current_dir` and `HOME`/XDG point at a fresh tempdir.

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
    c.arg("config");
    c.args(args);
    c.output().expect("failed to invoke caliban config")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("terminated by signal")
}

#[test]
fn config_print_emits_settings_and_sources_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["print"]);
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("config print should emit valid JSON");
    assert!(v.get("settings").is_some(), "envelope must have `settings`");
    assert!(v.get("_sources").is_some(), "envelope must have `_sources`");
}

#[test]
fn config_migrate_dry_run_with_no_legacy_reports_nothing_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["migrate", "--dry-run"]);
    assert_eq!(code(&out), 0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no legacy TOMLs to migrate"),
        "expected the no-op migrate message, got: {stderr}"
    );
    // Nothing should be written.
    assert!(!dir.path().join(".caliban/settings.json").exists());
}
