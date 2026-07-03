//! `mcp.toml` config schema + discovery + merge.
//!
//! Phase B extends the v1 stdio-only schema with HTTP and SSE transports plus
//! per-server permission blocks. See `docs/superpowers/specs/2026-05-24-mcp-v2-design.md`
//! and `docs/adr/0023-mcp-v2-transports-and-oauth.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::McpError;
use crate::oauth::ManualOauthConfig;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Raw (pre-validation) form for serde
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct RawServerConfig {
    #[serde(default)]
    transport: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    oauth: Option<String>,
    #[serde(default)]
    disabled: bool,
    /// Per-server lazy override (ADR-0046). See `ServerConfig::lazy`.
    #[serde(default)]
    lazy: Option<bool>,
    #[serde(default)]
    permissions: ServerPermissions,
    /// `[server.X.oauth_config]` table block. Required when `oauth = "manual"`.
    /// (Spec calls it `[server.X.oauth]`, but `oauth = "..."` as a string
    /// already occupies that key; we sidestep the TOML conflict by spelling
    /// the table key `oauth_config`.)
    #[serde(default)]
    oauth_config: Option<ManualOauthConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct ServersFile {
    #[serde(default)]
    server: BTreeMap<String, RawServerConfig>,
}

// ---------------------------------------------------------------------------
// Public helpers
// ---------------------------------------------------------------------------

/// Validate a server name against `^[a-z0-9_-]{1,32}$`.
#[must_use]
pub fn is_valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Resolve the standard discovery paths for `mcp.toml`.
///
/// Returns `(user_candidates, project_path)`. The `user_candidates` vec is
/// an ordered, deduplicated list of user-scope candidates:
///
/// 1. `$XDG_CONFIG_HOME/caliban/mcp.toml`, else `~/.config/caliban/mcp.toml`
///    (the XDG path — XDG-first on every OS per ADR 0050).
/// 2. `platform_config_dir().join("caliban/mcp.toml")` (the base config path).
///    Under ADR 0050 this now resolves to the same `~/.config/caliban` on every
///    platform, so it is virtually always deduped against (1); it is retained
///    only as a defensive second candidate.
///
/// `load_config` consults these in order with **first-found wins** semantics at
/// the user tier. Since both resolve to `~/.config/caliban`, `mcp.toml` and the
/// unified `settings.toml` now live in the same directory — the split that let
/// a `[mcp_servers]` block in `settings.toml` be silently ignored on macOS is
/// gone.
#[must_use]
pub fn discovery_paths(workspace_root: &Path) -> (Vec<PathBuf>, PathBuf) {
    let mut user: Vec<PathBuf> = Vec::new();
    // 1. XDG path: "$XDG_CONFIG_HOME ? : $HOME/.config", + caliban/mcp.toml.
    let xdg = caliban_common::paths::xdg_config_home("caliban").join("mcp.toml");
    user.push(xdg);
    // 2. Base config path. XDG-first (ADR 0050) makes this identical to (1) on
    //    every OS; dedupe keeps a single candidate.
    if let Some(native) =
        caliban_common::paths::platform_config_dir().map(|d| d.join("caliban").join("mcp.toml"))
        && !user.iter().any(|p| p == &native)
    {
        user.push(native);
    }
    let project = workspace_root.join(".caliban").join("mcp.toml");
    (user, project)
}

/// Load and merge MCP config from the user file and the project file.
///
/// Either file may be missing — both missing is a no-op (`Ok(empty config)`).
/// At the user tier, the first existing path in [`discovery_paths`]'s
/// `user_candidates` list wins (XDG first, platform-native as fallback).
/// Project entries replace user entries with the same name wholesale.
///
/// `${VAR}` / `${VAR:-default}` / `${CLAUDE_PROJECT_DIR}` expansion is applied
/// to `command`, `args`, `env.*`, `cwd`, `url`, and `headers.*`.
///
/// # Errors
/// Returns [`McpError::ConfigParse`] if a file exists but is malformed,
/// [`McpError::InvalidServerName`] if a server key violates the naming rule,
/// or one of the transport-specific validation variants
/// ([`McpError::InvalidUrl`], [`McpError::MissingUrl`], etc.).
#[deprecated(
    since = "0.0.1",
    note = "load via caliban-settings; legacy loaders remove in v0.2"
)]
pub fn load_config(workspace_root: &Path) -> Result<McpConfig, McpError> {
    let (user_candidates, project) = discovery_paths(workspace_root);
    let mut merged: BTreeMap<String, ServerConfig> = BTreeMap::new();
    // First existing user-scope path wins.
    for candidate in &user_candidates {
        if candidate.exists() {
            merge_from(&mut merged, candidate, workspace_root)?;
            tracing::debug!(
                target: caliban_common::tracing_targets::TARGET_MCP,
                path = %candidate.display(),
                "loaded user-scope mcp.toml",
            );
            break;
        }
    }
    merge_from(&mut merged, &project, workspace_root)?;
    Ok(McpConfig { servers: merged })
}

