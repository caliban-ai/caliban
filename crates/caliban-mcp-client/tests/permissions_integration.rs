//! Phase B — per-server permission scoping wired end-to-end through
//! `PermissionsHook`. Ensures `[server.X.permissions] deny = ["delete_*"]`
//! actually blocks `mcp__X__delete_<anything>` at the hook layer.

#![allow(clippy::missing_panics_doc, clippy::pedantic)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use caliban_agent_core::{
    Action, HookDecision, Hooks, NonInteractiveAskHandler, NoopHooks, PermissionsHook, Rule,
    ToolCtx, default_rules,
};
use caliban_mcp_client::{
    ManualOauthConfig, OauthMode, ServerConfig, ServerPermissions, TransportKind, merge_with_global,
};

fn server_with_permissions(perms: ServerPermissions) -> ServerConfig {
    ServerConfig {
        transport: TransportKind::Stdio,
        command: "noop".to_string(),
        args: vec![],
        env: BTreeMap::new(),
        cwd: Option::<PathBuf>::None,
        url: None,
        headers: BTreeMap::new(),
        oauth: OauthMode::Off,
        manual_oauth: ManualOauthConfig::default(),
        disabled: false,
        lazy: None,
        permissions: perms,
    }
}

fn build_hook(rules: Vec<Rule>) -> PermissionsHook {
    PermissionsHook::new(
        rules,
        Arc::new(NonInteractiveAskHandler { auto_allow: false }),
        Arc::new(NoopHooks),
    )
}

/// Server-scoped deny blocks the matching mcp tool name.
#[tokio::test]
async fn server_deny_rule_blocks_matching_tool() {
    let mut servers = BTreeMap::new();
    servers.insert(
        "linear".to_string(),
        server_with_permissions(ServerPermissions {
            allow: vec![],
            deny: vec!["delete_*".to_string()],
            ask: vec![],
        }),
    );

    // Global rules: empty + defaults (where `*` → Ask) at the tail. Without the
    // per-server rule, `mcp__linear__delete_issue` would land at `*` → Ask.
    let rules = merge_with_global(default_rules(), &servers);
    let hook = build_hook(rules);

    let input = serde_json::json!({});
    let ctx = ToolCtx {
        turn_index: 0,
        tool_use_id: "t1",
        tool_name: "mcp__linear__delete_issue",
        input: &input,
    };
    let action = hook.evaluate(&ctx);
    assert_eq!(action, Action::Deny);

    let decision = hook.before_tool(&ctx).await.unwrap();
    assert!(
        matches!(decision, HookDecision::Deny(_)),
        "got: {decision:?}",
    );
}

/// Server-scoped allow lets a tool through that the global default would have
/// asked about.
#[tokio::test]
async fn server_allow_lets_tool_through() {
    let mut servers = BTreeMap::new();
    servers.insert(
        "linear".to_string(),
        server_with_permissions(ServerPermissions {
            allow: vec!["read_*".to_string()],
            deny: vec![],
            ask: vec![],
        }),
    );
    let rules = merge_with_global(default_rules(), &servers);
    let hook = build_hook(rules);

    let input = serde_json::json!({});
    let ctx = ToolCtx {
        turn_index: 0,
        tool_use_id: "t1",
        tool_name: "mcp__linear__read_issues",
        input: &input,
    };
    let action = hook.evaluate(&ctx);
    assert_eq!(action, Action::Allow);
}

/// A global deny still wins over a per-server allow — the spec's order is
/// "global deny → server deny/ask/allow → global ask/allow → default(Ask)".
#[tokio::test]
async fn global_deny_overrides_server_allow() {
    let mut servers = BTreeMap::new();
    servers.insert(
        "linear".to_string(),
        server_with_permissions(ServerPermissions {
            allow: vec!["delete_*".to_string()],
            deny: vec![],
            ask: vec![],
        }),
    );
    // Global deny on `mcp__*` placed *before* the server block — the merge
    // helper threads the server rules in between global denies and global rest.
    let mut global = vec![Rule {
        tool: "mcp__*__delete_*".to_string(),
        action: Action::Deny,
        comment: Some("global deny".to_string()),
        reason: None,
        expires_at: None,
    }];
    global.extend(default_rules());
    let rules = merge_with_global(global, &servers);
    let hook = build_hook(rules);

    let input = serde_json::json!({});
    let ctx = ToolCtx {
        turn_index: 0,
        tool_use_id: "t1",
        tool_name: "mcp__linear__delete_issue",
        input: &input,
    };
    let action = hook.evaluate(&ctx);
    assert_eq!(
        action,
        Action::Deny,
        "global deny must outrank server allow"
    );
}

/// Pattern normalization — server pattern `weird/name.tool` becomes
/// `weird_name_tool` so it matches the registry name our `McpTool` would
/// register.
#[test]
fn pattern_normalization_matches_registered_tool_name() {
    use caliban_mcp_client::compile_server_permission_rules;
    let mut servers = BTreeMap::new();
    servers.insert(
        "fs".to_string(),
        server_with_permissions(ServerPermissions {
            allow: vec!["weird/name.tool".to_string()],
            deny: vec![],
            ask: vec![],
        }),
    );
    let rules = compile_server_permission_rules(&servers);
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].tool, "mcp__fs__weird_name_tool");
}
