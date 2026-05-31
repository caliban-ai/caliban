//! Integration tests for `caliban perms` subcommand (Phase 6, Tasks 6.2–6.5).
//!
//! These tests run the compiled `caliban` binary via `Command::new(env!("CARGO_BIN_EXE_caliban"))`.
//! Each test sets up a temporary directory with whatever settings files are needed, then
//! asserts on the exit code and/or written file contents.

/// Helper: write a minimal settings.toml with the given permissions rules TOML body.
/// Uses `settings.toml` (the v2 native format) so rules are loaded by the primary
/// scope reader rather than the legacy compat shim.
fn write_settings_with_rules(dir: &std::path::Path, rules_toml: &str) {
    std::fs::create_dir_all(dir.join(".caliban")).unwrap();
    std::fs::write(dir.join(".caliban/settings.toml"), rules_toml).unwrap();
}

/// `caliban perms test` returns exit code 0 (allow) when the tool call matches an allow rule.
#[test]
fn perms_test_subcommand_returns_allow_on_match() {
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Bash:git *"
action = "allow"
"#,
    );
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_caliban"))
        .args(["perms", "test", "Bash", r#"{"command":"git push"}"#])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert_eq!(
        status.code(),
        Some(0),
        "expected exit 0 (allow) for matching allow rule"
    );
}

/// `caliban perms test` returns exit code 1 (deny) when the tool call matches a deny rule.
#[test]
fn perms_test_subcommand_returns_deny_on_match() {
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Bash:rm *"
action = "deny"
"#,
    );
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_caliban"))
        .args(["perms", "test", "Bash", r#"{"command":"rm -rf /"}"#])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert_eq!(
        status.code(),
        Some(1),
        "expected exit 1 (deny) for matching deny rule"
    );
}

/// `caliban perms list` exits 0 and prints something.
#[test]
fn perms_list_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Read"
action = "allow"
"#,
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_caliban"))
        .args(["perms", "list"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Read"),
        "expected 'Read' in list output: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Task 6.3: add / remove
// ---------------------------------------------------------------------------

/// `caliban perms add` writes a rule, and `caliban perms remove` removes it.
#[test]
fn perms_add_then_remove_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    // add
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_caliban"))
        .args([
            "perms",
            "add",
            "Bash:foo *",
            "allow",
            "--scope",
            "project",
            "--comment",
            "from CLI",
        ])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0), "perms add should succeed");
    let body = std::fs::read_to_string(dir.path().join(".caliban/permissions.toml")).unwrap();
    assert!(
        body.contains("Bash:foo"),
        "rule should appear in permissions.toml after add"
    );
    assert!(
        body.contains("from CLI"),
        "comment should appear in permissions.toml"
    );
    // remove
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_caliban"))
        .args([
            "perms",
            "remove",
            "--pattern",
            "Bash:foo *",
            "--scope",
            "project",
        ])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0), "perms remove should succeed");
    let body2 = std::fs::read_to_string(dir.path().join(".caliban/permissions.toml")).unwrap();
    assert!(
        !body2.contains("Bash:foo"),
        "rule should be gone after remove"
    );
}

/// `caliban perms list --json` produces valid JSON.
#[test]
fn perms_list_json_mode_produces_valid_json() {
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Bash:git *"
action = "allow"
"#,
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_caliban"))
        .args(["perms", "list", "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("expected valid JSON from perms list --json");
    assert!(parsed.is_array(), "expected a JSON array");
}
