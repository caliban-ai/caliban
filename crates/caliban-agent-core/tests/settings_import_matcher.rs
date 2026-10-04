//! Integration: imported Claude Code permission rules actually match through
//! the real `caliban-agent-core` matcher (end-to-end import -> load -> match).
//!
//! Moved here from `caliban-settings`' unit tests in #711 (ADR 0061):
//! `caliban-settings` no longer depends on `caliban-agent-core`, so the tests
//! that exercise both the importer and the matcher live in the crate that owns
//! the matcher (which depends on settings), not the reverse.

/// A `Bash` tool call carrying `input`, for driving the real matcher.
fn bash_ctx(input: &serde_json::Value) -> caliban_agent_core::ToolCtx<'_> {
    caliban_agent_core::ToolCtx {
        session_id: "test-session",
        turn_index: 0,
        tool_use_id: "t",
        tool_name: "Bash",
        input,
        is_read_only: false,
    }
}

/// Regression (#518): Claude Code's native permission syntax is the
/// **parenthesised** form (`Bash(git *)`, `Edit(src/**)`), and the importer
/// copies patterns through verbatim. Before the matcher accepted that
/// grammar, `caliban perms import` faithfully produced TOML in which every
/// imported rule was inert — a user migrating from Claude Code silently
/// lost their whole permission config without ever seeing a deny message.
///
/// End-to-end: JSON in → TOML out → parsed back into real `Rule`s → run
/// through the real matcher.
#[test]
fn imported_claude_code_paren_rules_actually_match() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("settings.json");
    let dst = dir.path().join("permissions.toml");
    // Verbatim Claude Code syntax — parens, not colons.
    std::fs::write(
        &src,
        r#"{"permissions":{"allow":["Bash(git *)","Edit(src/**)"],"deny":["Bash(rm *)"]}}"#,
    )
    .unwrap();
    assert_eq!(
        caliban_settings::import::import_permissions_to_toml(&src, &dst).unwrap(),
        3
    );

    // Round-trip the emitted TOML back through the real settings type and
    // rule projection, so this covers the whole import → load path.
    let body = std::fs::read_to_string(&dst).unwrap();
    let settings: caliban_settings::Settings = toml::from_str(&body).unwrap();
    let rules = settings.permission_rules();
    assert_eq!(rules.len(), 3, "all three rules should survive the import");

    let ws = std::path::Path::new("/repo");
    let git = serde_json::json!({"command": "git status"});
    let rm = serde_json::json!({"command": "rm -rf /"});
    let allow_pat = &rules
        .iter()
        .find(|r| r.action == caliban_config_types::Action::Allow && r.tool.starts_with("Bash"))
        .expect("the imported allow rule keeps its verbatim pattern")
        .tool;
    assert_eq!(
        allow_pat, "Bash(git *)",
        "importer copies patterns verbatim"
    );
    assert!(
        caliban_agent_core::permissions_matcher::matches_with_workspace(
            allow_pat,
            &bash_ctx(&git),
            ws
        ),
        "an imported Claude Code `Bash(git *)` rule must match `git status`"
    );
    // The imported deny must be live too, so migrating doesn't silently
    // drop a *restriction* either.
    assert!(
        caliban_agent_core::permissions_matcher::matches_with_workspace(
            "Bash(rm *)",
            &bash_ctx(&rm),
            ws
        )
    );
}

/// #618: Claude Code's real Bash rule grammar is the **colon specifier**
/// `Bash(cmd:*)` (a command-prefix match), not the space-glob `Bash(cmd *)`.
/// caliban's matcher reads `cmd:*` literally, so an imported CC config's
/// Bash rules were inert — a migrated `deny` blocked nothing. Import must
/// translate the specifier to caliban's glob form.
#[test]
fn imported_claude_code_colon_specifier_rules_actually_match() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("settings.json");
    let dst = dir.path().join("permissions.toml");
    std::fs::write(
        &src,
        r#"{"permissions":{"allow":["Bash(git diff:*)"],"deny":["Bash(rm:*)"]}}"#,
    )
    .unwrap();
    assert_eq!(
        caliban_settings::import::import_permissions_to_toml(&src, &dst).unwrap(),
        2
    );

    let body = std::fs::read_to_string(&dst).unwrap();
    let settings: caliban_settings::Settings = toml::from_str(&body).unwrap();
    let rules = settings.permission_rules();
    let ws = std::path::Path::new("/repo");
    let m = |pat: &str, cmd: &str| {
        caliban_agent_core::permissions_matcher::matches_with_workspace(
            pat,
            &bash_ctx(&serde_json::json!({ "command": cmd })),
            ws,
        )
    };
    let deny = &rules
        .iter()
        .find(|r| r.action == caliban_config_types::Action::Deny)
        .expect("deny rule survives import")
        .tool;
    let allow = &rules
        .iter()
        .find(|r| r.action == caliban_config_types::Action::Allow)
        .expect("allow rule survives import")
        .tool;

    // `Bash(rm:*)` must deny `rm` and `rm <args>`, but NOT over-match `rmdir`.
    assert!(m(deny, "rm -rf /"), "deny must fire on `rm -rf /`");
    assert!(m(deny, "rm"), "deny must fire on bare `rm`");
    assert!(!m(deny, "rmdir /tmp/x"), "deny must not over-match `rmdir`");
    // `Bash(git diff:*)` must allow `git diff` and `git diff <args>`, not `git difftool`.
    assert!(m(allow, "git diff"), "allow must fire on `git diff`");
    assert!(m(allow, "git diff --stat"));
    assert!(
        !m(allow, "git difftool"),
        "allow must not over-match `git difftool`"
    );
}
