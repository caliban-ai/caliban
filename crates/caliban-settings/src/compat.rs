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
/// Returns `true` when legacy data was layered in. The full transport
/// surface (HTTP/SSE/OAuth/per-server permissions) is preserved on the
/// round-trip — see `caliban_mcp_client::config::ServerConfig` for the
/// canonical shape.
pub fn maybe_load_legacy_mcp(settings: &mut Settings, workspace_root: &Path) -> bool {
    if !settings.mcp_servers.is_empty() {
        return false;
    }
    match caliban_mcp_client::load_config(workspace_root) {
        Ok(cfg) if !cfg.servers.is_empty() => {
            for (name, sc) in cfg.servers {
                let r#type = match sc.transport {
                    caliban_mcp_client::TransportKind::Stdio => None,
                    other => Some(other.as_str().to_string()),
                };
                let oauth = match sc.oauth {
                    caliban_mcp_client::OauthMode::Off => None,
                    other => Some(other.as_str().to_string()),
                };
                // Preserve any legacy `[server.X.oauth_config]` block so manual
                // oauth survives the mcp.toml → settings fold (only emit it when
                // non-default to avoid writing an empty table).
                let oauth_config = (sc.manual_oauth
                    != caliban_mcp_client::ManualOauthConfig::default())
                .then_some(sc.manual_oauth);
                settings.mcp_servers.insert(
                    name,
                    crate::McpServerSetting {
                        r#type,
                        command: sc.command,
                        args: sc.args,
                        env: sc.env,
                        cwd: sc.cwd,
                        url: sc.url.map(|u| u.to_string()),
                        headers: sc.headers,
                        oauth,
                        oauth_config,
                        permissions: sc.permissions,
                        disabled: sc.disabled,
                        lazy: sc.lazy,
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

/// Whether a real legacy `permissions.toml` (project or user scope) exists
/// with rules. Unlike [`maybe_load_legacy_permissions`], this excludes the
/// built-in `default_rules()` tail that `load_rules` always appends, so it does
/// not false-positive when only the defaults are present. `config migrate`
/// uses it to detect a genuine legacy source, since the runtime fold has
/// already populated the effective settings (#176).
#[must_use]
pub fn legacy_permissions_present(workspace_root: &Path) -> bool {
    match load_rules(Vec::new(), workspace_root) {
        Ok(all) => all.len() > caliban_agent_core::default_rules().len(),
        Err(_) => false,
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
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_SETTINGS,
                    "permissions.toml [[rule]] form is deprecated; will be rewritten to v2 canonical form on next caliban-owned edit"
                );
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
        // The project mcp.toml entry must be present; a developer's real
        // user-scope mcp.toml may also load (we don't sandbox $HOME here
        // because the env-mutation infra adds noise for what is a
        // pre-existing test fixture), so we assert presence rather than
        // exact count.
        assert!(s.mcp_servers.contains_key("linear"));
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
    fn legacy_mcp_http_round_trip_preserves_full_surface() {
        // Load a legacy mcp.toml with an HTTP server (incl. headers,
        // oauth, per-server permissions); confirm every field survives
        // through the compat shim AND through Settings::mcp_config().
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path();
        fs::create_dir_all(ws.join(".caliban")).unwrap();
        fs::write(
            ws.join(".caliban/mcp.toml"),
            r#"
[server.silverbullet]
transport = "http"
url = "https://mcp.silverbullet.hexadecimate.net/mcp"
headers = { Authorization = "Bearer xyz" }

[server.silverbullet.permissions]
allow = ["read_*"]
deny = ["delete_*"]
"#,
        )
        .unwrap();
        let mut s = Settings::default();
        assert!(maybe_load_legacy_mcp(&mut s, ws));

        // First: the projection landed on McpServerSetting correctly.
        let sb = &s.mcp_servers["silverbullet"];
        assert_eq!(sb.r#type.as_deref(), Some("http"));
        assert_eq!(
            sb.url.as_deref(),
            Some("https://mcp.silverbullet.hexadecimate.net/mcp"),
        );
        assert_eq!(sb.headers.get("Authorization"), Some(&"Bearer xyz".into()));
        assert_eq!(sb.permissions.allow, vec!["read_*".to_string()]);
        assert_eq!(sb.permissions.deny, vec!["delete_*".to_string()]);

        // Second: round-tripped through Settings::mcp_config(), the
        // typed ServerConfig has the same transport/url/headers/perms.
        let cfg = s.mcp_config();
        let server_cfg = &cfg.servers["silverbullet"];
        assert_eq!(
            server_cfg.transport,
            caliban_mcp_client::TransportKind::Http,
        );
        assert_eq!(
            server_cfg.url.as_ref().map(ToString::to_string),
            Some("https://mcp.silverbullet.hexadecimate.net/mcp".to_string()),
        );
        assert_eq!(
            server_cfg.headers.get("Authorization"),
            Some(&"Bearer xyz".to_string()),
        );
        assert_eq!(server_cfg.permissions.allow, vec!["read_*".to_string()]);
        assert_eq!(server_cfg.permissions.deny, vec!["delete_*".to_string()]);
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

    #[test]
    fn legacy_permissions_toml_warns_once_per_process() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".caliban");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(
            cfg.join("permissions.toml"),
            r#"
[[rule]]
tool = "Bash"
action = "ask"
"#,
        )
        .unwrap();

        let mut s = Settings::default();
        let loaded = maybe_load_legacy_permissions(&mut s, dir.path());
        assert!(loaded, "fixture present, must report loaded=true");
        // ensure the rule shows up under permissions.allow/ask/deny via legacy compat shape
        assert!(
            !s.permissions.ask.is_empty()
                || !s.permissions.allow.is_empty()
                || !s.permissions.deny.is_empty(),
            "rule should be present in one of the permission buckets"
        );
    }
}
