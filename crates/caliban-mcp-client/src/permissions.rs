//! Per-server permission scoping — compiles `[server.X.permissions]` blocks
//! into the global rule grammar.
//!
//! The composition order, per `docs/superpowers/specs/2026-05-24-mcp-v2-design.md`:
//!
//! ```text
//! global deny → server deny → server ask → server allow
//!             → global ask → global allow → default(Ask)
//! ```
//!
//! Callers split their global rule list into `(global_deny, global_rest)` and
//! sandwich the per-server rules from [`compile_server_permission_rules`]
//! in between. The bin crate's `caliban/src/main.rs` does this stitching.

use std::collections::BTreeMap;

use caliban_agent_core::{Action, Rule};

use crate::config::ServerConfig;
use crate::tool::normalize_tool_name;

/// Compile per-server `[server.X.permissions]` blocks into globally-scoped
/// `Rule`s ordered as `deny → ask → allow` (within each server).
///
/// Each pattern is normalized — bare tool names ("delete_*") become full
/// `mcp__<server>__delete_*` patterns so they slot into the global engine
/// without per-tool tracking.
#[must_use]
pub fn compile_server_permission_rules(servers: &BTreeMap<String, ServerConfig>) -> Vec<Rule> {
    let mut deny: Vec<Rule> = Vec::new();
    let mut ask: Vec<Rule> = Vec::new();
    let mut allow: Vec<Rule> = Vec::new();
    for (name, cfg) in servers {
        for pat in &cfg.permissions.deny {
            deny.push(scoped_rule(name, pat, Action::Deny));
        }
        for pat in &cfg.permissions.ask {
            ask.push(scoped_rule(name, pat, Action::Ask));
        }
        for pat in &cfg.permissions.allow {
            allow.push(scoped_rule(name, pat, Action::Allow));
        }
    }
    let mut out = deny;
    out.extend(ask);
    out.extend(allow);
    out
}

/// Build one server-scoped rule. The pattern is normalized (`/` → `_`, etc.)
/// to match the same transformation applied to advertised tool names. Glob
/// metacharacters (`*`, `?`) are preserved.
fn scoped_rule(server: &str, raw_pattern: &str, action: Action) -> Rule {
    // Normalize the pattern just like a tool name — but glob metacharacters
    // must survive. So normalize while pinning `*` and `?` through verbatim.
    let normalized_pat: String = raw_pattern
        .chars()
        .map(|c| {
            if matches!(c, '*' | '?') {
                c.to_string()
            } else {
                // Single-char normalization via the existing helper would
                // collapse multi-byte chars; do it locally so it matches.
                normalize_tool_name(&c.to_string())
            }
        })
        .collect();
    Rule {
        tool: format!("mcp__{server}__{normalized_pat}"),
        action,
        comment: Some(format!(
            "server '{server}' permission rule from mcp.toml ({})",
            match action {
                Action::Allow => "allow",
                Action::Deny => "deny",
                Action::Ask => "ask",
            },
        )),
        reason: None,
        expires_at: None,
    }
}

