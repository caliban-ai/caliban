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

use crate::StatuslineConfig;

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

/// Whether sandboxed Bash commands may reach the network (#406).
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SandboxNetwork {
    /// No egress. Loopback still works, so localhost test servers are fine.
    /// The default whenever the workspace fence is active: a sandboxed command
    /// may read the disk but cannot phone home.
    #[default]
    Deny,
    /// Full egress — the escape hatch for runs that genuinely need `git fetch`,
    /// `cargo` against crates.io, or `gh`. Note this also re-opens the
    /// credential-exfiltration path the fence exists to close, so use it
    /// deliberately.
    Allow,
}

/// `sandbox` block of `settings.json` — OS-sandbox posture for Bash commands
/// under `--workspace` (#406, ADR 0054).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct SandboxSettings {
    /// Egress posture. `None` means "unset" — the fence default (deny) applies.
    /// Overridden by `--sandbox-network` on the CLI.
    pub network: Option<SandboxNetwork>,
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
    /// OAuth client/endpoint config (`[mcp_servers.X.oauth_config]`).
    ///
    /// Required for `oauth = "manual"` (supplies `auth_url` / `token_url` /
    /// `client_id`). For `oauth = "auto"` the endpoints are discovered, but a
    /// `client_id` is still needed here whenever the authorization server does
    /// not support dynamic client registration (e.g. GitHub) — register an
    /// OAuth app and set `client_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_config: Option<caliban_mcp_client::ManualOauthConfig>,
    // ---- common ----
    /// Per-server permission scoping (composes with global rules).
    pub permissions: caliban_mcp_client::ServerPermissions,
    /// Mark this server as disabled.
    pub disabled: bool,
    /// Per-server lazy override (ADR-0046). When `tools.lazy_mcp` is
    /// true globally, individual servers can opt back to eager loading
    /// by setting `lazy = false`. `None` follows the global default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lazy: Option<bool>,
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
// tools — lazy MCP tool loading (ADR-0046)
// ---------------------------------------------------------------------------

/// Knobs for the two-stage tool surface. When `lazy_mcp` is true,
/// MCP tools are dropped from the wire payload until the model
/// activates them via the `ToolSearch` built-in; `max_active_schemas`
/// is the soft LRU cap on the activation set.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ToolsConfig {
    /// Hide MCP tools behind `ToolSearch` activation. Default `false`
    /// in v1; opt-in.
    pub lazy_mcp: Option<bool>,
    /// Soft cap on simultaneously-active MCP tools. LRU eviction
    /// applies when exceeded. Default `24`.
    pub max_active_schemas: Option<usize>,
    /// Inject a proactive skill-invocation nudge into the system prompt
    /// when skills are loaded (a `## Skills` awareness block listing the
    /// available skill names). `None`/`true` = on (default), `false` =
    /// off. See issue #56.
    pub skill_guidance: Option<bool>,
}

// ---------------------------------------------------------------------------
// agent_loop — agent-loop policy surface (ADR 0058, B1 · #661)
// ---------------------------------------------------------------------------

/// The `[agent_loop]` configuration group: a coherent home for agent-loop
/// **policy** knobs — the turn budget and the spiral-containment guards — as
/// distinct from the model-inference knobs (`effort`, `thinking`) that govern a
/// single call. See ADR 0058.
///
/// B1 (#661) gathers the previously hard-coded guards
/// (`no_edit_nudge_threshold`, `empty_turn_nudge_max`, `max_turn_thinking_chars`)
/// plus the existing turn budget (`max_turns`) under this one surface, without
/// changing any default. Later children add the new budgets and guards (B2 time
/// budget, B3 cost budget, B4 divergence guard, B5 verification policy, B6
/// adaptive defaults + named profiles).
///
/// How much verification the system prompt encourages (ADR 0058, B5 · #665).
///
/// This shapes *guidance text* injected into the system prompt — caliban never
/// runs tests on the model's behalf; it shapes when the model is *encouraged* to
/// write and run its own reproduction. The `off`/`verify-when-cheap`/`full` axis
/// is exactly what eval sub-project A controlled (the +8pt / −16pt lever, whose
/// sign flips with model strength — hence the per-profile default in B6).
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum VerificationGuidance {
    /// No verification guidance is injected (behavior-preserving default).
    #[default]
    Off,
    /// Encourage the model to verify *when it is cheap* to do so — write and run
    /// a quick reproduction where the cost is low. The "A" treatment arm.
    VerifyWhenCheap,
    /// Strongly encourage the model to verify its work before finishing.
    Full,
}

/// A named agent-loop **policy profile** (ADR 0058, B6 · #666). Bundles the
/// B2–B5 knobs into a coherent posture, so operators pick an intent rather than
/// tuning each knob. The default profile is chosen from execution context
/// (local vs cloud) and is always overridable by naming one explicitly.
///
/// The profiles differ primarily on the **verification** lever — the one eval
/// sub-project A proved load-bearing, whose sign flips with model strength.
/// Budgets (turn/time/cost) stay opt-in: a profile never imposes a surprising
/// hard cap; set those knobs explicitly when a run needs them.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentLoopProfile {
    /// Cloud default: cost-conservative. Verification off — verification costs
    /// ~1.8× (eval sub-project A), so the cheapest safe posture skips it. A user
    /// who wants a strong cloud model's +8pt self-correction picks
    /// `quality-first` explicitly.
    CostOptimized,
    /// Quality over cost, for a strong model: full verification, no cost cap.
    /// The explicit choice for a capable model where correctness dominates.
    QualityFirst,
    /// Local default: quality-first *paired with a tight divergence guard*.
    /// Local generation is ~free, so verify-when-cheap is on; the divergence
    /// guard (B4, #664) contains the weak-model churn that verification can
    /// otherwise induce. The guard is eval-gated and not yet shipped, so this
    /// profile currently sets `verify-when-cheap` and will enable the guard once
    /// B4 lands.
    LocalGuarded,
}

impl AgentLoopProfile {
    /// The profile's verification-guidance default (ADR 0058, B5 lever).
    #[must_use]
    pub fn verification_guidance(self) -> VerificationGuidance {
        match self {
            Self::CostOptimized => VerificationGuidance::Off,
            Self::QualityFirst => VerificationGuidance::Full,
            Self::LocalGuarded => VerificationGuidance::VerifyWhenCheap,
        }
    }
}

