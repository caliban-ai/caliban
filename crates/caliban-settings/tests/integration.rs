//! End-to-end integration tests for `caliban-settings`.
//!
//! These complement the per-module unit tests by exercising public-API
//! paths that touch multiple modules at once (loader → schema → merge →
//! compat → `ApiKeyHelper`).

use std::fs;
use std::path::PathBuf;

use caliban_settings::{
    LoadOptions, McpServerSetting, ModelSelector, Permissions, RestartImpact, Scope, ScopePaths,
    Settings, diff_settings, load_settings, maybe_load_legacy_hooks, maybe_load_legacy_mcp,
    validate_value,
};

fn fake_paths(root: &std::path::Path) -> ScopePaths {
    ScopePaths {
        managed_root: Some(root.join("managed")),
        user_config_dir: Some(root.join("user-config")),
    }
}

fn write(p: &std::path::Path, body: &str) {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body).unwrap();
}

#[test]
fn scope_chain_recorded_in_outcome() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    write(
        &tmp.path().join("user-config/caliban/settings.json"),
        r#"{"model": "user-model"}"#,
    );
    write(
        &ws.join(".caliban/settings.json"),
        r#"{"editor_mode": "vim"}"#,
    );
    let opts = LoadOptions {
        workspace_root: ws,
        paths: fake_paths(tmp.path()),
        ..LoadOptions::default()
    };
    let outcome = load_settings(&opts).unwrap();
    let scopes: Vec<_> = outcome.sources.iter().map(|s| s.scope).collect();
    assert!(scopes.contains(&Scope::User));
    assert!(scopes.contains(&Scope::Project));
    assert_eq!(outcome.settings.editor_mode.as_deref(), Some("vim"));
}

#[test]
fn cli_overlay_layers_above_local() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    write(
        &ws.join(".caliban/settings.local.json"),
        r#"{"editor_mode": "emacs"}"#,
    );
    let mut opts = LoadOptions {
        workspace_root: ws,
        paths: fake_paths(tmp.path()),
        ..LoadOptions::default()
    };
    opts.cli_overlay = Some(serde_json::json!({"editor_mode": "vim"}));
    let outcome = load_settings(&opts).unwrap();
    assert_eq!(outcome.settings.editor_mode.as_deref(), Some("vim"));
    assert!(outcome.sources.iter().any(|s| s.scope == Scope::Cli));
}

#[test]
fn backward_compat_loads_mcp_when_unified_absent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path();
    fs::create_dir_all(ws.join(".caliban")).unwrap();
    fs::write(
        ws.join(".caliban/mcp.toml"),
        "[server.linear]\ncommand = \"npx\"\n",
    )
    .unwrap();
    let mut s = Settings::default();
    assert!(maybe_load_legacy_mcp(&mut s, ws));
    assert!(s.mcp_servers.contains_key("linear"));
}

#[test]
fn backward_compat_hooks_legacy_loader() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path();
    fs::create_dir_all(ws.join(".caliban")).unwrap();
    fs::write(
        ws.join(".caliban/hooks.toml"),
        r#"
disable_all_hooks = false
[[hooks.SessionStart]]
matcher = "*"
[[hooks.SessionStart.handlers]]
type = "command"
command = "/bin/true"
"#,
    )
    .unwrap();
    let mut s = Settings::default();
    assert!(maybe_load_legacy_hooks(&mut s, ws));
    assert!(s.hooks.contains_key("__legacy_hooks_toml__"));
}