fn merge_from(
    into: &mut BTreeMap<String, ServerConfig>,
    path: &Path,
    workspace_root: &Path,
) -> Result<(), McpError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(McpError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
    };
    let parsed: ServersFile = toml::from_str(&raw).map_err(|source| McpError::ConfigParse {
        path: path.to_path_buf(),
        source,
    })?;
    for (name, raw_cfg) in parsed.server {
        if !is_valid_server_name(&name) {
            return Err(McpError::InvalidServerName(name));
        }
        let cfg = normalize(&name, raw_cfg, workspace_root)?;
        into.insert(name, cfg);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Validation + env-var expansion
// ---------------------------------------------------------------------------

fn normalize(
    server: &str,
    raw: RawServerConfig,
    workspace_root: &Path,
) -> Result<ServerConfig, McpError> {
    let transport = parse_transport(server, raw.transport.as_deref())?;
    let oauth = parse_oauth(server, raw.oauth.as_deref())?;
    // stdio + oauth makes no sense — oauth is per-HTTP-call.
    if matches!(transport, TransportKind::Stdio) && !matches!(oauth, OauthMode::Off) {
        return Err(McpError::StdioFieldMismatch {
            server: server.to_string(),
            field: "oauth",
        });
    }
    match transport {
        TransportKind::Stdio => normalize_stdio(server, transport, oauth, raw, workspace_root),
        TransportKind::Http | TransportKind::Sse => {
            normalize_remote(server, transport, oauth, raw, workspace_root)
        }
    }
}

fn parse_transport(server: &str, raw: Option<&str>) -> Result<TransportKind, McpError> {
    match raw {
        None | Some("stdio") => Ok(TransportKind::Stdio),
        Some("http") => Ok(TransportKind::Http),
        Some("sse") => Ok(TransportKind::Sse),
        Some(other) => Err(McpError::InvalidTransport {
            server: server.to_string(),
            value: other.to_string(),
        }),
    }
}

fn parse_oauth(server: &str, raw: Option<&str>) -> Result<OauthMode, McpError> {
    match raw {
        None | Some("off") => Ok(OauthMode::Off),
        Some("auto") => Ok(OauthMode::Auto),
        Some("manual") => Ok(OauthMode::Manual),
        Some(other) => Err(McpError::InvalidOauthMode {
            server: server.to_string(),
            value: other.to_string(),
        }),
    }
}

fn normalize_stdio(
    server: &str,
    transport: TransportKind,
    oauth: OauthMode,
    raw: RawServerConfig,
    workspace_root: &Path,
) -> Result<ServerConfig, McpError> {
    if raw.url.is_some() {
        return Err(McpError::StdioFieldMismatch {
            server: server.to_string(),
            field: "url",
        });
    }
    if !raw.headers.is_empty() {
        return Err(McpError::StdioFieldMismatch {
            server: server.to_string(),
            field: "headers",
        });
    }
    let command = raw.command.unwrap_or_default();
    if command.is_empty() {
        return Err(McpError::StdioFieldMismatch {
            server: server.to_string(),
            field: "command",
        });
    }
    let command = expand_value(server, "command", &command, workspace_root)?;
    let args = raw
        .args
        .iter()
        .enumerate()
        .map(|(i, a)| expand_value(server, &format!("args[{i}]"), a, workspace_root))
        .collect::<Result<Vec<_>, _>>()?;
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in &raw.env {
        env.insert(
            k.clone(),
            expand_value(server, &format!("env[{k}]"), v, workspace_root)?,
        );
    }
    Ok(ServerConfig {
        transport,
        command,
        args,
        env,
        cwd: raw.cwd,
        url: None,
        headers: BTreeMap::new(),
        oauth,
        manual_oauth: ManualOauthConfig::default(),
        disabled: raw.disabled,
        lazy: raw.lazy,
        permissions: raw.permissions,
    })
}

fn normalize_remote(
    server: &str,
    transport: TransportKind,
    oauth: OauthMode,
    raw: RawServerConfig,
    workspace_root: &Path,
) -> Result<ServerConfig, McpError> {
    // Reject stdio-only fields so the config is unambiguous.
    if raw.command.is_some() {
        return Err(McpError::StdioFieldMismatch {
            server: server.to_string(),
            field: "command",
        });
    }
    if !raw.args.is_empty() {
        return Err(McpError::StdioFieldMismatch {
            server: server.to_string(),
            field: "args",
        });
    }
    if !raw.env.is_empty() {
        return Err(McpError::StdioFieldMismatch {
            server: server.to_string(),
            field: "env",
        });
    }
    if raw.cwd.is_some() {
        return Err(McpError::StdioFieldMismatch {
            server: server.to_string(),
            field: "cwd",
        });
    }
    let url_raw = raw.url.as_deref().ok_or_else(|| McpError::MissingUrl {
        server: server.to_string(),
        transport: transport.as_str(),
    })?;
    let url_expanded = expand_value(server, "url", url_raw, workspace_root)?;
    let parsed = Url::parse(&url_expanded).map_err(|e| McpError::InvalidUrl {
        server: server.to_string(),
        url: url_expanded.clone(),
        reason: e.to_string(),
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(McpError::InvalidUrl {
            server: server.to_string(),
            url: url_expanded,
            reason: format!("scheme must be http or https, got '{}'", parsed.scheme()),
        });
    }
    if !parsed.has_host() {
        return Err(McpError::InvalidUrl {
            server: server.to_string(),
            url: url_expanded,
            reason: "missing host".to_string(),
        });
    }
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in &raw.headers {
        let v = expand_value(server, &format!("headers[{k}]"), v, workspace_root)?;
        headers.insert(k.clone(), v);
    }
    // Manual OAuth block — required when `oauth = "manual"`. We expand any
    // `${VAR}` references in its string fields so operators can keep secrets
    // out of the file.
    let manual_oauth = match (oauth, raw.oauth_config.as_ref()) {
        (OauthMode::Manual, None) => {
            return Err(McpError::OauthManualIncomplete {
                server: server.to_string(),
                field: "oauth_config (block)",
            });
        }
        (_, None) => ManualOauthConfig::default(),
        (_, Some(cfg)) => expand_manual_oauth(server, cfg, workspace_root)?,
    };
    Ok(ServerConfig {
        transport,
        command: String::new(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        url: Some(parsed),
        headers,
        oauth,
        manual_oauth,
        disabled: raw.disabled,
        lazy: raw.lazy,
        permissions: raw.permissions,
    })
}

fn expand_manual_oauth(
    server: &str,
    cfg: &ManualOauthConfig,
    workspace_root: &Path,
) -> Result<ManualOauthConfig, McpError> {
    let expand_opt = |name: &str, raw: Option<&String>| -> Result<Option<String>, McpError> {
        match raw {
            None => Ok(None),
            Some(v) => Ok(Some(expand_value(
                server,
                &format!("oauth_config.{name}"),
                v,
                workspace_root,
            )?)),
        }
    };
    Ok(ManualOauthConfig {
        client_id: expand_opt("client_id", cfg.client_id.as_ref())?,
        client_secret: expand_opt("client_secret", cfg.client_secret.as_ref())?,
        auth_url: expand_opt("auth_url", cfg.auth_url.as_ref())?,
        token_url: expand_opt("token_url", cfg.token_url.as_ref())?,
        scopes: cfg.scopes.clone(),
        audience: expand_opt("audience", cfg.audience.as_ref())?,
    })
}

// ---------------------------------------------------------------------------
// Env-var expansion (delegates to `caliban_common::expand`)
// ---------------------------------------------------------------------------

/// Expand `${VAR}`, `${VAR:-default}`, and `${CLAUDE_PROJECT_DIR}` references
/// inside `raw`. Supports inline expansion (multiple variables per value).
///
/// `CLAUDE_PROJECT_DIR` is bound to `workspace_root`'s string form — operators
/// don't need to set the env var themselves for it to expand.
fn expand_value(
    server: &str,
    field: &str,
    raw: &str,
    workspace_root: &Path,
) -> Result<String, McpError> {
    let mut ctx = caliban_common::expand::ExpandContext::from_process_env();
    ctx.set(
        "CLAUDE_PROJECT_DIR",
        workspace_root.to_string_lossy().into_owned(),
    );
    caliban_common::expand::expand_vars(raw, &ctx).map_err(|e| match e {
        caliban_common::expand::ExpandError::MissingVar { name } => McpError::MissingEnvField {
            server: server.to_string(),
            field: field.to_string(),
            var: name,
        },
        // Unclosed brace and invalid syntax both surface as missing-var with
        // a synthetic name — kept under one McpError variant to avoid
        // churning the error enum in this PR.
        caliban_common::expand::ExpandError::UnclosedBrace { .. }
        | caliban_common::expand::ExpandError::InvalidSyntax { .. } => McpError::MissingEnvField {
            server: server.to_string(),
            field: field.to_string(),
            var: "<expansion-error>".to_string(),
        },
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    fn parse_one(body: &str) -> ServersFile {
        toml::from_str(body).expect("parse")
    }

    #[test]
    fn parses_minimal_stdio_server() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = "[server.s1]\ncommand = \"echo\"\n";
        let raw = parse_one(body);
        let cfg = normalize("s1", raw.server.into_values().next().unwrap(), tmp.path()).unwrap();
        assert_eq!(cfg.transport, TransportKind::Stdio);
        assert_eq!(cfg.command, "echo");
        assert!(cfg.args.is_empty());
        assert!(cfg.url.is_none());
    }

    #[test]
    fn server_config_lazy_parses() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = "[server.s1]\ncommand = \"echo\"\nlazy = false\n";
        let raw = parse_one(body);
        let cfg = normalize("s1", raw.server.into_values().next().unwrap(), tmp.path()).unwrap();
        assert_eq!(cfg.lazy, Some(false));
    }

    #[test]
    fn server_config_lazy_absent_is_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = "[server.s1]\ncommand = \"echo\"\n";
        let raw = parse_one(body);
        let cfg = normalize("s1", raw.server.into_values().next().unwrap(), tmp.path()).unwrap();
        assert_eq!(cfg.lazy, None);
    }

    #[test]
    fn server_config_lazy_true_parses() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = "[server.s1]\ncommand = \"echo\"\nlazy = true\n";
        let raw = parse_one(body);
        let cfg = normalize("s1", raw.server.into_values().next().unwrap(), tmp.path()).unwrap();
        assert_eq!(cfg.lazy, Some(true));
    }

    #[test]
    fn parses_http_server() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.notion]
transport = "http"
url = "https://example.com/mcp"
headers = { X-Workspace = "demo" }
"#;
        let raw = parse_one(body);
        let cfg = normalize(
            "notion",
            raw.server.into_values().next().unwrap(),
            tmp.path(),
        )
        .unwrap();
        assert_eq!(cfg.transport, TransportKind::Http);
        assert_eq!(cfg.url.unwrap().to_string(), "https://example.com/mcp");
        assert_eq!(cfg.headers.get("X-Workspace"), Some(&"demo".to_string()));
    }

    #[test]
    fn http_requires_url() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = "[server.bad]\ntransport = \"http\"\n";
        let raw = parse_one(body);
        let err =
            normalize("bad", raw.server.into_values().next().unwrap(), tmp.path()).unwrap_err();
        assert!(matches!(err, McpError::MissingUrl { .. }), "got: {err:?}");
    }

    #[test]
    fn http_rejects_non_absolute_url() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.bad]