/// Merge global rules with per-server rules per the spec's documented order:
///
/// `global deny → server (deny → ask → allow) → global (ask → allow)`
///
/// The default-rules catch-all (`*` → Ask) lives at the tail of `global_rules`
/// already (per `caliban-agent-core::permissions::default_rules()`), so we
/// leave it where it sits.
///
/// Returns a freshly-allocated `Vec<Rule>` suitable to hand to
/// [`caliban_agent_core::PermissionsHook::new`].
#[must_use]
pub fn merge_with_global(
    global_rules: Vec<Rule>,
    servers: &BTreeMap<String, ServerConfig>,
) -> Vec<Rule> {
    let server_rules = compile_server_permission_rules(servers);
    if server_rules.is_empty() {
        return global_rules;
    }
    // Partition `global_rules` by action class — *all* global denies (not just
    // a contiguous head run) move ahead of the per-server block, then the rest
    // (ask/allow/default) follow. A positional "first non-deny" split is wrong:
    // the real chain is `[cli allow…, cli/settings deny…, default_rules…]`, so a
    // non-Deny rule routinely precedes a global deny (e.g. `deny:mcp__*`), and a
    // positional split would splice the server block — including server `allow`s
    // — ahead of that deny, letting a server allow beat it (QA R3 #213).
    // Relative order within each class is preserved (stable partition).
    let mut global_deny: Vec<Rule> = Vec::new();
    let mut global_rest: Vec<Rule> = Vec::new();
    for r in global_rules {
        if matches!(r.action, Action::Deny) {
            global_deny.push(r);
        } else {
            global_rest.push(r);
        }
    }
    let mut out: Vec<Rule> = global_deny;
    out.extend(server_rules);
    out.extend(global_rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServerPermissions, TransportKind};
    use std::path::PathBuf;

    fn server_with_permissions(perms: ServerPermissions) -> ServerConfig {
        ServerConfig {
            transport: TransportKind::Stdio,
            command: "noop".to_string(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: Option::<PathBuf>::None,
            url: None,
            headers: BTreeMap::new(),
            oauth: crate::config::OauthMode::Off,
            manual_oauth: crate::oauth::ManualOauthConfig::default(),
            disabled: false,
            lazy: None,
            permissions: perms,
        }
    }

    #[test]
    fn compile_produces_deny_then_ask_then_allow() {
        let mut servers = BTreeMap::new();
        servers.insert(
            "linear".to_string(),
            server_with_permissions(ServerPermissions {
                allow: vec!["read_*".to_string()],
                deny: vec!["delete_*".to_string()],
                ask: vec!["create_*".to_string()],
            }),
        );
        let rules = compile_server_permission_rules(&servers);
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].tool, "mcp__linear__delete_*");
        assert_eq!(rules[0].action, Action::Deny);
        assert_eq!(rules[1].tool, "mcp__linear__create_*");
        assert_eq!(rules[1].action, Action::Ask);
        assert_eq!(rules[2].tool, "mcp__linear__read_*");
        assert_eq!(rules[2].action, Action::Allow);
    }

    #[test]
    fn merge_with_global_preserves_global_deny_priority() {
        // global denies should still come first; the per-server deny shouldn't
        // be able to override them.
        let global = vec![
            Rule {
                tool: "mcp__*".to_string(),
                action: Action::Deny,
                comment: None,
                reason: None,
                expires_at: None,
            },
            Rule {
                tool: "*".to_string(),
                action: Action::Ask,
                comment: None,
                reason: None,
                expires_at: None,
            },
        ];
        let mut servers = BTreeMap::new();
        servers.insert(
            "linear".to_string(),
            server_with_permissions(ServerPermissions {
                allow: vec!["read_*".to_string()],
                deny: vec![],
                ask: vec![],
            }),
        );
        let merged = merge_with_global(global, &servers);
        // First should still be the global deny.
        assert_eq!(merged[0].tool, "mcp__*");
        assert_eq!(merged[0].action, Action::Deny);
        // Per-server allow goes between global deny and global ask.
        assert_eq!(merged[1].tool, "mcp__linear__read_*");
        // Then the global ask.
        assert_eq!(merged.last().unwrap().tool, "*");
    }

    #[test]
    fn merge_with_global_keeps_deny_priority_when_a_non_deny_precedes_it() {
        // Regression for QA R3 #213: the real global chain is
        // `[cli allow…, cli/settings deny…, default_rules…]`, so a non-Deny
        // rule (e.g. a CLI `--allow`) sits *ahead* of the global `deny:mcp__*`.
        // The old positional split stopped at that first non-Deny and spliced
        // the per-server block — including server `allow`s — ahead of the
        // global deny, letting a server allow beat `deny mcp__*`.
        let global = vec![
            Rule {
                tool: "Read".to_string(),
                action: Action::Allow,
                comment: None,
                reason: None,
                expires_at: None,
            },
            Rule {
                tool: "mcp__*".to_string(),
                action: Action::Deny,
                comment: None,
                reason: None,
                expires_at: None,
            },
            Rule {
                tool: "*".to_string(),
                action: Action::Ask,
                comment: None,
                reason: None,
                expires_at: None,
            },
        ];
        let mut servers = BTreeMap::new();
        servers.insert(
            "linear".to_string(),
            server_with_permissions(ServerPermissions {
                allow: vec!["read_*".to_string()],
                deny: vec![],
                ask: vec![],
            }),
        );
        let merged = merge_with_global(global, &servers);
        // The global `deny:mcp__*` must precede the server `allow` so that
        // first-match-wins blocks `mcp__linear__read_issue`.
        let deny_idx = merged
            .iter()
            .position(|r| r.tool == "mcp__*" && r.action == Action::Deny)
            .expect("global deny present");
        let server_allow_idx = merged
            .iter()
            .position(|r| r.tool == "mcp__linear__read_*")
            .expect("server allow present");
        assert!(
            deny_idx < server_allow_idx,
            "global deny (idx {deny_idx}) must come before server allow (idx {server_allow_idx}); merged={:?}",
            merged
                .iter()
                .map(|r| (&r.tool, r.action))
                .collect::<Vec<_>>(),
        );
    }
}
