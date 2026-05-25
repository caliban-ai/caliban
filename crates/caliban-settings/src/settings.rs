//! The strongly-typed [`Settings`] struct + supporting types.
//!
//! Unknown top-level keys are tolerated for forward-compat (captured in
//! [`Settings::extra`] via `#[serde(flatten)]`).
//!
//! The struct intentionally mirrors the *top-level* shape of
//! `settings.json` but does **not** redefine the deep types owned by
//! other crates (e.g. `caliban_mcp_client::ServerConfig`). Instead the
//! settings crate keeps these top-level slices as `serde_json::Value`
//! / lightweight projection structs and exposes converter helpers (see
//! [`Settings::mcp_config`] etc.) so callers continue to receive the
//! existing per-crate types.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// Permission rule arrays. Mirrors the `permissions` block of a Claude-
/// Code-compatible `settings.json`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Permissions {
    /// Tools/patterns that auto-allow.
    pub allow: Vec<String>,
    /// Tools/patterns that prompt the user (`Ask`).
    pub ask: Vec<String>,
    /// Tools/patterns that hard-deny.
    pub deny: Vec<String>,
    /// Forward-compat container for unknown keys nested under `permissions`.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Model selector
// ---------------------------------------------------------------------------

/// `model` is either a bare string or a `{ provider, name }` object.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ModelSelector {
    /// Bare string form, e.g. `"claude-sonnet-4-7"`.
    Name(String),
    /// Fully-qualified form, e.g. `{ provider = "anthropic", name = "claude-sonnet-4-7" }`.
    Qualified {
        /// Provider id (e.g. `"anthropic"`, `"openai"`).
        provider: String,
        /// Model name within the provider.
        name: String,
    },
}

impl ModelSelector {
    /// Render `provider/name` (when qualified) or just `name`.
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Name(n) => n.clone(),
            Self::Qualified { provider, name } => format!("{provider}/{name}"),
        }
    }
}

// ---------------------------------------------------------------------------
// MCP server (projection of caliban_mcp_client::ServerConfig)
// ---------------------------------------------------------------------------

/// Projection of `caliban_mcp_client::ServerConfig` carried by Settings.
///
/// The crate wraps rather than re-exports the foreign type so we don't
/// take a hard dependency on `mcp-client`'s serde shape changing (and so
/// the MCP v2 sibling work can evolve the foreign struct independently).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpServerSetting {
    /// Executable command.
    pub command: String,
    /// Argv after the command.
    pub args: Vec<String>,
    /// Environment variables.
    pub env: BTreeMap<String, String>,
    /// Working directory override.
    pub cwd: Option<PathBuf>,
    /// Mark this server as disabled.
    pub disabled: bool,
}

// ---------------------------------------------------------------------------
// api_key_helper raw form
// ---------------------------------------------------------------------------