transport = "http"
url = "not-a-url"
"#;
        let raw = parse_one(body);
        let err =
            normalize("bad", raw.server.into_values().next().unwrap(), tmp.path()).unwrap_err();
        assert!(matches!(err, McpError::InvalidUrl { .. }), "got: {err:?}");
    }

    #[test]
    fn http_rejects_non_http_scheme() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.bad]
transport = "http"
url = "ftp://example.com/mcp"
"#;
        let raw = parse_one(body);
        let err =
            normalize("bad", raw.server.into_values().next().unwrap(), tmp.path()).unwrap_err();
        let McpError::InvalidUrl { reason, .. } = err else {
            panic!("expected InvalidUrl");
        };
        assert!(reason.contains("scheme"), "reason: {reason}");
    }

    #[test]
    fn stdio_rejects_url() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.bad]
command = "echo"
url = "https://example.com"
"#;
        let raw = parse_one(body);
        let err =
            normalize("bad", raw.server.into_values().next().unwrap(), tmp.path()).unwrap_err();
        assert!(
            matches!(err, McpError::StdioFieldMismatch { field: "url", .. }),
            "got: {err:?}",
        );
    }

    #[test]
    fn http_rejects_command() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.bad]
transport = "http"
url = "https://example.com"
command = "echo"
"#;
        let raw = parse_one(body);
        let err =
            normalize("bad", raw.server.into_values().next().unwrap(), tmp.path()).unwrap_err();
        assert!(
            matches!(
                err,
                McpError::StdioFieldMismatch {
                    field: "command",
                    ..
                }
            ),
            "got: {err:?}",
        );
    }

    #[test]
    fn oauth_auto_accepted_in_phase_c() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.s]