/// Every field is `Option` so an unset key leaves the corresponding
/// [`caliban_agent_core::AgentConfig`] default untouched (behavior-preserving).
// No `Eq`: `cost_budget_usd` is `f64` (only `PartialEq`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AgentLoopConfig {
    /// Hard cap on agent-loop iterations. `None` keeps the built-in default
    /// (50). The `--max-turns` CLI flag takes precedence over this
    /// (CLI > settings > default); the precedence is resolved by the caller
    /// (`startup::compose`), not by [`Settings::apply_agent_loop`], because the
    /// CLI flag must win — the same reason `max_tokens_recovery` is resolved
    /// inline rather than via an `apply_*` overlay.
    pub max_turns: Option<u32>,
    /// Consecutive zero-edit turns after which the loop injects a single
    /// neutral "make the edit" nudge (#239). `None` keeps the default (10);
    /// `0` disables the nudge.
    pub no_edit_nudge_threshold: Option<u32>,
    /// Maximum consecutive degenerate (output-but-no-work) turns the loop will
    /// nudge before letting the run end (#249). `None` keeps the default (2);
    /// `0` disables the guard.
    pub empty_turn_nudge_max: Option<u32>,
    /// Per-turn cap on cumulative *thinking* characters before the run
    /// terminates with `ThinkingBudgetExhausted` (#62). `None` keeps the
    /// default (262144); `0` disables the guard.
    pub max_turn_thinking_chars: Option<usize>,
    /// Wall-clock **time budget** for the whole agent loop, in seconds (ADR
    /// 0058, B2 · #662). `None`/unset and `0` both mean *no deadline* (today's
    /// behavior); any positive value ends the run with `TimeBudgetExceeded`
    /// once that many seconds of wall-clock elapse.
    pub time_budget_secs: Option<u64>,
    /// **Cost budget** for the whole agent loop, in USD (ADR 0058, B3 · #663).
    /// `None`/unset and any value `<= 0` mean *no cost cap*; a positive value
    /// ends the run with `CostBudgetExceeded` once the run's accumulated
    /// estimated cost reaches it. Enforced only when the binary has a rate card
    /// to price usage (it injects the cost model); inert otherwise.
    pub cost_budget_usd: Option<f64>,
    /// How much the system prompt encourages self-verification (ADR 0058, B5 ·
    /// #665). When set, it wins over the profile's default (explicit knob >
    /// profile > context). When unset, the effective value comes from the
    /// resolved profile (B6).
    pub verification_guidance: Option<VerificationGuidance>,
    /// Named policy profile (ADR 0058, B6 · #666). When set, it overrides the
    /// context-adaptive default (local vs cloud). When unset, the profile is
    /// chosen from execution context.
    pub profile: Option<AgentLoopProfile>,
}

// ---------------------------------------------------------------------------
// storage — memory storage substrate selection (#473)
// ---------------------------------------------------------------------------

/// Which gonzalo substrate backs memory storage. `fs` (default) is the
/// gonzalo-free local backend; `remote` targets a gonzalo daemon. `git`/`s3`
/// parse but are not wired yet (tracked in #469).
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageSubstrate {
    /// Gonzalo-free local filesystem backend (default; no feature required).
    #[default]
    Fs,
    /// A remote gonzalo daemon (requires the `gonzalo` build feature).
    Remote,
    /// Gonzalo git substrate — recognized but not wired yet (#469).
    Git,
    /// Gonzalo S3 substrate — recognized but not wired yet (#469).
    S3,
}

/// Connection settings for a remote gonzalo daemon.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteStorageConfig {
    /// Base URL of the gonzalod HTTP endpoint (e.g. `http://host:8080`).
    pub url: String,
    /// NAME of the environment variable holding the bearer token. The token
    /// itself is never stored in settings.json.
    pub token_env: Option<String>,
}

/// Memory storage substrate selection (#473). Absent ⇒ `fs` ⇒ unchanged.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    /// Which substrate backs memory storage (default `fs`).
    pub substrate: StorageSubstrate,
    /// Remote daemon connection settings; required when `substrate = remote`.
    pub remote: Option<RemoteStorageConfig>,
}

/// Environment variable that overrides `storage.substrate` (#659).
pub const ENV_STORAGE_SUBSTRATE: &str = "CALIBAN_STORAGE_SUBSTRATE";
/// Environment variable that overrides `storage.remote.url` (#659).
pub const ENV_STORAGE_REMOTE_URL: &str = "CALIBAN_STORAGE_REMOTE_URL";
/// Environment variable that overrides `storage.remote.token_env` (#659).
pub const ENV_STORAGE_REMOTE_TOKEN_ENV: &str = "CALIBAN_STORAGE_REMOTE_TOKEN_ENV";

/// Parse a substrate token using the **same lowercase vocabulary the settings
/// file accepts** (`fs`/`remote`/`git`/`s3`), by round-tripping through the
/// enum's own `Deserialize`. This keeps the env override in lockstep with the
/// file so the two can never diverge on the same concept — note it is `fs`, not
/// `local`. Returns `None` for an unknown token.
fn parse_storage_substrate(token: &str) -> Option<StorageSubstrate> {
    serde_json::from_value(serde_json::Value::String(token.trim().to_ascii_lowercase())).ok()
}

