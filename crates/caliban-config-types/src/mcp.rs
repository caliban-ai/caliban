//! MCP server configuration data types (`mcp.toml` schema, resolved form).
//!
//! These are the plain, validated shapes shared between `caliban-settings`
//! (which embeds and projects them) and `caliban-mcp-client` (which loads and
//! connects them). The pre-validation serde form (`RawServerConfig`) and the
//! loader/validation logic stay in `caliban-mcp-client::config`; only the
//! public data types live here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use url::Url;

/// Transport kind selector. Defaults to `Stdio` to keep v1 configs working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransportKind {
    /// Spawn a child process and speak JSON-RPC over its stdio.
    #[default]
    Stdio,
    /// Connect over rmcp's streamable-http client (POST + chunked + SSE).
    Http,
    /// Legacy "SSE-only" servers; routed through the same rmcp streamable-http
    /// client transport (rmcp 1.7 folded the standalone SSE client into the
    /// streamable-http worker — see the spec note in
    /// `docs/superpowers/specs/2026-05-24-mcp-v2-design.md`).
    Sse,
}

impl TransportKind {
    /// Stringly-typed name for diagnostics and the `/mcp` overlay.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }
}

/// OAuth mode. Phase B only accepts `Off`; `Auto` and `Manual` are reserved
/// for Phase C and rejected at config-parse time with a clear error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OauthMode {
    /// No OAuth — direct connection (possibly with static `Authorization` header
    /// supplied via the `headers` table).
    #[default]
    Off,
    /// Discover via `/.well-known/oauth-protected-resource` (Phase C).
    Auto,
    /// Use the manually-configured `[server.X.oauth]` block (Phase C).
    Manual,
}

impl OauthMode {
    /// Stringly-typed label for diagnostics + the `/mcp` overlay.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

/// Per-server permission rules — globs scoped to this server's tools. Glob
/// syntax matches the existing permissions engine (`*`, `?`); each entry is
/// compared against the *unprefixed* tool name. The mcp client transforms
/// these into full `mcp__<server>__<tool>` patterns when handing them to the
/// global permissions engine.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ServerPermissions {
    /// Patterns to allow without prompting.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Patterns to deny.
    #[serde(default)]
    pub deny: Vec<String>,
    /// Patterns to ask about interactively.
    #[serde(default)]
    pub ask: Vec<String>,
}

/// Manually-configured OAuth endpoints (per-server `[server.X.oauth]`
/// block). Used in `oauth = "manual"` mode; `auto` discovers these from
/// the server's well-known documents.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ManualOauthConfig {
    /// Client identifier registered with the auth server.
    #[serde(default)]
    pub client_id: Option<String>,
    /// Client secret (optional — PKCE flows are typically public).
    #[serde(default)]
    pub client_secret: Option<String>,
    /// Authorization endpoint URL.
    #[serde(default)]
    pub auth_url: Option<String>,
    /// Token endpoint URL.
    #[serde(default)]
    pub token_url: Option<String>,
    /// Scopes to request (space-joined when sent).
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Optional explicit `audience` claim (RFC 8707).
    #[serde(default)]
    pub audience: Option<String>,
}

/// One MCP server entry as written in `mcp.toml`. The field set spans all
/// three transports; validation enforces the right subset per `transport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Transport selector (defaults to `stdio` for v1 compatibility).
    pub transport: TransportKind,
    // ---- stdio ----
    /// Executable for `transport = "stdio"`. Empty when not stdio.
    pub command: String,
    /// CLI arguments forwarded verbatim (stdio only).
    pub args: Vec<String>,
    /// Environment variables (stdio only). Values support `${VAR}` /
    /// `${VAR:-default}` / `${CLAUDE_PROJECT_DIR}` expansion.
    pub env: BTreeMap<String, String>,
    /// Working directory (stdio only). Relative paths resolve against
    /// caliban's cwd; `None` inherits.
    pub cwd: Option<PathBuf>,
    // ---- http / sse ----
    /// Absolute http/https URL for http/sse transports. `None` for stdio.
    pub url: Option<Url>,
    /// Static request headers (http/sse only). Values support env expansion.
    pub headers: BTreeMap<String, String>,
    /// OAuth mode (`off`/`auto`/`manual`). Phase C wires `auto` and `manual`.
    pub oauth: OauthMode,
    /// Manual OAuth config (`[server.X.oauth]` block) — only used when
    /// `oauth = "manual"`.
    pub manual_oauth: ManualOauthConfig,
    // ---- common ----
    /// Skip this server entirely.
    pub disabled: bool,
    /// Per-server lazy override (ADR-0046). When `tools.lazy_mcp` is
    /// true globally, individual servers can opt back to eager loading
    /// by setting `lazy = false`. `None` follows the global default.
    pub lazy: Option<bool>,
    /// Per-server permission scoping (composes with global rules).
    pub permissions: ServerPermissions,
}

/// The merged, parsed MCP config.
#[derive(Debug, Default)]
pub struct McpConfig {
    /// Map of server name → resolved config (with `${VAR}` expanded and
    /// transport-specific validation applied).
    pub servers: BTreeMap<String, ServerConfig>,
}
