//! Integration tests for `append_rule_to_file` — exercises the full
//! write path (atomic rename + flock) and verifies that what is written
//! can be round-tripped through `Settings::permission_rules`.

use caliban_settings::{RuleSpec, Settings, append_rule_to_file};

#[test]
fn append_rule_to_empty_file_creates_one_rule_block() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join(".caliban").join("permissions.toml");
    let rule = RuleSpec {
        pattern: "Bash:cargo test --all".into(),
        action: "allow".into(),
        comment: Some("modal Y".into()),
        reason: None,
        expires_at: None,
        tool: None,
    };
    append_rule_to_file(&target, &rule).unwrap();
    let got = std::fs::read_to_string(&target).unwrap();
    assert!(got.contains("[[permissions.rules]]"));
    assert!(got.contains("pattern = \"Bash:cargo test --all\""));
    assert!(got.contains("action  = \"allow\""));
    assert!(got.contains("comment = \"modal Y\""));
}

#[test]
fn append_rule_round_trips_through_settings_load() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join(".caliban").join("permissions.toml");
    append_rule_to_file(
        &target,
        &RuleSpec {
            pattern: "Bash:rm *".into(),
            action: "deny".into(),
            comment: None,
            reason: Some("dangerous".into()),
            expires_at: None,
            tool: None,
        },
    )
    .unwrap();
    let body = std::fs::read_to_string(&target).unwrap();
    let s: Settings = toml::from_str(&body).unwrap();
    let rules = s.permission_rules();
    assert!(
        rules
            .iter()
            .any(|r| r.tool == "Bash:rm *" && r.action == caliban_agent_core::Action::Deny),
        "expected a deny rule for 'Bash:rm *' in: {rules:?}"
    );
}