impl StorageConfig {
    /// Apply the `CALIBAN_STORAGE_*` environment overrides on top of the loaded
    /// settings — **env wins** over the settings file (#659). `lookup` resolves a
    /// variable name to its value (production passes `|k| std::env::var(k).ok()`;
    /// tests pass a map), so the logic is pure and testable without touching the
    /// process environment.
    ///
    /// Rules, matching the ticket contract:
    /// - A **blank** (empty or whitespace-only) value is ignored — the file
    ///   setting stands.
    /// - `CALIBAN_STORAGE_SUBSTRATE` accepts the same vocabulary as the file
    ///   (`fs`/`remote`/`git`/`s3`); an **invalid** value is a hard error that
    ///   names the variable, never a silent fallback.
    /// - `CALIBAN_STORAGE_REMOTE_URL` / `_TOKEN_ENV` synthesize `[storage.remote]`
    ///   when it is absent, so `substrate=remote` + a URL alone (no settings
    ///   file) yields a usable remote store.
    /// - The token itself is never read here — only `token_env`, the *name* of
    ///   the variable holding it (the indirection that keeps secrets out of
    ///   settings and out of these overrides).
    ///
    /// # Errors
    /// Returns a human-facing message naming `CALIBAN_STORAGE_SUBSTRATE` when it
    /// holds an unrecognized substrate. The struct is not mutated on error.
    pub fn apply_env_overrides(
        &mut self,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<(), String> {
        // Resolve the substrate first (fallible) before mutating anything, so an
        // invalid substrate leaves the struct untouched.
        let new_substrate = match lookup(ENV_STORAGE_SUBSTRATE) {
            Some(raw) if !raw.trim().is_empty() => {
                Some(parse_storage_substrate(&raw).ok_or_else(|| {
                    format!(
                        "{ENV_STORAGE_SUBSTRATE}: unknown storage substrate {raw:?}; \
                         expected one of fs, remote, git, s3"
                    )
                })?)
            }
            _ => None,
        };
        if let Some(s) = new_substrate {
            self.substrate = s;
        }
        if let Some(raw) = lookup(ENV_STORAGE_REMOTE_URL) {
            let url = raw.trim();
            if !url.is_empty() {
                self.remote
                    .get_or_insert_with(RemoteStorageConfig::default)
                    .url = url.to_string();
            }
        }
        if let Some(raw) = lookup(ENV_STORAGE_REMOTE_TOKEN_ENV) {
            let name = raw.trim();
            if !name.is_empty() {
                self.remote
                    .get_or_insert_with(RemoteStorageConfig::default)
                    .token_env = Some(name.to_string());
            }
        }
        Ok(())
    }
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

    // ----- sandbox ----------------------------------------------------------
    /// OS-sandbox posture for Bash commands under `--workspace` (#406).
    pub sandbox: SandboxSettings,

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
    /// Opt-in allowing HTTP hooks to target loopback/private addresses (a
    /// genuinely-local hook server). Link-local / cloud-metadata stays blocked
    /// regardless. Off by default (#217).
    pub allow_local_http_hook_targets: Option<bool>,

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

    // ----- storage ----------------------------------------------------------
    /// Memory storage substrate selection (#473).
    pub storage: StorageConfig,

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
    /// Custom statusline command. Uses Claude Code-compatible `statusLine`
    /// casing on disk; `status_line` is accepted as a TOML-friendly alias.
    #[serde(rename = "statusLine", alias = "status_line")]
    pub status_line: Option<StatuslineConfig>,
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
    /// Compaction strategy used by `/compact` and threshold-autocompact.
    /// One of `"summarize"` (default — LLM summary of older turns; preserves
    /// context but incurs a provider call), `"drop-oldest"` (LLM-free; drops
    /// the oldest turns past the recent window), or `"noop"` (disable).
    /// Unknown values fall back to the default. Without this the agent runs
    /// the builder default `NoopCompactor` and neither path reduces history
    /// (#292).
    pub compact_strategy: Option<String>,
    /// Global per-tool-result cap in characters. `0` disables.
    pub tool_result_cap_chars: Option<usize>,
    /// Minimum estimated tokens on the last user message to merit the
    /// conversation-level cache marker.
    pub min_cache_block_tokens: Option<usize>,
    /// Stage A budget escalation + Stage B meta-continuation when a turn
    /// ends in `MaxTokens` (the "max-tokens recovery" two-stage flow).
    /// Default `true` when unset; opt out with `false`. CLI flag
    /// `--max-tokens-recovery` overrides.
    pub max_tokens_recovery: Option<bool>,

    // ----- stream watchdog (#263 / #254) ------------------------------------
    /// Idle window (ms) tolerated *after* the first output chunk (mid-content
    /// stall). `None` keeps the 90s default; `0` disables the watchdog.
    pub stream_idle_timeout_ms: Option<u32>,
    /// Idle window (ms) tolerated *before* the first output chunk (slow
    /// local-model prefill, #263). `None` keeps the 300s default; `0` falls
    /// back to the idle window.
    pub stream_prefill_timeout_ms: Option<u32>,

    // ----- tool surface (ADR-0046) ------------------------------------------
    /// Lazy MCP tool loading knobs. Default off in v1.
    pub tools: Option<ToolsConfig>,

    // ----- agent-loop policy (ADR 0058, B1 · #661) --------------------------
    /// The `[agent_loop]` policy group — turn budget + spiral-containment
    /// guards, gathered under one surface. Every field is optional and
    /// defaults to today's hard-coded value (behavior-preserving).
    pub agent_loop: Option<AgentLoopConfig>,

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
        // `${VAR}` / `${VAR:-default}` / `${CLAUDE_PROJECT_DIR}` expansion is
        // applied to every string-valued MCP field so secrets (OAuth client
        // secrets, bearer tokens) can live in the environment rather than in
        // `settings.toml`. This matches the legacy `mcp.toml` loader's semantics
        // (`caliban_mcp_client::config`); `CLAUDE_PROJECT_DIR` binds to the
        // current working directory (the workspace root in normal runs).
        let mut ctx = caliban_common::expand::ExpandContext::from_process_env();
        if let Ok(cwd) = std::env::current_dir() {
            ctx.set("CLAUDE_PROJECT_DIR", cwd.to_string_lossy().into_owned());
        }

        let mut servers = std::collections::BTreeMap::new();
        for (name, s) in &self.mcp_servers {
            let transport = parse_transport(name, s.r#type.as_deref());
            let oauth = parse_oauth(name, s.oauth.as_deref());
            // Parse URL string into the typed `url::Url`. Bad input
            // surfaces as `None` here; downstream the manager will
            // refuse to start a remote transport without a valid url.
            let url = s.url.as_deref().and_then(|raw| {
                let raw = expand_mcp_field(&ctx, name, "url", raw);
                match url::Url::parse(&raw) {
                    Ok(u) => Some(u),
                    Err(e) => {
                        tracing::warn!(
                            target: caliban_common::tracing_targets::TARGET_MCP,
                            server = name.as_str(),
                            url = raw.as_str(),
                            error = %e,
                            "invalid MCP server url; ignoring",
                        );
                        None
                    }
                }
            });
            servers.insert(
                name.clone(),
                caliban_mcp_client::ServerConfig {
                    transport,
                    command: expand_mcp_field(&ctx, name, "command", &s.command),
                    args: s
                        .args
                        .iter()
                        .map(|a| expand_mcp_field(&ctx, name, "args", a))
                        .collect(),
                    // #694: the global `settings.env` (env overrides for child
                    // processes) applies to spawned MCP servers, with the
                    // per-server `env` taking precedence over it.
                    env: {
                        let mut merged: std::collections::BTreeMap<String, String> = self
                            .env
                            .iter()
                            .map(|(k, v)| (k.clone(), expand_mcp_field(&ctx, name, "env", v)))
                            .collect();
                        for (k, v) in &s.env {
                            merged.insert(k.clone(), expand_mcp_field(&ctx, name, "env", v));
                        }
                        merged
                    },
                    cwd: s.cwd.clone(),
                    url,
                    headers: s
                        .headers
                        .iter()
                        .map(|(k, v)| (k.clone(), expand_mcp_field(&ctx, name, "headers", v)))
                        .collect(),
                    oauth,
                    manual_oauth: expand_manual_oauth(
                        &ctx,
                        name,
                        s.oauth_config.clone().unwrap_or_default(),
                    ),
                    disabled: s.disabled,
                    lazy: s.lazy,
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
            allow_local_http_hook_targets: self.allow_local_http_hook_targets.unwrap_or(false),
            events: std::collections::BTreeMap::new(),
            // #694: global env overrides flow to command hooks (per-hook wins).
            global_env: self.env.clone(),
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

    /// Resolve the configured compaction strategy name, defaulting to
    /// `"summarize"` when unset. The strategy object itself is constructed at
    /// agent-build time (it needs the provider); this only resolves the name.
    #[must_use]
    pub fn compact_strategy_or_default(&self) -> &str {
        self.compact_strategy.as_deref().unwrap_or("summarize")
    }

    /// Apply stream-watchdog knobs onto a fresh
    /// [`caliban_agent_core::AgentConfig`]. Only fields explicitly set in
    /// settings override the defaults. See #263 / #254.
    pub fn apply_stream_watchdog(&self, cfg: &mut caliban_agent_core::AgentConfig) {
        if let Some(v) = self.stream_idle_timeout_ms {
            cfg.stream_idle_timeout_ms = v;
        }
        if let Some(v) = self.stream_prefill_timeout_ms {
            cfg.stream_prefill_timeout_ms = v;
        }
    }

    /// Apply the `[agent_loop]` spiral-containment guards onto a fresh
    /// [`caliban_agent_core::AgentConfig`] (ADR 0058, B1 · #661). Only fields
    /// explicitly set in settings override the defaults; everything else is
    /// left at the upstream default (see `AgentConfig::default()`), so an
    /// absent group is behavior-preserving.
    ///
    /// **`max_turns` is intentionally not applied here.** It carries a CLI flag
    /// (`--max-turns`) that must win over settings, so the caller
    /// (`startup::compose`) resolves it with the CLI > settings > default idiom
    /// via [`Self::agent_loop_max_turns`]. Folding it into this overlay would let
    /// a settings value silently override an explicit CLI flag.
    pub fn apply_agent_loop(&self, cfg: &mut caliban_agent_core::AgentConfig) {
        let Some(al) = self.agent_loop.as_ref() else {
            return;
        };
        if let Some(v) = al.no_edit_nudge_threshold {
            cfg.no_edit_nudge_threshold = v;
        }
        if let Some(v) = al.empty_turn_nudge_max {
            cfg.empty_turn_nudge_max = v;
        }
        if let Some(v) = al.max_turn_thinking_chars {
            cfg.max_turn_thinking_chars = v;
        }
        // B2 (#662): a positive value sets the wall-clock deadline; `0` means
        // "no deadline" (disabled) so a config value cannot accidentally set a
        // zero-second budget that terminates before the first turn.
        if let Some(secs) = al.time_budget_secs {
            cfg.time_budget = (secs > 0).then(|| std::time::Duration::from_secs(secs));
        }
        // B3 (#663): a positive value sets the cost cap; `<= 0` disables it so a
        // config value cannot impose a zero-dollar budget that stops every run.
        // Enforcement additionally requires an injected cost model (the binary
        // supplies one); the cap is inert without pricing.
        if let Some(usd) = al.cost_budget_usd {
            cfg.cost_budget_usd = (usd > 0.0).then_some(usd);
        }
    }

    /// The `[agent_loop] max_turns` value, if set. The caller layers CLI over
    /// this over the built-in default (CLI > settings > default); see
    /// [`Self::apply_agent_loop`] for why `max_turns` is resolved by the caller
    /// rather than overlaid.
    #[must_use]
    pub fn agent_loop_max_turns(&self) -> Option<u32> {
        self.agent_loop.as_ref().and_then(|al| al.max_turns)
    }

    /// The effective verification-guidance level (ADR 0058, B5 · #665),
    /// defaulting to [`VerificationGuidance::Off`] (no injected guidance —
    /// today's behavior) when unset. The binary reads this to shape the system
    /// prompt; it does not thread through `AgentConfig` because it affects only
    /// the prompt, not the loop.
    #[must_use]
    pub fn agent_loop_verification_guidance(&self) -> VerificationGuidance {
        self.agent_loop
            .as_ref()
            .and_then(|al| al.verification_guidance)
            .unwrap_or_default()
    }

    /// Resolve the effective agent-loop profile (ADR 0058, B6 · #666): an
    /// explicitly-named `[agent_loop] profile` wins; otherwise it is chosen from
    /// execution context — `local_execution` selects [`AgentLoopProfile::LocalGuarded`]
    /// (quality-first + divergence guard), cloud selects
    /// [`AgentLoopProfile::CostOptimized`] (cost-conservative). Cloud is the safe
    /// fallback when context is uncertain.
    #[must_use]
    pub fn resolve_agent_loop_profile(&self, local_execution: bool) -> AgentLoopProfile {
        if let Some(p) = self.agent_loop.as_ref().and_then(|al| al.profile) {
            return p;
        }
        if local_execution {
            AgentLoopProfile::LocalGuarded
        } else {
            AgentLoopProfile::CostOptimized
        }
    }

    /// The effective verification guidance with full precedence (ADR 0058, B6):
    /// an explicit `[agent_loop] verification_guidance` wins; otherwise the
    /// resolved profile's default applies. `local_execution` only matters when
    /// no explicit profile is named.
    #[must_use]
    pub fn effective_verification_guidance(&self, local_execution: bool) -> VerificationGuidance {
        if let Some(vg) = self
            .agent_loop
            .as_ref()
            .and_then(|al| al.verification_guidance)
        {
            return vg;
        }
        self.resolve_agent_loop_profile(local_execution)
            .verification_guidance()
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

/// Expand `${VAR}` references in one MCP config string. On an expansion error
/// (missing var with no default, malformed syntax) warn and return the raw
/// value unchanged — matching ADR 0026's warn-and-continue posture. The bad
/// value then surfaces downstream (e.g. a failed handshake) rather than
/// silently dropping the server.
fn expand_mcp_field(
    ctx: &caliban_common::expand::ExpandContext,
    server: &str,
    field: &str,
    raw: &str,
) -> String {
    match caliban_common::expand::expand_vars(raw, ctx) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_MCP,
                server = server,
                field = field,
                error = %e,
                "MCP config value expansion failed; using raw value",
            );
            raw.to_string()
        }
    }
}

/// Expand `${VAR}` references inside a `[mcp_servers.X.oauth_config]` block
/// (client id/secret, endpoints, audience, scopes) so credentials can live in
/// the environment.
fn expand_manual_oauth(
    ctx: &caliban_common::expand::ExpandContext,
    server: &str,
    cfg: caliban_mcp_client::ManualOauthConfig,
) -> caliban_mcp_client::ManualOauthConfig {
    let expand_opt = |field: &str, v: Option<String>| -> Option<String> {
        v.map(|raw| expand_mcp_field(ctx, server, field, &raw))
    };
    caliban_mcp_client::ManualOauthConfig {
        client_id: expand_opt("oauth_config.client_id", cfg.client_id),
        client_secret: expand_opt("oauth_config.client_secret", cfg.client_secret),
        auth_url: expand_opt("oauth_config.auth_url", cfg.auth_url),
        token_url: expand_opt("oauth_config.token_url", cfg.token_url),
        audience: expand_opt("oauth_config.audience", cfg.audience),
        scopes: cfg
            .scopes
            .into_iter()
            .map(|s| expand_mcp_field(ctx, server, "oauth_config.scopes", &s))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_network_parses_from_settings_json() {
        let s: Settings = serde_json::from_value(serde_json::json!({
            "sandbox": { "network": "allow" }
        }))
        .expect("parse");
        assert_eq!(s.sandbox.network, Some(SandboxNetwork::Allow));

        let s: Settings = serde_json::from_value(serde_json::json!({
            "sandbox": { "network": "deny" }
        }))
        .expect("parse");
        assert_eq!(s.sandbox.network, Some(SandboxNetwork::Deny));
    }

    #[test]
    fn sandbox_network_unset_is_none() {
        // `None` means "the scope didn't declare this" — the fence default
        // (deny) applies. Distinct from an explicit `"deny"`.
        let s: Settings = serde_json::from_value(serde_json::json!({})).expect("parse");
        assert_eq!(s.sandbox.network, None);
    }

    #[test]
    fn sandbox_network_rejects_unknown_value() {
        // Fail closed on a typo rather than silently falling back to a
        // permissive default (the #410 lesson).
        let r: Result<Settings, _> = serde_json::from_value(serde_json::json!({
            "sandbox": { "network": "open" }
        }));
        assert!(r.is_err(), "an unknown egress posture must not parse");
    }

    #[test]
    fn mcp_field_expands_var() {
        let mut ctx = caliban_common::expand::ExpandContext::from_process_env();
        ctx.set("GITHUB_MCP_TOKEN", "ghs_secret");
        assert_eq!(
            expand_mcp_field(&ctx, "github", "headers", "Bearer ${GITHUB_MCP_TOKEN}"),
            "Bearer ghs_secret",
        );
    }

    #[test]
    fn mcp_field_default_and_literal() {
        let ctx = caliban_common::expand::ExpandContext::from_process_env();
        // Default form when the var is unset.
        assert_eq!(
            expand_mcp_field(
                &ctx,
                "s",
                "url",
                "https://${NOPE_UNSET_VAR:-example.com}/mcp"
            ),
            "https://example.com/mcp",
        );
        // A plain literal is untouched.
        assert_eq!(
            expand_mcp_field(&ctx, "s", "url", "https://example.com/mcp"),
            "https://example.com/mcp",
        );
    }

    #[test]
    fn mcp_field_missing_var_falls_back_to_raw() {
        // Missing var with no default: warn-and-continue returns the raw string
        // (the bad value then surfaces downstream, not silently dropped).
        let ctx = caliban_common::expand::ExpandContext::from_process_env();
        let raw = "Bearer ${DEFINITELY_UNSET_TOKEN_VAR_XYZ}";
        assert_eq!(expand_mcp_field(&ctx, "s", "headers", raw), raw);
    }

    #[test]
    fn manual_oauth_expands_client_id_and_scopes() {
        let mut ctx = caliban_common::expand::ExpandContext::from_process_env();
        ctx.set("CID", "Iv1.abc");
        ctx.set("SEC", "shhh");
        let cfg = caliban_mcp_client::ManualOauthConfig {
            client_id: Some("${CID}".to_string()),
            client_secret: Some("${SEC}".to_string()),
            auth_url: None,
            token_url: None,
            audience: None,
            scopes: vec!["${CID}".to_string(), "read".to_string()],
        };
        let out = expand_manual_oauth(&ctx, "github", cfg);
        assert_eq!(out.client_id.as_deref(), Some("Iv1.abc"));
        assert_eq!(out.client_secret.as_deref(), Some("shhh"));
        assert_eq!(out.scopes, vec!["Iv1.abc".to_string(), "read".to_string()]);
    }

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
    fn tools_config_roundtrip() {
        let raw = r#"{"tools": {"lazy_mcp": true, "max_active_schemas": 32}}"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        let tools = s.tools.expect("tools should parse");
        assert_eq!(tools.lazy_mcp, Some(true));
        assert_eq!(tools.max_active_schemas, Some(32));
        // Absent skill_guidance defaults to None (= on).
        assert_eq!(tools.skill_guidance, None);
    }

    #[test]
    fn tools_config_skill_guidance_opt_out() {
        let raw = r#"{"tools": {"skill_guidance": false}}"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        let tools = s.tools.expect("tools should parse");
        assert_eq!(tools.skill_guidance, Some(false));
    }

    #[test]
    fn tools_config_absent_leaves_field_none() {
        let s: Settings = serde_json::from_str(r#"{"model": "test"}"#).unwrap();
        assert!(s.tools.is_none());
    }

    #[test]
    fn apply_context_management_overrides_each_field() {
        // Round-trip a settings.toml fragment that sets all four Plan B
        // context-management knobs to non-default values, then assert
        // apply_context_management copies each onto a fresh AgentConfig.
        // Guards the historical wiring gap (PR #60 added the Settings
        // fields + the helper but never wired the call from build_agent).
        let raw = r#"{
            "auto_compact_threshold": 0.42,
            "micro_compact_enabled": false,
            "tool_result_cap_chars": 12345,
            "min_cache_block_tokens": 789
        }"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        s.apply_context_management(&mut cfg);
        assert!((cfg.auto_compact_threshold.unwrap() - 0.42_f32).abs() < 1e-6);
        assert!(!cfg.micro_compact_enabled);
        assert_eq!(cfg.tool_result_cap_chars, 12_345);
        assert_eq!(cfg.min_cache_block_tokens, 789);
    }

    #[test]
    fn compact_strategy_defaults_to_summarize_and_honors_override() {
        let s: Settings = serde_json::from_str(r"{}").unwrap();
        assert_eq!(s.compact_strategy_or_default(), "summarize");
        let s: Settings = serde_json::from_str(r#"{"compact_strategy": "drop-oldest"}"#).unwrap();
        assert_eq!(s.compact_strategy_or_default(), "drop-oldest");
    }

    #[test]
    fn apply_context_management_leaves_defaults_when_unset() {
        // No knobs set → AgentConfig::default() values survive untouched.
        let s: Settings = serde_json::from_str(r"{}").unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        let snap_threshold = cfg.auto_compact_threshold;
        let snap_micro = cfg.micro_compact_enabled;
        let snap_cap = cfg.tool_result_cap_chars;
        let snap_min = cfg.min_cache_block_tokens;
        s.apply_context_management(&mut cfg);
        assert_eq!(cfg.auto_compact_threshold, snap_threshold);
        assert_eq!(cfg.micro_compact_enabled, snap_micro);
        assert_eq!(cfg.tool_result_cap_chars, snap_cap);
        assert_eq!(cfg.min_cache_block_tokens, snap_min);
    }

    #[test]
    fn agent_loop_config_roundtrip() {
        let raw = r#"{
            "agent_loop": {
                "max_turns": 120,
                "no_edit_nudge_threshold": 4,
                "empty_turn_nudge_max": 1,
                "max_turn_thinking_chars": 9999,
                "time_budget_secs": 600,
                "cost_budget_usd": 2.5
            }
        }"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        let al = s.agent_loop.clone().expect("agent_loop should parse");
        assert_eq!(al.max_turns, Some(120));
        assert_eq!(al.no_edit_nudge_threshold, Some(4));
        assert_eq!(al.empty_turn_nudge_max, Some(1));
        assert_eq!(al.max_turn_thinking_chars, Some(9999));
        assert_eq!(al.time_budget_secs, Some(600));
        assert_eq!(al.cost_budget_usd, Some(2.5));
        assert_eq!(s.agent_loop_max_turns(), Some(120));
    }

    #[test]
    fn verification_guidance_parses_and_defaults_to_off() {
        // Unset → Off (behavior-preserving default).
        let s: Settings = serde_json::from_str(r"{}").unwrap();
        assert_eq!(
            s.agent_loop_verification_guidance(),
            VerificationGuidance::Off
        );
        // Each kebab-case value parses.
        for (raw, want) in [
            ("off", VerificationGuidance::Off),
            ("verify-when-cheap", VerificationGuidance::VerifyWhenCheap),
            ("full", VerificationGuidance::Full),
        ] {
            let json = format!(r#"{{"agent_loop": {{"verification_guidance": "{raw}"}}}}"#);
            let s: Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(s.agent_loop_verification_guidance(), want, "value {raw}");
        }
    }

    #[test]
    fn profile_resolution_is_context_adaptive_and_overridable() {
        // No profile set → context picks it: local→LocalGuarded, cloud→CostOptimized.
        let s: Settings = serde_json::from_str(r"{}").unwrap();
        assert_eq!(
            s.resolve_agent_loop_profile(true),
            AgentLoopProfile::LocalGuarded
        );
        assert_eq!(
            s.resolve_agent_loop_profile(false),
            AgentLoopProfile::CostOptimized
        );

        // Explicit profile overrides context (both directions).
        let s: Settings =
            serde_json::from_str(r#"{"agent_loop": {"profile": "quality-first"}}"#).unwrap();
        assert_eq!(
            s.resolve_agent_loop_profile(true),
            AgentLoopProfile::QualityFirst
        );
        assert_eq!(
            s.resolve_agent_loop_profile(false),
            AgentLoopProfile::QualityFirst
        );
    }

    #[test]
    fn effective_verification_guidance_precedence_explicit_over_profile_over_context() {
        // Context default: cloud→off, local→verify-when-cheap.
        let s: Settings = serde_json::from_str(r"{}").unwrap();
        assert_eq!(
            s.effective_verification_guidance(false),
            VerificationGuidance::Off
        );
        assert_eq!(
            s.effective_verification_guidance(true),
            VerificationGuidance::VerifyWhenCheap
        );

        // Named profile beats context: quality-first → full, regardless of local.
        let s: Settings =
            serde_json::from_str(r#"{"agent_loop": {"profile": "quality-first"}}"#).unwrap();
        assert_eq!(
            s.effective_verification_guidance(false),
            VerificationGuidance::Full
        );

        // Explicit verification_guidance beats the profile.
        let s: Settings = serde_json::from_str(
            r#"{"agent_loop": {"profile": "quality-first", "verification_guidance": "off"}}"#,
        )
        .unwrap();
        assert_eq!(
            s.effective_verification_guidance(true),
            VerificationGuidance::Off,
            "explicit knob must win over the profile"
        );
    }

    #[test]
    fn agent_loop_profile_maps_to_verification_guidance() {
        assert_eq!(
            AgentLoopProfile::CostOptimized.verification_guidance(),
            VerificationGuidance::Off
        );
        assert_eq!(
            AgentLoopProfile::QualityFirst.verification_guidance(),
            VerificationGuidance::Full
        );
        assert_eq!(
            AgentLoopProfile::LocalGuarded.verification_guidance(),
            VerificationGuidance::VerifyWhenCheap
        );
    }

    #[test]
    fn apply_agent_loop_cost_budget_positive_sets_and_nonpositive_disables() {
        let s: Settings =
            serde_json::from_str(r#"{"agent_loop": {"cost_budget_usd": 1.25}}"#).unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        assert_eq!(cfg.cost_budget_usd, None, "default is no cost cap");
        s.apply_agent_loop(&mut cfg);
        assert_eq!(cfg.cost_budget_usd, Some(1.25));

        // `0` (or negative) disables — never a zero-dollar budget.
        let s: Settings =
            serde_json::from_str(r#"{"agent_loop": {"cost_budget_usd": 0}}"#).unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        s.apply_agent_loop(&mut cfg);
        assert_eq!(cfg.cost_budget_usd, None);
    }

    #[test]
    fn apply_agent_loop_time_budget_positive_sets_and_zero_disables() {
        // A positive value becomes a Duration deadline.
        let s: Settings =
            serde_json::from_str(r#"{"agent_loop": {"time_budget_secs": 300}}"#).unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        assert_eq!(cfg.time_budget, None, "default is no deadline");
        s.apply_agent_loop(&mut cfg);
        assert_eq!(cfg.time_budget, Some(std::time::Duration::from_mins(5)));

        // `0` explicitly disables (maps to None), never a zero-second budget.
        let s: Settings =
            serde_json::from_str(r#"{"agent_loop": {"time_budget_secs": 0}}"#).unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        s.apply_agent_loop(&mut cfg);
        assert_eq!(cfg.time_budget, None);
    }

    #[test]
    fn agent_loop_absent_leaves_field_none() {
        let s: Settings = serde_json::from_str(r#"{"model": "test"}"#).unwrap();
        assert!(s.agent_loop.is_none());
        assert_eq!(s.agent_loop_max_turns(), None);
    }

    #[test]
    fn apply_agent_loop_overrides_each_guard() {
        // Every guard set to a non-default value is copied onto a fresh
        // AgentConfig. max_turns is deliberately NOT applied by this overlay
        // (CLI precedence — see apply_agent_loop docs), so it stays default.
        let raw = r#"{
            "agent_loop": {
                "max_turns": 7,
                "no_edit_nudge_threshold": 4,
                "empty_turn_nudge_max": 1,
                "max_turn_thinking_chars": 9999
            }
        }"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        let default_max_turns = cfg.max_turns;
        s.apply_agent_loop(&mut cfg);
        assert_eq!(cfg.no_edit_nudge_threshold, 4);
        assert_eq!(cfg.empty_turn_nudge_max, 1);
        assert_eq!(cfg.max_turn_thinking_chars, 9999);
        // max_turns is resolved by the caller, not this overlay.
        assert_eq!(cfg.max_turns, default_max_turns);
    }

    #[test]
    fn apply_agent_loop_leaves_defaults_when_unset() {
        // No agent_loop group → AgentConfig::default() guards survive untouched.
        let s: Settings = serde_json::from_str(r"{}").unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        let snap_no_edit = cfg.no_edit_nudge_threshold;
        let snap_empty = cfg.empty_turn_nudge_max;
        let snap_thinking = cfg.max_turn_thinking_chars;
        s.apply_agent_loop(&mut cfg);
        assert_eq!(cfg.no_edit_nudge_threshold, snap_no_edit);
        assert_eq!(cfg.empty_turn_nudge_max, snap_empty);
        assert_eq!(cfg.max_turn_thinking_chars, snap_thinking);
    }

    #[test]
    fn apply_agent_loop_partial_leaves_unset_guards_at_default() {
        // Only one guard set; the others keep their AgentConfig defaults.
        let raw = r#"{"agent_loop": {"no_edit_nudge_threshold": 0}}"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        let snap_empty = cfg.empty_turn_nudge_max;
        let snap_thinking = cfg.max_turn_thinking_chars;
        s.apply_agent_loop(&mut cfg);
        assert_eq!(cfg.no_edit_nudge_threshold, 0);
        assert_eq!(cfg.empty_turn_nudge_max, snap_empty);
        assert_eq!(cfg.max_turn_thinking_chars, snap_thinking);
    }

    #[test]
    fn agent_loop_rejects_unknown_key() {
        // deny_unknown_fields catches a typo'd knob rather than silently
        // dropping it.
        let raw = r#"{"agent_loop": {"no_edit_nudge": 4}}"#;
        assert!(serde_json::from_str::<Settings>(raw).is_err());
    }

    #[test]
    fn apply_stream_watchdog_overrides_each_field() {
        let raw = r#"{
            "stream_idle_timeout_ms": 45000,
            "stream_prefill_timeout_ms": 600000
        }"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        s.apply_stream_watchdog(&mut cfg);
        assert_eq!(cfg.stream_idle_timeout_ms, 45_000);
        assert_eq!(cfg.stream_prefill_timeout_ms, 600_000);
    }

    #[test]
    fn apply_stream_watchdog_leaves_defaults_when_unset() {
        let s: Settings = serde_json::from_str(r"{}").unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        let snap_idle = cfg.stream_idle_timeout_ms;
        let snap_prefill = cfg.stream_prefill_timeout_ms;
        s.apply_stream_watchdog(&mut cfg);
        assert_eq!(cfg.stream_idle_timeout_ms, snap_idle);
        assert_eq!(cfg.stream_prefill_timeout_ms, snap_prefill);
    }

    #[test]
    fn max_tokens_recovery_roundtrip() {
        let s: Settings = serde_json::from_str(r#"{"max_tokens_recovery": true}"#).unwrap();
        assert_eq!(s.max_tokens_recovery, Some(true));
        let s: Settings = serde_json::from_str(r#"{"max_tokens_recovery": false}"#).unwrap();
        assert_eq!(s.max_tokens_recovery, Some(false));
        let s: Settings = serde_json::from_str(r"{}").unwrap();
        assert!(s.max_tokens_recovery.is_none());
    }

    #[test]
    fn tools_config_partial_fields_default_to_none_inside() {
        let raw = r#"{"tools": {"lazy_mcp": true}}"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        let tools = s.tools.expect("tools should parse");
        assert_eq!(tools.lazy_mcp, Some(true));
        assert_eq!(tools.max_active_schemas, None);
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

    #[test]
    fn mcp_config_merges_global_env_under_per_server_env() {
        // #694: settings.env applies to MCP servers; per-server env wins.
        let mut srv = BTreeMap::new();
        let mut server_env = BTreeMap::new();
        server_env.insert("SHARED".to_string(), "from-server".to_string());
        srv.insert(
            "s".to_string(),
            McpServerSetting {
                command: "x".into(),
                env: server_env,
                ..Default::default()
            },
        );
        let mut global = BTreeMap::new();
        global.insert("GLOBAL_ONLY".to_string(), "g".to_string());
        global.insert("SHARED".to_string(), "from-global".to_string());
        let s = Settings {
            env: global,
            mcp_servers: srv,
            ..Default::default()
        };
        let env = &s.mcp_config().servers["s"].env;
        assert_eq!(env.get("GLOBAL_ONLY").map(String::as_str), Some("g"));
        assert_eq!(
            env.get("SHARED").map(String::as_str),
            Some("from-server"),
            "per-server env must override settings.env"
        );
    }

    #[test]
    fn hook_config_carries_global_env_from_settings_env() {
        // #694: settings.env flows to command hooks via HooksConfig.global_env.
        let mut global = BTreeMap::new();
        global.insert("K".to_string(), "v".to_string());
        let s = Settings {
            env: global,
            ..Default::default()
        };
        assert_eq!(
            s.hook_config().global_env.get("K").map(String::as_str),
            Some("v")
        );
        // Absent settings.env → empty global_env (behavior-preserving).
        assert!(Settings::default().hook_config().global_env.is_empty());
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

    #[cfg(test)]
    mod storage_config_tests {
        use super::{
            ENV_STORAGE_REMOTE_TOKEN_ENV, ENV_STORAGE_REMOTE_URL, ENV_STORAGE_SUBSTRATE,
            RemoteStorageConfig, Settings, StorageConfig, StorageSubstrate,
        };
        use std::collections::HashMap;

        /// Build an injected env lookup from `(name, value)` pairs.
        fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
            let m: HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            move |k: &str| m.get(k).cloned()
        }

        #[test]
        fn absent_storage_defaults_to_fs() {
            let s: Settings = serde_json::from_str("{}").unwrap();
            assert_eq!(s.storage.substrate, StorageSubstrate::Fs);
            assert!(s.storage.remote.is_none());
        }

        #[test]
        fn parses_remote_with_url_and_token_env() {
            let s: Settings = serde_json::from_str(
                r#"{ "storage": { "substrate": "remote", "remote": { "url": "http://h:8080", "token_env": "GONZALO_TOKEN" } } }"#,
            )
            .unwrap();
            assert_eq!(s.storage.substrate, StorageSubstrate::Remote);
            let r = s.storage.remote.unwrap();
            assert_eq!(r.url, "http://h:8080");
            assert_eq!(r.token_env.as_deref(), Some("GONZALO_TOKEN"));
        }

        #[test]
        fn unknown_storage_key_is_rejected() {
            let e = serde_json::from_str::<Settings>(
                r#"{ "storage": { "substrate": "fs", "nope": 1 } }"#,
            );
            assert!(e.is_err(), "deny_unknown_fields should reject 'nope'");
        }

        #[test]
        fn unknown_substrate_value_is_rejected() {
            let e = serde_json::from_str::<Settings>(r#"{ "storage": { "substrate": "sqlite" } }"#);
            assert!(e.is_err(), "unknown substrate variant should fail");
        }

        // ----- #659 env overrides (env wins over the loaded file) -----

        #[test]
        fn env_substrate_overrides_file_setting() {
            let mut s = StorageConfig::default(); // fs
            s.apply_env_overrides(env(&[(ENV_STORAGE_SUBSTRATE, "remote")]))
                .unwrap();
            assert_eq!(s.substrate, StorageSubstrate::Remote);
        }

        #[test]
        fn env_url_and_token_env_synthesize_remote_block() {
            let mut s = StorageConfig::default();
            s.apply_env_overrides(env(&[
                (ENV_STORAGE_SUBSTRATE, "remote"),
                (ENV_STORAGE_REMOTE_URL, "http://gonzalod:8080"),
                (ENV_STORAGE_REMOTE_TOKEN_ENV, "GONZALO_TOKEN"),
            ]))
            .unwrap();
            assert_eq!(s.substrate, StorageSubstrate::Remote);
            let r = s.remote.expect("remote block synthesized from env");
            assert_eq!(r.url, "http://gonzalod:8080");
            assert_eq!(r.token_env.as_deref(), Some("GONZALO_TOKEN"));
        }

        #[test]
        fn remote_from_env_alone_with_no_file_yields_remote_store() {
            // No settings file ⇒ default config (fs, no remote). substrate + url
            // from env alone must produce a usable remote store.
            let mut s = StorageConfig::default();
            s.apply_env_overrides(env(&[
                (ENV_STORAGE_SUBSTRATE, "remote"),
                (ENV_STORAGE_REMOTE_URL, "http://h:8080"),
            ]))
            .unwrap();
            assert_eq!(s.substrate, StorageSubstrate::Remote);
            assert_eq!(s.remote.expect("remote synthesized").url, "http://h:8080");
        }

        #[test]
        fn blank_env_values_are_ignored_file_stands() {
            let mut s = StorageConfig {
                substrate: StorageSubstrate::Remote,
                remote: Some(RemoteStorageConfig {
                    url: "http://from-file:9".into(),
                    token_env: Some("FILE_TOKEN".into()),
                }),
            };
            s.apply_env_overrides(env(&[
                (ENV_STORAGE_SUBSTRATE, "   "),
                (ENV_STORAGE_REMOTE_URL, ""),
                (ENV_STORAGE_REMOTE_TOKEN_ENV, ""),
            ]))
            .unwrap();
            assert_eq!(s.substrate, StorageSubstrate::Remote);
            let r = s.remote.unwrap();
            assert_eq!(r.url, "http://from-file:9");
            assert_eq!(r.token_env.as_deref(), Some("FILE_TOKEN"));
        }

        #[test]
        fn invalid_substrate_errors_naming_the_var_and_does_not_mutate() {
            let mut s = StorageConfig::default();
            let err = s
                .apply_env_overrides(env(&[(ENV_STORAGE_SUBSTRATE, "sqlite")]))
                .unwrap_err();
            assert!(
                err.contains("CALIBAN_STORAGE_SUBSTRATE"),
                "error must name the variable: {err}"
            );
            assert!(
                err.contains("sqlite"),
                "error must echo the bad value: {err}"
            );
            assert_eq!(
                s.substrate,
                StorageSubstrate::Fs,
                "must not mutate on error"
            );
        }

        #[test]
        fn env_vocabulary_matches_the_file_fs_not_local() {
            // `local` is NOT accepted — the env var uses the file's vocabulary.
            let mut s = StorageConfig::default();
            assert!(
                s.apply_env_overrides(env(&[(ENV_STORAGE_SUBSTRATE, "local")]))
                    .is_err(),
                "`local` is not a valid substrate; the file uses `fs`"
            );
            // `fs` (with surrounding whitespace) is accepted and normalizes.
            let mut s2 = StorageConfig {
                substrate: StorageSubstrate::Remote,
                remote: None,
            };
            s2.apply_env_overrides(env(&[(ENV_STORAGE_SUBSTRATE, " FS ")]))
                .unwrap();
            assert_eq!(s2.substrate, StorageSubstrate::Fs);
        }

        #[test]
        fn absent_env_leaves_settings_untouched() {
            let mut s = StorageConfig {
                substrate: StorageSubstrate::Remote,
                remote: Some(RemoteStorageConfig {
                    url: "http://f:1".into(),
                    token_env: None,
                }),
            };
            s.apply_env_overrides(|_| None).unwrap();
            assert_eq!(s.substrate, StorageSubstrate::Remote);
            assert_eq!(s.remote.unwrap().url, "http://f:1");
        }
    }

    #[test]
    fn schema_accepts_remote_storage_block() {
        let doc = serde_json::json!({ "storage": { "substrate": "remote", "remote": { "url": "http://h:8080", "token_env": "T" } } });
        let errors = crate::schema::validate_value(&doc);
        assert!(
            errors.is_empty(),
            "schema rejected valid storage: {errors:?}"
        );
    }
}
