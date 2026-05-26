//! Integration tests for `hooks.toml` loading from project + user scope.
//!
//! These tests exercise the legacy `HooksConfig::load_one` / `HooksConfig::load`
//! entry points, which are `#[deprecated]` in favor of `caliban-settings`.
//! The legacy loaders remain functional for one release cycle, so we
//! suppress the deprecation lint here.

#![allow(deprecated)]

use std::path::PathBuf;

use caliban_agent_core::{HooksConfig, HooksConfigError};
use tempfile::TempDir;

fn write(p: &PathBuf, body: &str) {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

#[test]
fn project_scope_only() {
    let dir = TempDir::new().unwrap();
    write(
        &dir.path().join(".caliban/hooks.toml"),
        r#"
[[hooks.SessionStart]]
matcher = "*"
[[hooks.SessionStart.handlers]]
type = "command"
command = "/bin/true"
"#,
    );
    let cfg = HooksConfig::load_one(&dir.path().join(".caliban/hooks.toml")).unwrap();
    assert_eq!(cfg.handler_count("SessionStart"), 1);
    assert!(!cfg.disable_all_hooks);
}

#[test]
fn missing_file_is_default() {
    let cfg = HooksConfig::load_one(&PathBuf::from("/definitely/not/here/hooks.toml")).unwrap();
    assert_eq!(cfg.total_handler_count(), 0);
}

#[test]
fn disable_all_hooks_top_level() {
    let body = "disable_all_hooks = true\n";
    let cfg = HooksConfig::from_str(body, &PathBuf::from("h.toml")).unwrap();
    assert!(cfg.disable_all_hooks);
}

#[test]
fn allow_managed_hooks_only_top_level() {
    let body = "allow_managed_hooks_only = true\n";
    let cfg = HooksConfig::from_str(body, &PathBuf::from("h.toml")).unwrap();
    assert!(cfg.allow_managed_hooks_only);
}

#[test]
fn parse_error_surfaces() {
    let body = "[[hooks.SessionStart.handlers]]\ntype = ";
    let err = HooksConfig::from_str(body, &PathBuf::from("h.toml")).unwrap_err();
    assert!(matches!(err, HooksConfigError::Parse { .. }));
}

#[test]
fn handler_invalid_when_missing_required_fields() {
    let body = r#"
[[hooks.PreToolUse]]
matcher = "Bash"
[[hooks.PreToolUse.handlers]]
type = "http"
"#;
    let err = HooksConfig::from_str(body, &PathBuf::from("h.toml")).unwrap_err();
    assert!(matches!(err, HooksConfigError::Invalid { .. }));
}

#[test]
fn agent_handler_must_be_async() {
    let body = r#"
[[hooks.FileChanged]]
matcher = "*.rs"
[[hooks.FileChanged.handlers]]
type = "agent"
agent = "code-review"
async = false
"#;
    let err = HooksConfig::from_str(body, &PathBuf::from("h.toml")).unwrap_err();
    match err {
        HooksConfigError::Invalid { message, .. } => {
            assert!(message.contains("async"), "msg = {message}");
        }
        _ => panic!(),
    }
}

#[test]
fn all_handler_types_round_trip() {
    let body = r#"
allowed_http_hook_urls = ["https://hooks.example.com/*"]
http_hook_allowed_env_vars = ["TOKEN"]

[[hooks.SessionStart]]
matcher = "*"
[[hooks.SessionStart.handlers]]
type    = "command"
command = "/bin/true"

[[hooks.PreToolUse]]
matcher = "WebFetch"
[[hooks.PreToolUse.handlers]]
type    = "http"
url     = "https://hooks.example.com/preflight"

[[hooks.PostToolUse]]
matcher = "*"
[[hooks.PostToolUse.handlers]]
type  = "mcp"
mcp   = "audit"
tool  = "log"
async = true

[[hooks.UserPromptSubmit]]
matcher = "*"
[[hooks.UserPromptSubmit.handlers]]
type   = "prompt"
prompt = "Classify"

[[hooks.FileChanged]]
matcher = "*.rs"
[[hooks.FileChanged.handlers]]
type  = "agent"
agent = "code-review"
async = true
"#;
    let cfg = HooksConfig::from_str(body, &PathBuf::from("h.toml")).unwrap();
    assert_eq!(cfg.handler_count("SessionStart"), 1);
    assert_eq!(cfg.handler_count("PreToolUse"), 1);
    assert_eq!(cfg.handler_count("PostToolUse"), 1);
    assert_eq!(cfg.handler_count("UserPromptSubmit"), 1);
    assert_eq!(cfg.handler_count("FileChanged"), 1);
    assert_eq!(cfg.allowed_http_hook_urls.len(), 1);
    assert_eq!(cfg.http_hook_allowed_env_vars, vec!["TOKEN"]);
}
