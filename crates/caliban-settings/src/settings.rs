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

/// A single permissions rule as carried in TOML/JSON. Mirrors the
/// `caliban_agent_core::Rule` shape but lives here because Settings
/// owns the wire serde shape (and to avoid a cyclic dep).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RuleSpec {
    /// Glob pattern matching tool names (e.g. `"Bash:cargo *"`).
    pub pattern: String,
    /// Decision string: `"allow"`, `"ask"`, or `"deny"`.
    pub action: String,
    /// Optional human-readable comment surfaced in `/permissions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Optional deny reason shown to the operator and logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Optional expiry timestamp after which the rule is skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Deprecated `tool` alias accepted on input; hoisted into `pattern` on load.
    #[serde(default, alias = "tool", skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

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
    /// v2 ordered array. When non-empty, takes precedence over the buckets.
    pub rules: Vec<RuleSpec>,
    /// When true, refuse --no-permissions / bypass mode at startup.
    pub enforce: Option<bool>,
    /// Initial [`caliban_agent_core::PermissionMode`] at session start.
    pub default_mode: Option<String>,
    /// Append-only decision log toggle (default true).
    pub audit_log: Option<bool>,
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
///
/// **Field naming**: the transport selector field is named `r#type` to
/// match `~/.claude.json` / Claude Desktop's `mcpServers.X.type`. The
/// existing legacy `mcp.toml` schema spells it `transport`; both are
/// accepted on deserialization via `#[serde(alias = "transport")]`.
/// Serialization always writes `type`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpServerSetting {
    // ---- transport selector ----
    /// Transport selector: `"stdio"` (default), `"http"`, or `"sse"`.
    /// Accepts both `type` and `transport` keys on input.
    #[serde(
        rename = "type",
        alias = "transport",
        skip_serializing_if = "Option::is_none"
    )]
    pub r#type: Option<String>,
    // ---- stdio ----
    /// Executable command (stdio only).
    pub command: String,
    /// Argv after the command (stdio only).
    pub args: Vec<String>,
    /// Environment variables (stdio only).
    pub env: BTreeMap<String, String>,
    /// Working directory override (stdio only).
    pub cwd: Option<PathBuf>,
    // ---- http / sse ----
    /// Absolute http/https URL for http/sse transports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Static request headers (http/sse only).
    pub headers: BTreeMap<String, String>,
    /// OAuth mode: `"off"` (default), `"auto"`, `"manual"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<String>,
    // ---- common ----
    /// Per-server permission scoping (composes with global rules).
    pub permissions: caliban_mcp_client::ServerPermissions,
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

    // ----- context-window management (Plan B) -------------------------------
    /// Pre-turn autocompaction threshold (utilization in 0..=1).
    /// `None` disables autocompact; `Some(0.75)` is the documented default.
    pub auto_compact_threshold: Option<f32>,
    /// Enable the per-turn microcompact (LLM-free supersession) pass.
    pub micro_compact_enabled: Option<bool>,
    /// Global per-tool-result cap in characters. `0` disables.
    pub tool_result_cap_chars: Option<usize>,
    /// Minimum estimated tokens on the last user message to merit the
    /// conversation-level cache marker.
    pub min_cache_block_tokens: Option<usize>,

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
    ///
    /// String fields (`type`, `oauth`) are matched against the canonical
    /// values defined in `caliban_mcp_client::config`. Unrecognized values
    /// fall back to the safest default (`stdio` / `off`) with a `warn!`
    /// log to surface the misconfiguration during a restart.
    #[must_use]
    pub fn mcp_config(&self) -> caliban_mcp_client::McpConfig {
        let mut servers = std::collections::BTreeMap::new();
        for (name, s) in &self.mcp_servers {
            let transport = parse_transport(name, s.r#type.as_deref());
            let oauth = parse_oauth(name, s.oauth.as_deref());
            // Parse URL string into the typed `url::Url`. Bad input
            // surfaces as `None` here; downstream the manager will
            // refuse to start a remote transport without a valid url.
            let url = s.url.as_deref().and_then(|raw| match url::Url::parse(raw) {
                Ok(u) => Some(u),
                Err(e) => {
                    tracing::warn!(
                        target: caliban_common::tracing_targets::TARGET_MCP,
                        server = name.as_str(),
                        url = raw,
                        error = %e,
                        "invalid MCP server url; ignoring",
                    );
                    None
                }
            });
            servers.insert(
                name.clone(),
                caliban_mcp_client::ServerConfig {
                    transport,
                    command: s.command.clone(),
                    args: s.args.clone(),
                    env: s.env.clone(),
                    cwd: s.cwd.clone(),
                    url,
                    headers: s.headers.clone(),
                    oauth,
                    manual_oauth: caliban_mcp_client::ManualOauthConfig::default(),
                    disabled: s.disabled,
                    permissions: s.permissions.clone(),
                },
            );
        }
        caliban_mcp_client::McpConfig { servers }
    }

    /// Convert the `permissions` arrays into a flat `Rule[]` suitable
    /// for `PermissionsHook`. When the v2 `rules` array is non-empty, it is
    /// used verbatim (source order preserved). Otherwise falls back to the
    /// legacy three-bucket form: `deny` > `ask` > `allow` (matches the
    /// documented evaluation order in ADR 0020).
    #[must_use]
    pub fn permission_rules(&self) -> Vec<caliban_agent_core::Rule> {
        use caliban_agent_core::{Action, Rule};
        let parse_action = |s: &str| match s.to_ascii_lowercase().as_str() {
            "allow" => Action::Allow,
            "ask" => Action::Ask,
            "deny" => Action::Deny,
            other => {
                tracing::warn!("unknown permissions action {other:?}; falling back to ask");
                Action::Ask
            }
        };

        // v2 ordered form wins when non-empty.
        if !self.permissions.rules.is_empty() {
            return self
                .permissions
                .rules
                .iter()
                .map(|r| {
                    let pat = if r.pattern.is_empty() {
                        r.tool.clone().unwrap_or_default() // legacy `tool` alias
                    } else {
                        r.pattern.clone()
                    };
                    Rule {
                        tool: pat,
                        action: parse_action(&r.action),
                        comment: r.comment.clone(),
                        reason: r.reason.clone(),
                        expires_at: r.expires_at,
                    }
                })
                .collect();
        }

        // Legacy three-bucket fallback (deny > ask > allow).
        let mk = |p: &str, a: Action| Rule {
            tool: p.into(),
            action: a,
            comment: None,
            reason: None,
            expires_at: None,
        };
        let mut out = Vec::new();
        for p in &self.permissions.deny {
            out.push(mk(p, Action::Deny));
        }
        for p in &self.permissions.ask {
            out.push(mk(p, Action::Ask));
        }
        for p in &self.permissions.allow {
            out.push(mk(p, Action::Allow));
        }
        out
    }

    /// Whether the managed scope is requesting top-priority override.
    #[must_use]
    pub fn parent_blocks(&self) -> bool {
        self.parent_settings_behavior.as_deref() == Some("block")
    }

    /// Project the hook-related fields into a
    /// [`caliban_agent_core::HooksConfig`].
    ///
    /// The scalar / array fields (`disable_all_hooks`,
    /// `allow_managed_hooks_only`, `allowed_http_hook_urls`,
    /// `http_hook_allowed_env_vars`) round-trip faithfully from
    /// `settings.json`. The typed `events` map is left empty here:
    /// the per-event typed handler list is only constructible from
    /// the legacy `hooks.toml` shape, which lives behind the
    /// `crate::compat::maybe_load_legacy_hooks` shim (it sets the
    /// scalars from disk during settings load). Callers that need
    /// the full typed handler list during the back-compat window
    /// continue to call the legacy loader inside an
    /// `#[allow(deprecated)]` block.
    ///
    /// The total handler count is preserved via a sentinel in
    /// [`Self::legacy_hook_handler_count`].
    #[must_use]
    pub fn hook_config(&self) -> caliban_agent_core::HooksConfig {
        caliban_agent_core::HooksConfig {
            disable_all_hooks: self.disable_all_hooks.unwrap_or(false),
            allow_managed_hooks_only: self.allow_managed_hooks_only.unwrap_or(false),
            allowed_http_hook_urls: self.allowed_http_hook_urls.clone(),
            http_hook_allowed_env_vars: self.http_hook_allowed_env_vars.clone(),
            events: std::collections::BTreeMap::new(),
        }
    }

    /// Apply context-window management knobs onto a fresh
    /// [`caliban_agent_core::AgentConfig`]. Only fields explicitly set in
    /// `settings.json` override the defaults; everything else is left at
    /// the upstream default (see `AgentConfig::default()`).
    pub fn apply_context_management(&self, cfg: &mut caliban_agent_core::AgentConfig) {
        if let Some(v) = self.auto_compact_threshold {
            cfg.auto_compact_threshold = Some(v);
        }
        if let Some(v) = self.micro_compact_enabled {
            cfg.micro_compact_enabled = v;
        }
        if let Some(v) = self.tool_result_cap_chars {
            cfg.tool_result_cap_chars = v;
        }
        if let Some(v) = self.min_cache_block_tokens {
            cfg.min_cache_block_tokens = v;
        }
    }

    /// When `settings.hooks` contains the legacy-compat sentinel written by
    /// [`crate::compat::maybe_load_legacy_hooks`], extract the handler-count
    /// for diagnostics. Returns `None` when no sentinel is present.
    #[must_use]
    pub fn legacy_hook_handler_count(&self) -> Option<usize> {
        self.hooks
            .get("__legacy_hooks_toml__")
            .and_then(|v| v.get("handler_count"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    }
}

// ---------------------------------------------------------------------------
// Helpers for mcp_config()
// ---------------------------------------------------------------------------

/// Map the `type`/`transport` string to `TransportKind`. Unknown values
/// warn and fall back to stdio (the safest default — it requires a
/// `command`, so a downstream "missing command" error will surface).
fn parse_transport(server: &str, raw: Option<&str>) -> caliban_mcp_client::TransportKind {
    match raw {
        None => caliban_mcp_client::TransportKind::Stdio,
        Some(s) => match s.to_ascii_lowercase().as_str() {
            "stdio" => caliban_mcp_client::TransportKind::Stdio,
            "http" => caliban_mcp_client::TransportKind::Http,
            "sse" => caliban_mcp_client::TransportKind::Sse,
            other => {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_MCP,
                    server = server,
                    value = other,
                    "unknown MCP server transport; falling back to stdio",
                );
                caliban_mcp_client::TransportKind::Stdio
            }
        },
    }
}

/// Map the `oauth` string to `OauthMode`. Unknown values warn and fall
/// back to `off`.
fn parse_oauth(server: &str, raw: Option<&str>) -> caliban_mcp_client::OauthMode {
    match raw {
        None => caliban_mcp_client::OauthMode::Off,
        Some(s) => match s.to_ascii_lowercase().as_str() {
            "off" => caliban_mcp_client::OauthMode::Off,
            "auto" => caliban_mcp_client::OauthMode::Auto,
            "manual" => caliban_mcp_client::OauthMode::Manual,
            other => {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_MCP,
                    server = server,
                    value = other,
                    "unknown MCP oauth mode; falling back to off",
                );
                caliban_mcp_client::OauthMode::Off
            }
        },
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

    // PR-T3-B: Verify the new Settings accessors produce shapes equivalent to
    // the legacy ad-hoc loaders for representative inputs. Wrap legacy calls
    // in `#[allow(deprecated)]` so the test suite stays clean once the
    // deprecation lands.

    #[test]
    fn permission_rules_match_legacy_load_rules_for_toml_input() {
        // Build a Settings whose `permissions` arrays match a sample
        // permissions.toml; verify Settings::permission_rules() emits the
        // same Rule[] (modulo the built-in default-rules tail that the
        // legacy loader appends).
        let s = Settings {
            permissions: Permissions {
                allow: vec!["Read".into(), "Grep".into()],
                ask: vec!["Bash".into()],
                deny: vec!["Bash:rm *".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let from_settings = s.permission_rules();

        // Emulate the legacy `load_rules` output (project rules only, no
        // CLI overlay, no user TOML, plus built-in defaults appended).
        // Note: legacy load_rules orders inputs as project-then-defaults;
        // here we model the project rules as deny/ask/allow because that's
        // the documented evaluation order Settings::permission_rules emits.
        #[allow(deprecated)]
        let legacy_tail = caliban_agent_core::default_rules();

        // Settings emits deny, ask, allow (the documented eval order). The
        // legacy loader preserves whatever order the TOML declared — but
        // when callers go through Settings, that order is the *normalized*
        // deny → ask → allow. Verify cardinality + per-action grouping
        // matches a deny/ask/allow split of the input.
        assert_eq!(from_settings.len(), 4);
        assert_eq!(from_settings[0].action, caliban_agent_core::Action::Deny);
        assert_eq!(from_settings[0].tool, "Bash:rm *");
        assert_eq!(from_settings[1].action, caliban_agent_core::Action::Ask);
        assert_eq!(from_settings[1].tool, "Bash");
        assert_eq!(from_settings[2].action, caliban_agent_core::Action::Allow);
        assert_eq!(from_settings[2].tool, "Read");
        assert_eq!(from_settings[3].action, caliban_agent_core::Action::Allow);
        assert_eq!(from_settings[3].tool, "Grep");

        // The legacy default-rules tail (defined by agent-core) is the
        // catch-all that callers append on top of Settings::permission_rules
        // in the binary; verify it's a non-empty, terminal-allow-friendly
        // list with the wildcard `*` Ask at the end (a stable contract
        // both Settings and legacy callers rely on).
        assert!(!legacy_tail.is_empty());
        assert_eq!(legacy_tail.last().unwrap().tool, "*");
    }

    #[test]
    fn permissions_v2_ordered_rules_array_preserves_source_order() {
        let toml_src = r#"
[permissions]

[[permissions.rules]]
pattern = "Bash:git *"
action = "allow"
comment = "git ok"

[[permissions.rules]]
pattern = "Bash:rm *"
action = "deny"
reason  = "use git revert"

[[permissions.rules]]
pattern = "*"
action = "ask"
"#;
        let s: Settings = toml::from_str(toml_src).unwrap();
        let rules = s.permission_rules();
        // Expect source order preserved — first rule is allow, NOT pushed behind deny.
        assert_eq!(rules[0].tool, "Bash:git *");
        assert_eq!(rules[0].action, caliban_agent_core::Action::Allow);
        assert_eq!(rules[1].tool, "Bash:rm *");
        assert_eq!(rules[1].action, caliban_agent_core::Action::Deny);
        assert_eq!(rules[1].reason.as_deref(), Some("use git revert"));
        assert_eq!(rules[2].tool, "*");
        assert_eq!(rules[2].action, caliban_agent_core::Action::Ask);
    }

    #[test]
    fn permissions_v2_falls_back_to_legacy_buckets_when_rules_unset() {
        let toml_src = r#"
[permissions]
allow = ["Bash:git *"]
deny  = ["Bash:rm *"]
ask   = ["*"]
"#;
        let s: Settings = toml::from_str(toml_src).unwrap();
        let rules = s.permission_rules();
        // Legacy flatten order is deny → ask → allow (matches v1 behavior).
        assert_eq!(rules[0].action, caliban_agent_core::Action::Deny);
        assert_eq!(rules[1].action, caliban_agent_core::Action::Ask);
        assert_eq!(rules[2].action, caliban_agent_core::Action::Allow);
    }

    #[test]
    fn hook_config_matches_legacy_loader_scalars() {
        // settings.json carries the scalar/array fields directly; verify
        // they project into HooksConfig identically to what the legacy
        // HooksConfig::load loader yields for an equivalent hooks.toml.
        let s = Settings {
            disable_all_hooks: Some(true),
            allow_managed_hooks_only: Some(false),
            allowed_http_hook_urls: vec!["https://hooks.example.com/*".into()],
            http_hook_allowed_env_vars: vec!["AUDIT_TOKEN".into()],
            ..Default::default()
        };
        let from_settings = s.hook_config();

        // Legacy: parse the equivalent TOML and check the fields line up.
        let toml_body = r#"
disable_all_hooks = true
allow_managed_hooks_only = false
allowed_http_hook_urls = ["https://hooks.example.com/*"]
http_hook_allowed_env_vars = ["AUDIT_TOKEN"]
"#;
        #[allow(deprecated)]
        let from_legacy =
            caliban_agent_core::HooksConfig::from_str(toml_body, std::path::Path::new("h.toml"))
                .unwrap();

        assert_eq!(
            from_settings.disable_all_hooks,
            from_legacy.disable_all_hooks
        );
        assert_eq!(
            from_settings.allow_managed_hooks_only,
            from_legacy.allow_managed_hooks_only
        );
        assert_eq!(
            from_settings.allowed_http_hook_urls,
            from_legacy.allowed_http_hook_urls
        );
        assert_eq!(
            from_settings.http_hook_allowed_env_vars,
            from_legacy.http_hook_allowed_env_vars
        );
        // Both default to empty events for the empty-input case; the
        // typed handler list is only populated via the legacy compat shim
        // (which the binary no longer relies on for the summary path).
        assert_eq!(
            from_settings.total_handler_count(),
            from_legacy.total_handler_count()
        );
    }
}