/// The raw shape of the `api_key_helper` setting before promotion to
/// the [`crate::ApiKeyHelperSpec`] pool. Either a bare command string,
/// a single object, or an array of provider-keyed objects.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ApiKeyHelperRaw {
    /// `"api_key_helper": "/path/to/script"`.
    Command(String),
    /// `"api_key_helper": { "command": "...", "provider": "..." }`.
    Object(BTreeMap<String, serde_json::Value>),
    /// `"api_key_helper": [ { ... }, { ... } ]`.
    List(Vec<BTreeMap<String, serde_json::Value>>),
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Top-level merged settings.
///
/// Most fields are `Option<…>` so the merger can tell "scope didn't
/// declare this" apart from "scope explicitly set the default value".
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    // ----- model / agent ----------------------------------------------------
    /// Agent profile name (sub-agent dispatch hint).
    pub agent: Option<String>,
    /// Primary model.
    pub model: Option<ModelSelector>,
    /// Fallback model used when the primary errors.
    pub fallback_model: Option<ModelSelector>,
    /// Per-route model overrides (`{ "fast-classifier": "claude-haiku-4-7" }`).
    pub model_overrides: BTreeMap<String, String>,

    // ----- permissions ------------------------------------------------------
    /// Allow / ask / deny rule arrays.
    pub permissions: Permissions,

    // ----- hooks ------------------------------------------------------------
    /// Raw hook event → handler list (passed verbatim to
    /// `caliban_agent_core::HooksConfig`).
    pub hooks: BTreeMap<String, serde_json::Value>,
    /// Kill-switch — disable every external hook handler.
    pub disable_all_hooks: Option<bool>,
    /// When true, only managed-scope hooks fire.
    pub allow_managed_hooks_only: Option<bool>,
    /// HTTP-hook URL allowlist (glob).
    pub allowed_http_hook_urls: Vec<String>,
    /// HTTP-hook env-var allowlist.
    pub http_hook_allowed_env_vars: Vec<String>,

    // ----- MCP --------------------------------------------------------------
    /// Map of server name → server settings.
    pub mcp_servers: BTreeMap<String, McpServerSetting>,

    // ----- router -----------------------------------------------------------
    /// Router config; opaque blob passed through to
    /// `caliban-model-router::discovery`. Kept untyped because the router
    /// crate owns the schema.
    pub router: Option<serde_json::Value>,

    // ----- memory -----------------------------------------------------------
    /// Memory tier knobs (passed to `caliban_memory::MemoryConfig`).
    pub memory: Option<serde_json::Value>,

    // ----- plugins ----------------------------------------------------------
    /// Plugin manager knobs.
    pub plugins: Option<serde_json::Value>,

    // ----- UI ---------------------------------------------------------------
    /// Active output-style name.
    pub output_style: Option<String>,
    /// `vim` or `emacs`-flavored input editing.
    pub editor_mode: Option<String>,
    /// Compact vs. expanded TUI layout.
    pub view_mode: Option<String>,
    /// TUI knobs (theme, etc.). Opaque.
    pub tui: Option<serde_json::Value>,

    // ----- auth -------------------------------------------------------------
    /// Provider API-key supplier(s).
    pub api_key_helper: Option<ApiKeyHelperRaw>,

    // ----- observability ----------------------------------------------------
    /// `OTel` / cost emitter toggle.
    pub enable_telemetry: Option<bool>,

    // ----- managed-scope escape hatch ---------------------------------------
    /// When set in the managed scope, flips the managed layer to the top
    /// of the merge chain (enterprise lockdown). The string value
    /// `"block"` enables the override; `"augment"` (default behavior) is
    /// honored as a no-op.
    pub parent_settings_behavior: Option<String>,

    // ----- miscellaneous ----------------------------------------------------
    /// Extra workspace roots to consult.
    pub additional_directories: Vec<PathBuf>,
    /// `claudeMdExcludes` (passed to `caliban_memory`).
    pub claude_md_excludes: Vec<String>,
    /// Environment-variable overrides applied to child processes.
    pub env: BTreeMap<String, String>,

    // ----- forward-compat ---------------------------------------------------
    /// Any unknown top-level keys land here so we don't error on
    /// forward-compat fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Settings {
    /// Convert the MCP-server slice into a `caliban_mcp_client::McpConfig`.
    ///
    /// We construct the type via its public fields so the conversion
    /// continues to compile even when the foreign crate evolves
    /// independently (per the MCP v2 sibling spec).
    #[must_use]
    pub fn mcp_config(&self) -> caliban_mcp_client::McpConfig {
        let mut servers = std::collections::BTreeMap::new();
        for (name, s) in &self.mcp_servers {
            servers.insert(
                name.clone(),
                caliban_mcp_client::ServerConfig {
                    transport: caliban_mcp_client::TransportKind::Stdio,
                    command: s.command.clone(),
                    args: s.args.clone(),
                    env: s.env.clone(),
                    cwd: s.cwd.clone(),
                    url: None,
                    headers: std::collections::BTreeMap::new(),
                    oauth: caliban_mcp_client::OauthMode::Off,
                    manual_oauth: caliban_mcp_client::ManualOauthConfig::default(),
                    disabled: s.disabled,
                    permissions: caliban_mcp_client::ServerPermissions::default(),
                },
            );
        }
        caliban_mcp_client::McpConfig { servers }
    }

    /// Convert the `permissions` arrays into a flat `Rule[]` suitable
    /// for `PermissionsHook`. Order: `deny` > `ask` > `allow` (matches
    /// the documented evaluation order in ADR 0020).
    #[must_use]
    pub fn permission_rules(&self) -> Vec<caliban_agent_core::Rule> {
        use caliban_agent_core::{Action, Rule};
        let mut out = Vec::new();
        for p in &self.permissions.deny {
            out.push(Rule {
                tool: p.clone(),
                action: Action::Deny,
                comment: None,
            });
        }
        for p in &self.permissions.ask {
            out.push(Rule {
                tool: p.clone(),
                action: Action::Ask,
                comment: None,
            });
        }
        for p in &self.permissions.allow {
            out.push(Rule {
                tool: p.clone(),
                action: Action::Allow,
                comment: None,
            });
        }
        out
    }

    /// Whether the managed scope is requesting top-priority override.
    #[must_use]
    pub fn parent_blocks(&self) -> bool {
        self.parent_settings_behavior.as_deref() == Some("block")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_minimal() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert!(s.model.is_none());
        assert!(s.permissions.allow.is_empty());
        assert!(s.extra.is_empty());
    }

    #[test]
    fn model_selector_accepts_both_shapes() {
        let s: Settings = serde_json::from_str(r#"{"model": "claude-sonnet-4-7"}"#).unwrap();
        assert!(matches!(s.model, Some(ModelSelector::Name(_))));
        let s: Settings = serde_json::from_str(
            r#"{"model": {"provider": "anthropic", "name": "claude-sonnet-4-7"}}"#,
        )
        .unwrap();
        assert!(matches!(s.model, Some(ModelSelector::Qualified { .. })));
    }

    #[test]
    fn unknown_top_level_keys_land_in_extra() {
        let raw = r#"{"some_future_key": 42}"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        assert_eq!(
            s.extra
                .get("some_future_key")
                .and_then(serde_json::Value::as_i64),
            Some(42)
        );
    }

    #[test]
    fn permissions_to_rules_preserves_order() {
        let s = Settings {
            permissions: Permissions {
                allow: vec!["Read".into()],
                ask: vec!["Bash".into()],
                deny: vec!["Write:**".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let rules = s.permission_rules();
        assert_eq!(rules.len(), 3);
        // deny > ask > allow
        assert_eq!(rules[0].action, caliban_agent_core::Action::Deny);
        assert_eq!(rules[1].action, caliban_agent_core::Action::Ask);
        assert_eq!(rules[2].action, caliban_agent_core::Action::Allow);
    }

    #[test]
    fn mcp_config_conversion() {
        let mut srv = BTreeMap::new();
        srv.insert(
            "linear".to_string(),
            McpServerSetting {
                command: "npx".into(),
                args: vec!["-y".into(), "@linear/mcp-server".into()],
                ..Default::default()
            },
        );
        let s = Settings {
            mcp_servers: srv,
            ..Default::default()
        };
        let cfg = s.mcp_config();
        assert_eq!(cfg.servers.len(), 1);
        assert_eq!(cfg.servers["linear"].command, "npx");
    }
}
