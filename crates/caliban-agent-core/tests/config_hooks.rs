//! Integration tests for the config-hook execution bridge (#121): config TOML →
//! `build_config_hooks` → `CompositeHooks` → real shell handlers firing.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use caliban_agent_core::{
    CompositeHooks, HookDecision, Hooks, HooksConfig, SessionCtx, ToolCtx, build_config_hooks,
};
use tempfile::TempDir;

fn write_script(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, body).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

/// Build the bridge handlers and wrap them in a `CompositeHooks`, coercing the
/// `Send + Sync` trait objects to the plain `dyn Hooks` the constructor takes.
fn compose(cfg: &HooksConfig) -> CompositeHooks {
    let layers: Vec<std::sync::Arc<dyn Hooks>> = build_config_hooks(cfg, &client())
        .into_iter()
        .map(|h| h as std::sync::Arc<dyn Hooks>)
        .collect();
    CompositeHooks::new(layers)
}

#[tokio::test]
async fn session_start_hook_injects_additional_context() {
    let dir = TempDir::new().unwrap();
    let script = write_script(
        &dir,
        "ctx.sh",
        "#!/bin/sh\necho '{\"additionalContext\": \"INJECTED-FROM-HOOK\"}'\n",
    );
    let toml = format!(
        "[[hooks.SessionStart]]\n[[hooks.SessionStart.handlers]]\ntype = \"command\"\ncommand = \"{}\"\n",
        script.display()
    );
    let cfg = HooksConfig::from_str(&toml, Path::new("test")).unwrap();
    let composite = compose(&cfg);
    let cwd = std::env::current_dir().unwrap();
    let ctx = SessionCtx {
        session_id: "s",
        cwd: &cwd,
        provider: "test",
        model: "m",
    };
    let out = composite.session_start(&ctx).await.unwrap();
    assert_eq!(
        out.additional_context,
        vec!["INJECTED-FROM-HOOK".to_string()]
    );
}

#[tokio::test]
async fn pretooluse_hook_denies() {
    let dir = TempDir::new().unwrap();
    let script = write_script(
        &dir,
        "deny.sh",
        "#!/bin/sh\necho '{\"hookSpecificOutput\": {\"permissionDecision\": \"deny\", \"permissionDecisionReason\": \"nope\"}}'\n",
    );
    let toml = format!(
        "[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[[hooks.PreToolUse.handlers]]\ntype = \"command\"\ncommand = \"{}\"\n",
        script.display()
    );
    let cfg = HooksConfig::from_str(&toml, Path::new("test")).unwrap();
    let composite = compose(&cfg);
    let input = serde_json::json!({});
    let ctx = ToolCtx {
        turn_index: 0,
        tool_use_id: "t1",
        tool_name: "Bash",
        input: &input,
        is_read_only: false,
    };
    let d = composite.before_tool(&ctx).await.unwrap();
    assert!(matches!(d, HookDecision::Deny(_)));
}

#[tokio::test]
async fn disable_all_hooks_fires_nothing() {
    let dir = TempDir::new().unwrap();
    let script = write_script(
        &dir,
        "ctx.sh",
        "#!/bin/sh\necho '{\"additionalContext\": \"X\"}'\n",
    );
    let toml = format!(
        "disable_all_hooks = true\n[[hooks.SessionStart]]\n[[hooks.SessionStart.handlers]]\ntype = \"command\"\ncommand = \"{}\"\n",
        script.display()
    );
    let cfg = HooksConfig::from_str(&toml, Path::new("test")).unwrap();
    let composite = compose(&cfg);
    let cwd = std::env::current_dir().unwrap();
    let ctx = SessionCtx {
        session_id: "s",
        cwd: &cwd,
        provider: "test",
        model: "m",
    };
    let out = composite.session_start(&ctx).await.unwrap();
    assert!(out.additional_context.is_empty());
}
