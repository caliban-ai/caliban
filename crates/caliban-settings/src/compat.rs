//! Backward-compatibility helpers.
//!
//! When the unified `settings.json` does **not** define a given top-
//! level key, the legacy per-feature TOML for that key still loads
//! transparently. This module bridges between the new typed `Settings`
//! and the existing ad-hoc loaders in:
//!
//! - `caliban_mcp_client::load_config` (`mcp.toml`)
//! - `caliban_agent_core::permissions::load_rules` (`permissions.toml`)
//! - `caliban_agent_core::HooksConfig::load` (`hooks.toml`)
//!
//! All three legacy entry points are `#[deprecated]` in favor of
//! [`crate::load_settings`]; this module is the single sanctioned consumer
//! during the one-release compat window.

#![allow(deprecated)]

use std::path::Path;

use caliban_agent_core::{Action, load_rules};

use crate::Settings;

/// Fold the project + user `mcp.toml` (the existing loader) into
/// `settings.mcp_servers` **only when** the unified settings did not
/// already define any MCP servers.
///
/// Returns `true` when legacy data was layered in.
pub fn maybe_load_legacy_mcp(settings: &mut Settings, workspace_root: &Path) -> bool {
    if !settings.mcp_servers.is_empty() {
        return false;
    }
    match caliban_mcp_client::load_config(workspace_root) {
        Ok(cfg) if !cfg.servers.is_empty() => {
            for (name, sc) in cfg.servers {
                settings.mcp_servers.insert(
                    name,
                    crate::McpServerSetting {
                        command: sc.command,
                        args: sc.args,
                        env: sc.env,
                        cwd: sc.cwd,
                        disabled: sc.disabled,
                    },
                );
            }
            tracing::info!(target: caliban_common::tracing_targets::TARGET_SETTINGS, "loaded legacy mcp.toml as fallback (compat)");
            true
        }
        Ok(_) => false,
        Err(e) => {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_SETTINGS, error = %e, "mcp.toml legacy load failed");
            false
        }
    }
}

/// Fold the project + user `permissions.toml` into
/// `settings.permissions` **only when** the unified settings did not
/// already define any permission rules.
pub fn maybe_load_legacy_permissions(settings: &mut Settings, workspace_root: &Path) -> bool {
    let any = !settings.permissions.allow.is_empty()
        || !settings.permissions.ask.is_empty()
        || !settings.permissions.deny.is_empty();
    if any {
        return false;
    }
    match load_rules(Vec::new(), workspace_root) {
        Ok(rules) => {
            let mut found_any = false;
            for r in rules {
                match r.action {
                    Action::Allow => settings.permissions.allow.push(r.tool),
                    Action::Ask => settings.permissions.ask.push(r.tool),
                    Action::Deny => settings.permissions.deny.push(r.tool),
                }
                found_any = true;
            }
            if found_any {
                tracing::info!(target: caliban_common::tracing_targets::TARGET_SETTINGS, "loaded legacy permissions.toml as fallback (compat)");
            }
            found_any
        }
        Err(e) => {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_SETTINGS, error = %e, "permissions.toml legacy load failed");
            false
        }
    }
}

/// Fold the project + user `hooks.toml` into `settings.hooks` **only
/// when** the unified settings did not already define hook events.
pub fn maybe_load_legacy_hooks(settings: &mut Settings, workspace_root: &Path) -> bool {
    if !settings.hooks.is_empty() {
        return false;
    }
    match caliban_agent_core::HooksConfig::load(workspace_root) {
        Ok(cfg) if cfg.total_handler_count() > 0 || cfg.disable_all_hooks => {
            // We can't faithfully serialize the typed `HooksConfig`
            // back to the loose `serde_json::Value` shape (foreign-
            // type), so we record presence via a sentinel — the
            // caller-side compat shim in `caliban/src/main.rs` will
            // continue using the typed loader when this sentinel is
            // present.
            settings.hooks.insert(
                "__legacy_hooks_toml__".into(),
                serde_json::json!({"handler_count": cfg.total_handler_count()}),
            );
            if cfg.disable_all_hooks {
                settings.disable_all_hooks = Some(true);
            }
            if cfg.allow_managed_hooks_only {
                settings.allow_managed_hooks_only = Some(true);
            }
            settings
                .allowed_http_hook_urls
                .extend(cfg.allowed_http_hook_urls);
            settings
                .http_hook_allowed_env_vars
                .extend(cfg.http_hook_allowed_env_vars);
            tracing::info!(target: caliban_common::tracing_targets::TARGET_SETTINGS, "loaded legacy hooks.toml as fallback (compat)");
            true
        }
        Ok(_) => false,
        Err(e) => {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_SETTINGS, error = %e, "hooks.toml legacy load failed");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn legacy_mcp_loads_when_unified_empty() {
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
        assert_eq!(s.mcp_servers.len(), 1);
        assert_eq!(s.mcp_servers["linear"].command, "npx");
    }

    #[test]
    fn legacy_mcp_skipped_when_unified_present() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path();
        fs::create_dir_all(ws.join(".caliban")).unwrap();
        fs::write(
            ws.join(".caliban/mcp.toml"),
            "[server.linear]\ncommand = \"legacy\"\n",
        )
        .unwrap();
        let mut s = Settings::default();
        s.mcp_servers.insert(
            "existing".into(),
            crate::McpServerSetting {
                command: "fresh".into(),
                ..Default::default()
            },
        );
        assert!(!maybe_load_legacy_mcp(&mut s, ws));
        assert_eq!(s.mcp_servers.len(), 1);
        assert!(s.mcp_servers.contains_key("existing"));
    }

    #[test]
    fn legacy_permissions_loads_when_unified_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path();
        fs::create_dir_all(ws.join(".caliban")).unwrap();
        fs::write(
            ws.join(".caliban/permissions.toml"),
            r#"
[[rule]]
tool = "Bash:rm *"
action = "deny"
"#,
        )
        .unwrap();
        let mut s = Settings::default();
        assert!(maybe_load_legacy_permissions(&mut s, ws));
        assert!(s.permissions.deny.iter().any(|x| x == "Bash:rm *"));
    }
}
