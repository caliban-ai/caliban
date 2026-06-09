//! Integration tests for the offline diagnostic subcommands:
//! `caliban doctor` (diagnostics.rs) and `caliban router debug` (router.rs).
//!
//! Both run fully offline — `doctor` is invoked without `--deep` (no provider
//! auth pings), and `router debug` resolves a synthetic request with no
//! `caliban.toml` present. Hermetic via `current_dir` + `HOME`/XDG tempdir.

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
    c.args(args);
    c.output().expect("failed to invoke caliban")
}

#[test]
fn doctor_runs_offline_and_prints_checks() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["doctor"]);
    // exit_code() is 0 (no Fail rows) or 1 (some check Failed in the
    // sandbox env) — either is a clean run, not a crash/signal.
    let code = out.status.code().expect("terminated by signal");
    assert!(
        code == 0 || code == 1,
        "doctor exited with unexpected code {code}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("caliban doctor"),
        "expected the doctor header in output, got: {stdout}"
    );
    assert!(
        stdout.contains("check(s):"),
        "expected the check summary line"
    );
}

#[test]
fn router_debug_with_no_config_renders_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["router", "debug"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "router debug should exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.stdout.is_empty(),
        "router debug should print a diagnostic to stdout"
    );
}

#[test]
fn router_debug_honors_purpose_and_request_shape_flags() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(
        dir.path(),
        &[
            "router",
            "debug",
            "--purpose",
            "summarization",
            "--has-vision",
            "--has-tools",
            "--has-thinking",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "router debug with flags should exit 0"
    );
    assert!(!out.stdout.is_empty());
}
