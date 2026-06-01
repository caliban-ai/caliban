//! Integration tests for the `Settings.model` + `api_key_helper` wiring.
//!
//! These exercise the seam between `caliban-settings` (TOML parsing +
//! `ApiKeyHelperPool::has_spec_for` / `key_for`) and the binary's
//! resolution path that translates a `.caliban/settings.toml` into the
//! effective provider/model and an API key.
//!
//! The integration test for the actual provider HTTP path (real
//! requests against a mock server) is heavier and lives outside this
//! file — these tests cover the wiring contract.
#![allow(clippy::missing_docs_in_private_items)]

use std::path::PathBuf;

use caliban_settings::{ApiKeyHelperPool, LoadOptions, ModelSelector, ScopePaths, load_settings};

fn workspace_with_settings(toml: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = dir.path();
    let settings_dir = workspace.join(".caliban");
    std::fs::create_dir_all(&settings_dir).expect("mkdir .caliban");
    std::fs::write(settings_dir.join("settings.toml"), toml).expect("write settings.toml");
    dir
}

fn load(workspace: &std::path::Path) -> caliban_settings::Settings {
    // Restrict to project + local scopes so the test doesn't pick up
    // ambient developer settings.
    let opts = LoadOptions {
        workspace_root: workspace.to_path_buf(),
        paths: ScopePaths {
            managed_root: None,
            user_config_dir: Some(PathBuf::from("/nonexistent")),
        },
        scope_filter: Some(vec![
            caliban_settings::Scope::Project,
            caliban_settings::Scope::Local,
        ]),
        cli_overlay: None,
        bare: false,
        schema_validate: false,
    };
    load_settings(&opts).expect("load_settings").settings
}

#[test]
fn settings_toml_with_qualified_model_picks_openai() {
    let dir = workspace_with_settings(
        r#"
[model]
provider = "openai"
name = "gpt-4o"
"#,
    );
    let settings = load(dir.path());

    match settings.model {
        Some(ModelSelector::Qualified { provider, name }) => {
            assert_eq!(provider, "openai");
            assert_eq!(name, "gpt-4o");
        }
        other => panic!("expected Qualified model selector, got {other:?}"),
    }
}

#[test]
fn settings_toml_with_helper_makes_pool_resolve_via_script() {
    let dir = workspace_with_settings(
        r#"
[model]
provider = "openai"
name = "gpt-4o"

[[api_key_helper]]
provider = "openai"
command  = "/bin/sh"
args     = ["-c", "printf sk-from-helper"]
"#,
    );
    let settings = load(dir.path());

    let pool = ApiKeyHelperPool::from_raw(settings.api_key_helper.as_ref());
    assert!(
        pool.has_spec_for("openai"),
        "pool must have an openai spec after parsing api_key_helper"
    );
    // Anthropic gets no spec — the helper is provider-scoped.
    assert!(
        !pool.has_spec_for("anthropic"),
        "anthropic must not match an openai-scoped helper",
    );

    let outcome = pool.key_for("openai").expect("helper invocation");
    assert_eq!(outcome.key, "sk-from-helper");
}

#[test]
fn wildcard_helper_resolves_for_any_provider() {
    let dir = workspace_with_settings(
        r#"
[[api_key_helper]]
provider = "*"
command  = "/bin/sh"
args     = ["-c", "printf sk-wildcard"]
"#,
    );
    let settings = load(dir.path());

    let pool = ApiKeyHelperPool::from_raw(settings.api_key_helper.as_ref());
    for p in ["anthropic", "openai", "google", "ollama"] {
        assert!(pool.has_spec_for(p), "wildcard must match {p}");
    }
    assert_eq!(pool.key_for("openai").expect("ok").key, "sk-wildcard");
}

#[test]
fn empty_settings_produces_empty_pool() {
    let dir = workspace_with_settings("");
    let settings = load(dir.path());

    let pool = ApiKeyHelperPool::from_raw(settings.api_key_helper.as_ref());
    assert!(!pool.has_spec_for("openai"));
    assert!(!pool.has_spec_for("anthropic"));
}