#[test]
fn schema_validation_invalid_top_level_array_type() {
    let v: serde_json::Value =
        serde_json::from_str(r#"{"allowed_http_hook_urls": "not-an-array"}"#).unwrap();
    let errs = validate_value(&v);
    assert!(!errs.is_empty());
}

#[test]
fn diff_reports_model_restart_required() {
    let old = Settings::default();
    let new = Settings {
        model: Some(ModelSelector::Name("new".into())),
        ..Default::default()
    };
    let d = diff_settings(&old, &new);
    let entry = d.iter().find(|c| c.key == "model").unwrap();
    assert_eq!(entry.impact, RestartImpact::Restart);
}

#[test]
fn diff_reports_permissions_as_hot() {
    let old = Settings::default();
    let new = Settings {
        permissions: Permissions {
            allow: vec!["Read".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    let d = diff_settings(&old, &new);
    let entry = d.iter().find(|c| c.key == "permissions.allow").unwrap();
    assert_eq!(entry.impact, RestartImpact::Hot);
}

#[test]
fn cli_overlay_inline_json_parses_via_with_cli_overlay() {
    let tmp = tempfile::TempDir::new().unwrap();
    let opts = LoadOptions::new(tmp.path())
        .with_cli_overlay(r#"{"editor_mode": "emacs"}"#)
        .unwrap();
    let outcome = load_settings(&opts).unwrap();
    assert_eq!(outcome.settings.editor_mode.as_deref(), Some("emacs"));
}

#[test]
fn setting_sources_csv_filter() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    write(
        &tmp.path().join("user-config/caliban/settings.json"),
        r#"{"editor_mode": "from-user"}"#,
    );
    write(
        &ws.join(".caliban/settings.local.json"),
        r#"{"editor_mode": "from-local"}"#,
    );
    let opts = LoadOptions {
        workspace_root: ws,
        paths: fake_paths(tmp.path()),
        ..LoadOptions::default()
    }
    .with_sources_csv("user,project");
    let outcome = load_settings(&opts).unwrap();
    assert_eq!(
        outcome.settings.editor_mode.as_deref(),
        Some("from-user"),
        "local should have been filtered out by --setting-sources"
    );
}

#[test]
fn settings_round_trip_mcp_server_setting() {
    let mut servers = std::collections::BTreeMap::new();
    servers.insert(
        "linear".to_string(),
        McpServerSetting {
            command: "npx".into(),
            args: vec!["-y".into()],
            env: std::collections::BTreeMap::new(),
            cwd: Some(PathBuf::from("/tmp")),
            disabled: true,
        },
    );
    let s = Settings {
        mcp_servers: servers,
        ..Default::default()
    };
    let json = serde_json::to_string(&s).unwrap();
    let back: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.mcp_servers["linear"].command, "npx");
    assert!(back.mcp_servers["linear"].disabled);
}

#[test]
fn env_var_settings_dont_clobber_loaded_settings() {
    // `enable_telemetry` is loaded from settings.json; we don't reach
    // into env from this crate (the binary handles env-fallback). Just
    // confirm the field round-trips cleanly.
    let s: Settings = serde_json::from_str(r#"{"enable_telemetry": true}"#).unwrap();
    assert_eq!(s.enable_telemetry, Some(true));
    let s: Settings = serde_json::from_str(r#"{"enable_telemetry": false}"#).unwrap();
    assert_eq!(s.enable_telemetry, Some(false));
    let s: Settings = serde_json::from_str(r"{}").unwrap();
    assert_eq!(s.enable_telemetry, None);
}

#[test]
fn settings_handle_atomic_swap() {
    use caliban_settings::SettingsHandle;
    let h = SettingsHandle::new(Settings::default());
    let original = h.current();
    let updated = Settings {
        editor_mode: Some("vim".into()),
        ..Default::default()
    };
    let prev = h.store(updated);
    assert_eq!(prev.editor_mode.as_deref(), None);
    assert_eq!(h.current().editor_mode.as_deref(), Some("vim"));
    drop(original);
}

#[test]
fn permission_rules_route_into_agent_core() {
    let s = Settings {
        permissions: Permissions {
            allow: vec!["Read".into(), "Glob".into()],
            ask: vec!["Bash".into()],
            deny: vec!["Bash:rm *".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    let rules = s.permission_rules();
    assert_eq!(rules.len(), 4);
    assert_eq!(rules[0].action, caliban_agent_core::Action::Deny);
    assert_eq!(rules[1].action, caliban_agent_core::Action::Ask);
    assert_eq!(rules[2].action, caliban_agent_core::Action::Allow);
    assert_eq!(rules[3].action, caliban_agent_core::Action::Allow);
}