transport = "http"
url = "https://example.com"
oauth = "auto"
"#;
        let raw = parse_one(body);
        let cfg = normalize("s", raw.server.into_values().next().unwrap(), tmp.path()).unwrap();
        assert_eq!(cfg.oauth, OauthMode::Auto);
    }

    #[test]
    fn oauth_manual_requires_config_block() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.bad]
transport = "http"
url = "https://example.com"
oauth = "manual"
"#;
        let raw = parse_one(body);
        let err =
            normalize("bad", raw.server.into_values().next().unwrap(), tmp.path()).unwrap_err();
        assert!(
            matches!(err, McpError::OauthManualIncomplete { .. }),
            "got: {err:?}",
        );
    }

    #[test]
    fn oauth_manual_with_config_block_parses() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.ok]
transport = "http"
url = "https://example.com"
oauth = "manual"

[server.ok.oauth_config]
client_id = "my-client"
auth_url = "https://auth.example.com/authorize"
token_url = "https://auth.example.com/token"
scopes = ["read", "write"]
"#;
        let raw = parse_one(body);
        let cfg = normalize("ok", raw.server.into_values().next().unwrap(), tmp.path()).unwrap();
        assert_eq!(cfg.oauth, OauthMode::Manual);
        assert_eq!(cfg.manual_oauth.client_id.as_deref(), Some("my-client"));
        assert_eq!(
            cfg.manual_oauth.scopes,
            vec!["read".to_string(), "write".to_string()],
        );
    }

    #[test]
    fn oauth_stdio_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.bad]
