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

/// Shared helper: run `caliban perms <args>` in `dir` and capture output.
fn perms(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    let mut full = vec!["perms"];
    full.extend_from_slice(args);
    std::process::Command::new(env!("CARGO_BIN_EXE_caliban"))
        .args(&full)
        .current_dir(dir)
        // Keep the user scope inside the sandbox for verbs that merge it.
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("cfg"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .output()
        .unwrap()
}

// ---------------------------------------------------------------------------
// test / explain
// ---------------------------------------------------------------------------

#[test]
fn perms_test_ask_rule_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Bash:curl *"
action = "ask"
"#,
    );
    let out = perms(
        dir.path(),
        &["test", "Bash", r#"{"command":"curl example.com"}"#],
    );
    assert_eq!(out.status.code(), Some(2), "ask rule should exit 2");
    assert!(String::from_utf8_lossy(&out.stdout).contains("MATCH"));
}

#[test]
fn perms_test_no_match_exits_zero_with_fallthrough() {
    // With an explicit v2 ruleset, `permission_rules()` returns the
    // ordered rules *without* appending the built-in `("*", ask)`
    // catch-all. Testing a tool no rule matches exercises the no-match
    // fall-through branch (exit 0).
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Read"
action = "allow"
"#,
    );
    let out = perms(dir.path(), &["test", "Bash", r#"{"command":"ls"}"#]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("fall through"));
}

#[test]
fn perms_explain_lists_rules_with_match_markers() {
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Bash:git *"
action = "allow"

[[permissions.rules]]
pattern = "Read"
action = "allow"
"#,
    );
    let out = perms(
        dir.path(),
        &["explain", "Bash", r#"{"command":"git status"}"#],
    );
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("first match wins"));
    assert!(
        stdout.contains("MATCH"),
        "the git rule should be marked MATCH: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

#[test]
fn perms_export_toml_default_format() {
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Bash:git *"
action = "allow"
"#,
    );
    let out = perms(dir.path(), &["export"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[[permissions.rules]]"), "{stdout}");
    assert!(stdout.contains("Bash:git"), "{stdout}");
}

#[test]
fn perms_export_json_format_is_grouped_by_action() {
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Bash:rm *"
action = "deny"
"#,
    );
    let out = perms(dir.path(), &["export", "--format", "json"]);
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(v["permissions"]["deny"].is_array());
}

#[test]
fn perms_export_unknown_format_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = perms(dir.path(), &["export", "--format", "yaml"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown format"));
}

// ---------------------------------------------------------------------------
// lint
// ---------------------------------------------------------------------------

#[test]
fn perms_lint_clean_ruleset_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Read"
action = "allow"
"#,
    );
    let out = perms(dir.path(), &["lint"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("no duplicate patterns"));
}

#[test]
fn perms_lint_detects_duplicate_patterns_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Read"
action = "allow"

[[permissions.rules]]
pattern = "Read"
action = "allow"
"#,
    );
    let out = perms(dir.path(), &["lint"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "duplicate patterns should exit 1"
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("duplicate"));
}

// ---------------------------------------------------------------------------
// import / audit / scope + arg errors
// ---------------------------------------------------------------------------

#[test]
fn perms_import_dry_run_reports_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("foreign.json");
    std::fs::write(&src, r#"{"permissions":{"allow":["Read"]}}"#).unwrap();
    let out = perms(
        dir.path(),
        &[
            "import",
            "--from",
            src.to_str().unwrap(),
            "--scope",
            "project",
            "--dry-run",
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("would import"));
}

#[test]
fn perms_audit_with_no_log_prints_empty() {
    let dir = tempfile::tempdir().unwrap();
    let out = perms(dir.path(), &["audit"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("empty"));
}

#[test]
fn perms_list_unknown_scope_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    let out = perms(dir.path(), &["list", "--scope", "bogus"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown scope"));
}

#[test]
fn perms_remove_without_pattern_or_index_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = perms(dir.path(), &["remove", "--scope", "project"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("must specify --pattern or --index"));
}

#[test]
fn perms_remove_index_is_unsupported_in_v2() {
    let dir = tempfile::tempdir().unwrap();
    let out = perms(
        dir.path(),
        &["remove", "--index", "1", "--scope", "project"],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--index removal not supported"));
}

#[test]
fn perms_remove_nonexistent_pattern_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    write_settings_with_rules(
        dir.path(),
        r#"
[[permissions.rules]]
pattern = "Read"
action = "allow"
"#,
    );
    // permissions.toml is the write target; seed it so the file exists.
    std::fs::write(
        dir.path().join(".caliban/permissions.toml"),
        "[[permissions.rules]]\npattern = \"Read\"\naction = \"allow\"\n",
    )
    .unwrap();
    let out = perms(
        dir.path(),
        &["remove", "--pattern", "Nope:never", "--scope", "project"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no matching rule"));
}