command = "echo"
oauth = "auto"
"#;
        let raw = parse_one(body);
        let err =
            normalize("bad", raw.server.into_values().next().unwrap(), tmp.path()).unwrap_err();
        assert!(
            matches!(err, McpError::StdioFieldMismatch { field: "oauth", .. }),
            "got: {err:?}",
        );
    }

    #[test]
    fn oauth_off_is_ok() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.ok]
transport = "http"
url = "https://example.com"
oauth = "off"
"#;
        let raw = parse_one(body);
        let cfg = normalize("ok", raw.server.into_values().next().unwrap(), tmp.path()).unwrap();
        assert_eq!(cfg.oauth, OauthMode::Off);
    }

    #[test]
    fn oauth_garbage_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.bad]
transport = "http"
url = "https://example.com"
oauth = "wat"
"#;
        let raw = parse_one(body);
        let err =
            normalize("bad", raw.server.into_values().next().unwrap(), tmp.path()).unwrap_err();
        assert!(
            matches!(err, McpError::InvalidOauthMode { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn transport_garbage_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.bad]
transport = "carrier-pigeon"
"#;
        let raw = parse_one(body);
        let err =
            normalize("bad", raw.server.into_values().next().unwrap(), tmp.path()).unwrap_err();
        assert!(
            matches!(err, McpError::InvalidTransport { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn env_expansion_url_with_project_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let body = r#"
[server.s]
transport = "http"
url = "https://example.com${CLAUDE_PROJECT_DIR}/mcp"
"#;
        let raw = parse_one(body);
        let cfg = normalize("s", raw.server.into_values().next().unwrap(), &workspace).unwrap();
        // URL crate percent-encodes the path — assert the host + that the
        // workspace path appears (we don't care about the exact encoding).
        let s = cfg.url.unwrap().to_string();
        assert!(s.contains("example.com"), "url: {s}");
        assert!(s.contains("ws"), "url: {s}");
    }

    #[test]
    fn env_expansion_with_default() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.s]
transport = "http"
url = "https://${MCP_HOST_THAT_DOES_NOT_EXIST:-example.com}/mcp"
"#;
        let raw = parse_one(body);
        let cfg = normalize("s", raw.server.into_values().next().unwrap(), tmp.path()).unwrap();
        assert_eq!(cfg.url.unwrap().to_string(), "https://example.com/mcp");
    }

    #[test]
    fn env_expansion_missing_var_with_no_default_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.s]
transport = "http"
url = "https://${MCP_HOST_THAT_DOES_NOT_EXIST}/mcp"
"#;
        let raw = parse_one(body);
        let err = normalize("s", raw.server.into_values().next().unwrap(), tmp.path()).unwrap_err();
        assert!(
            matches!(err, McpError::MissingEnvField { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn permissions_block_parses() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = r#"
[server.linear]
transport = "http"
url = "https://linear.app/mcp"

[server.linear.permissions]
allow = ["read_*"]
deny  = ["delete_*"]
ask   = ["create_*"]
"#;
        let raw = parse_one(body);
        let cfg = normalize(
            "linear",
            raw.server.into_values().next().unwrap(),
            tmp.path(),
        )
        .unwrap();
        assert_eq!(cfg.permissions.allow, vec!["read_*"]);
        assert_eq!(cfg.permissions.deny, vec!["delete_*"]);
        assert_eq!(cfg.permissions.ask, vec!["create_*"]);
    }

    #[test]
    fn valid_name_rule() {
        assert!(is_valid_server_name("linear"));
        assert!(is_valid_server_name("ls-9_x"));
        assert!(!is_valid_server_name(""));
        assert!(!is_valid_server_name("UPPER"));
        assert!(!is_valid_server_name("with space"));
        assert!(!is_valid_server_name(&"x".repeat(33)));
    }

    #[test]
    fn project_overrides_user_wholesale() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let user = tmp.path().join("user/caliban/mcp.toml");
        write(
            &user,
            "[server.linear]\ncommand = \"user-cmd\"\nargs = [\"old\"]\n",
        );
        write(
            &workspace.join(".caliban/mcp.toml"),
            "[server.linear]\ncommand = \"project-cmd\"\n",
        );
        let mut merged: BTreeMap<String, ServerConfig> = BTreeMap::new();
        super::merge_from(&mut merged, &user, &workspace).unwrap();
        super::merge_from(
            &mut merged,
            &workspace.join(".caliban/mcp.toml"),
            &workspace,
        )
        .unwrap();
        assert_eq!(merged["linear"].command, "project-cmd");
        assert!(merged["linear"].args.is_empty(), "wholly replaced");
    }

    #[test]
    fn disabled_field_round_trip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = "[server.s1]\ncommand = \"x\"\ndisabled = true\n";
        let raw = parse_one(body);
        let cfg = normalize("s1", raw.server.into_values().next().unwrap(), tmp.path()).unwrap();
        assert!(cfg.disabled);
    }

    // -----------------------------------------------------------------
    // discovery_paths — XDG-first / platform-native fallback / dedupe
    // -----------------------------------------------------------------
    //
    // These tests mutate `XDG_CONFIG_HOME`, so they serialize behind a
    // process-wide mutex. `std::env::set_var` was marked `unsafe` in
    // Rust 2024 (racy across threads); we follow the same RAII guard
    // pattern as `caliban_common::paths::tests`.

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[allow(unsafe_code)]
    fn set_env(key: &str, value: Option<&str>) {
        match value {
            // SAFETY: serialized via ENV_LOCK in EnvGuard below.
            Some(v) => unsafe { std::env::set_var(key, v) },
            // SAFETY: serialized via ENV_LOCK in EnvGuard below.
            None => unsafe { std::env::remove_var(key) },
        }
    }

    struct EnvGuard {
        key: String,
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(key: &str, val: Option<&str>) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let prev = std::env::var(key).ok();
            set_env(key, val);
            Self {
                key: key.into(),
                prev,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            set_env(&self.key, self.prev.as_deref());
        }
    }

    #[test]
    fn discovery_paths_returns_at_least_one_user_candidate() {
        // We don't care about the specific path content here — just that
        // `discovery_paths` is contract-stable: always >= 1 user candidate
        // and always returns the project path with the expected suffix.
        let tmp = tempfile::TempDir::new().unwrap();
        let (user, project) = discovery_paths(tmp.path());
        assert!(!user.is_empty(), "expected at least one user candidate");
        assert!(project.ends_with(".caliban/mcp.toml"));
    }

    #[test]
    fn discovery_paths_xdg_override_honored() {
        // When XDG_CONFIG_HOME is set, the first user candidate must use it.
        let tmp = tempfile::TempDir::new().unwrap();
        let xdg_root = tmp.path().join("xdg-config");
        let _g = EnvGuard::set("XDG_CONFIG_HOME", xdg_root.to_str());
        let (user, _project) = discovery_paths(tmp.path());
        assert!(
            user[0].starts_with(&xdg_root),
            "first user candidate should be under XDG_CONFIG_HOME, got {:?}",
            user[0]
        );
        assert!(user[0].ends_with("caliban/mcp.toml"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn discovery_paths_linux_dedupes_when_xdg_equals_native() {
        // On Linux with XDG_CONFIG_HOME unset, dirs::config_dir() and the
        // XDG fallback both resolve to $HOME/.config/caliban — dedupe.
        let _g = EnvGuard::set("XDG_CONFIG_HOME", None);
        let (user, _project) = discovery_paths(std::path::Path::new("/tmp/ws"));
        assert_eq!(
            user.len(),
            1,
            "expected dedupe on Linux when XDG defaults to $HOME/.config; got {user:?}",
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn discovery_paths_macos_dedupes_to_xdg_config() {
        // XDG-first (ADR 0050): on macOS the base config dir is now
        // $HOME/.config (not ~/Library/Application Support), so the XDG
        // candidate and the platform-config candidate are identical and
        // dedupe to a single ~/.config/caliban/mcp.toml — same as Linux.
        let _g = EnvGuard::set("XDG_CONFIG_HOME", None);
        let (user, _project) = discovery_paths(std::path::Path::new("/tmp/ws"));
        assert_eq!(
            user.len(),
            1,
            "expected dedupe to a single XDG path on macOS (ADR 0050); got {user:?}",
        );
        assert!(
            user[0].to_string_lossy().contains(".config/caliban"),
            "expected ~/.config/caliban, got {:?}",
            user[0],
        );
        assert!(
            !user[0].to_string_lossy().contains("Application Support"),
            "must not use Application Support (ADR 0050), got {:?}",
            user[0],
        );
    }

    #[test]
    #[allow(deprecated)]
    fn load_config_prefers_xdg_when_both_user_paths_exist() {
        // First-found wins at the user tier. We can't easily construct a
        // realistic "two user paths" world via dirs::config_dir, but we
        // can verify the loop logic by setting XDG_CONFIG_HOME to a
        // populated tempdir and observing the result is taken from it.
        let tmp = tempfile::TempDir::new().unwrap();
        let xdg_root = tmp.path().join("xdg");
        let xdg_mcp = xdg_root.join("caliban").join("mcp.toml");
        write(&xdg_mcp, "[server.fromxdg]\ncommand = \"x\"\n");
        let _g = EnvGuard::set("XDG_CONFIG_HOME", xdg_root.to_str());
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let cfg = load_config(&ws).unwrap();
        assert!(
            cfg.servers.contains_key("fromxdg"),
            "expected XDG entry; got {:?}",
            cfg.servers.keys().collect::<Vec<_>>(),
        );
    }
}
