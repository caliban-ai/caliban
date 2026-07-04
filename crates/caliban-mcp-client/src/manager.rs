//! `McpClientManager` — owns child-process MCP server connections and
//! registers their advertised tools into the agent's `ToolRegistry`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use caliban_agent_core::mcp_activation::McpToolInfo;
use caliban_agent_core::{Tool, ToolRegistry};

use crate::client::{Conn, Transport};
use crate::config::{McpConfig, OauthMode, ServerConfig};
use crate::error::McpError;
use crate::oauth::{MemoryStore, OauthAuthenticator, TokenStore, default_store};
use crate::registry::{ServerStatus, ServerSummary};
use crate::tool::McpTool;

/// Default handshake timeout — overridable via `CALIBAN_MCP_TIMEOUT`
/// or the Claude-Code-compat `MCP_TIMEOUT`.
pub const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
/// Default per-tool-call timeout — overridable via `CALIBAN_MCP_TOOL_TIMEOUT`
/// or the Claude-Code-compat `MCP_TOOL_TIMEOUT`.
pub const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_mins(1);

/// Knobs for [`McpClientManager::start_with_options`]. Tests use this to drive
/// determinism without touching process-wide env vars (which are `unsafe` in
/// edition 2024).
#[derive(Debug, Clone)]
pub struct StartOptions {
    /// Handshake timeout. Overrides the `CALIBAN_MCP_TIMEOUT` env-var path.
    pub startup_timeout: Duration,
    /// Per-tool-call timeout. Overrides `CALIBAN_MCP_TOOL_TIMEOUT`.
    pub tool_timeout: Duration,
    /// Whether this run may perform interactive OAuth authorization (open a
    /// browser + block on the loopback callback). `true` for the TUI, `false`
    /// for headless/`--print`/non-TTY runs — a cold token cache then fails a
    /// server with an actionable error rather than hanging.
    pub interactive: bool,
    /// Token store for OAuth (`auto`/`manual` servers). Production uses the
    /// keyring→file store; tests inject a [`MemoryStore`] for isolation.
    pub token_store: Arc<dyn TokenStore>,
    /// HTTP client for OAuth discovery / token exchange / refresh.
    pub http: reqwest::Client,
    /// Fixed loopback OAuth callback port (`--mcp-oauth-port` /
    /// `CALIBAN_MCP_OAUTH_PORT`). `None` → ephemeral. Required for auth servers
    /// that pin the `redirect_uri` (GitHub OAuth Apps).
    pub oauth_callback_port: Option<u16>,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
            // Defaults are the hermetic/test profile: no browser, in-memory
            // token store. Production paths (`start`/`start_interactive`) build
            // `StartOptions` explicitly with the real store + interactivity.
            interactive: false,
            token_store: Arc::new(MemoryStore::default()),
            http: reqwest::Client::new(),
            oauth_callback_port: None,
        }
    }
}

/// Resolve a duration from `CALIBAN_<name>` (preferred) or `<fallback>` (compat).
/// Values are parsed as integer seconds. Unparseable or unset → `default`.
fn duration_from_env(primary: &str, fallback: &str, default: Duration) -> Duration {
    let read = |k: &str| {
        std::env::var(k)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs)
    };
    read(primary).or_else(|| read(fallback)).unwrap_or(default)
}

/// Owns all MCP server connections + a snapshot of each server's status for
/// the `/mcp` overlay. Constructed once during caliban startup; consumed by
/// [`Self::register_all`] (or the legacy [`Self::register_into`]) to attach
/// discovered tools to the parent `ToolRegistry`.
#[derive(Debug, Default)]
pub struct McpClientManager {
    /// Per-server connection (only `Connected` servers are present).
    conns: BTreeMap<String, Arc<Conn>>,
    /// All configured servers + their lifecycle state (Disabled / Connected /
    /// Failed). Kept in TOML-key order for stable display.
    summaries: Vec<ServerSummary>,
    /// `McpTool` instances awaiting registration in [`Self::register_all`].
    pending: Vec<Arc<McpTool>>,
}

impl McpClientManager {
    /// Spawn enabled servers, drive `initialize`, list each server's tools,
    /// and prepare them for registration. **Never aborts on a single server's
    /// failure** — each failure is logged via `tracing::warn!` and surfaces in
    /// the `/mcp` overlay.
    ///
    /// # Errors
    /// Phase A never returns an error from this entry point — startup is
    /// best-effort. The `Result` is preserved for forward compatibility with
    /// Phases B and C which may surface aggregated config errors.
    pub async fn start(cfg: &McpConfig) -> Result<Self, McpError> {
        Self::start_interactive(cfg, false, None).await
    }

    /// Like [`Self::start`], but declares whether the current run may perform
    /// interactive OAuth authorization. The TUI passes `true` (a browser may
    /// open for `oauth = auto|manual` servers on a cold token cache); headless
    /// / `--print` / non-TTY runs pass `false` so a cold cache fails fast with
    /// an actionable error instead of hanging on a callback that can't
    /// complete. Timeouts still come from the env-var path. `oauth_callback_port`
    /// pins the loopback OAuth callback port (`--mcp-oauth-port` /
    /// `CALIBAN_MCP_OAUTH_PORT`); `None` keeps it ephemeral.
    ///
    /// # Errors
    /// See [`Self::start`].
    pub async fn start_interactive(
        cfg: &McpConfig,
        interactive: bool,
        oauth_callback_port: Option<u16>,
    ) -> Result<Self, McpError> {
        let opts = StartOptions {
            startup_timeout: duration_from_env(
                "CALIBAN_MCP_TIMEOUT",
                "MCP_TIMEOUT",
                DEFAULT_STARTUP_TIMEOUT,
            ),
            tool_timeout: duration_from_env(
                "CALIBAN_MCP_TOOL_TIMEOUT",
                "MCP_TOOL_TIMEOUT",
                DEFAULT_TOOL_TIMEOUT,
            ),
            interactive,
            token_store: default_store(),
            http: reqwest::Client::new(),
            oauth_callback_port,
        };
        Self::start_with_options(cfg, opts).await
    }

    /// Like [`Self::start`] but uses an explicit [`StartOptions`] instead of
    /// reading from env vars. Used by tests for determinism.
    ///
    /// # Errors
    /// See [`Self::start`] — never returns an error in Phase A; reserved for
    /// Phase B/C config validation.
    pub async fn start_with_options(cfg: &McpConfig, opts: StartOptions) -> Result<Self, McpError> {
        // One authenticator (reqwest client + token store) shared across every
        // server in this pass. Only consulted for `oauth = auto|manual`.
        let authenticator = OauthAuthenticator::new(
            opts.http.clone(),
            Arc::clone(&opts.token_store),
            opts.interactive,
            opts.oauth_callback_port,
        );

        let mut mgr = Self::default();
        for (name, server) in &cfg.servers {
            mgr.connect_one(
                name,
                server,
                &authenticator,
                opts.startup_timeout,
                opts.tool_timeout,
            )
            .await;
        }
        Ok(mgr)
    }

    /// Bring up a single configured server: skip if disabled, build+authorize
    /// the transport, run the handshake, and register its tools. Every failure
    /// path records a `Failed` summary and returns — one server never aborts
    /// the pass.
    async fn connect_one(
        &mut self,
        name: &str,
        server: &ServerConfig,
        authenticator: &OauthAuthenticator,
        startup_timeout: Duration,
        tool_timeout: Duration,
    ) {
        let transport_label = server.transport.as_str();
        if server.disabled {
            self.summaries.push(ServerSummary {
                name: name.to_string(),
                status: ServerStatus::Disabled,
                transport: transport_label,
            });
            return;
        }

        let mut transport = match Transport::from_config(name, server) {
            Ok(t) => t,
            Err(e) => {
                self.record_failed(
                    name,
                    transport_label,
                    &e,
                    "mcp server config invalid; skipping",
                );
                return;
            }
        };

        // OAuth (ADR 0023 Phase C): for `auto`/`manual` servers, resolve a
        // Bearer token (reuse cached / silent refresh / interactive flow) and
        // attach it before the handshake. A failure here means the handshake
        // would just bounce with `AuthRequired`, so mark the server Failed with
        // the (actionable) auth error and skip it.
        if let Err(e) = Self::attach_oauth(authenticator, name, server, &mut transport).await {
            self.record_failed(
                name,
                transport_label,
                &e,
                "mcp server oauth authorization failed; skipping",
            );
            return;
        }

        match Conn::start(name.to_string(), transport, startup_timeout).await {
            Ok(conn) => {
                self.register_connected(name, transport_label, Arc::new(conn), tool_timeout)
                    .await;
            }
            Err(e) => {
                self.record_failed(
                    name,
                    transport_label,
                    &e,
                    "mcp server failed to start; skipping",
                );
            }
        }
    }

    /// List a connected server's tools and register them, or record a `Failed`
    /// summary (and drop the connection) if the listing fails.
    async fn register_connected(
        &mut self,
        name: &str,
        transport_label: &'static str,
        conn: Arc<Conn>,
        tool_timeout: Duration,
    ) {
        // `list_tools(None)` fetches the first page; we don't paginate (most
        // servers return their full list in one response).
        let list_result = conn.peer().list_tools(None).await;
        match list_result {
            Ok(listing) => {
                let advertised = listing.tools;
                let count = advertised.len();
                for adv in &advertised {
                    let mcp_tool = McpTool::new(name, Arc::clone(&conn), adv, tool_timeout);
                    self.pending.push(Arc::new(mcp_tool));
                }
                self.conns.insert(name.to_string(), Arc::clone(&conn));
                self.summaries.push(ServerSummary {
                    name: name.to_string(),
                    status: ServerStatus::Connected { tools: count },
                    transport: transport_label,
                });
                tracing::info!(
                    target: caliban_common::tracing_targets::TARGET_MCP,
                    server = %name,
                    transport = transport_label,
                    tools = count,
                    "mcp server connected",
                );
            }
            Err(e) => {
                let reason = format!("list_tools failed: {e}");
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_MCP,
                    server = %name,
                    error = %e,
                    "mcp server list_tools failed; skipping",
                );
                self.summaries.push(ServerSummary {
                    name: name.to_string(),
                    status: ServerStatus::Failed { reason },
                    transport: transport_label,
                });
                // Drop conn so the child shuts down.
                drop(conn);
            }
        }
    }

    /// Push a `Failed` summary for `name` and log `msg` at warn level.
    fn record_failed(
        &mut self,
        name: &str,
        transport_label: &'static str,
        error: &McpError,
        msg: &str,
    ) {
        tracing::warn!(
            target: caliban_common::tracing_targets::TARGET_MCP,
            server = %name,
            error = %error,
            "{msg}",
        );
        self.summaries.push(ServerSummary {
            name: name.to_string(),
            status: ServerStatus::Failed {
                reason: error.to_string(),
            },
            transport: transport_label,
        });
    }

    /// Resolve and attach an OAuth Bearer token to `transport` for one server.
    /// A no-op for `oauth = "off"`. Requires a URL (guaranteed for http/sse,
    /// the only transports that accept oauth — config validation rejects
    /// `stdio + oauth`).
    async fn attach_oauth(
        authenticator: &OauthAuthenticator,
        name: &str,
        server: &ServerConfig,
        transport: &mut Transport,
    ) -> Result<(), McpError> {
        if matches!(server.oauth, OauthMode::Off) {
            return Ok(());
        }
        let url = server.url.as_ref().ok_or_else(|| McpError::MissingUrl {
            server: name.to_string(),
            transport: server.transport.as_str(),
        })?;
        if let Some(token) = authenticator
            .bearer_for(name, server.oauth, url, &server.manual_oauth)
            .await?
        {
            transport.set_bearer(name, &token)?;
        }
        Ok(())
    }

    /// Number of `Connected` servers.
    #[must_use]
    pub fn enabled_count(&self) -> usize {
        self.summaries
            .iter()
            .filter(|s| matches!(s.status, ServerStatus::Connected { .. }))
            .count()
    }

    /// Number of `disabled = true` servers.
    #[must_use]
    pub fn skipped_disabled(&self) -> usize {
        self.summaries
            .iter()
            .filter(|s| s.status == ServerStatus::Disabled)
            .count()
    }

    /// Number of servers that failed to start.
    #[must_use]
    pub fn failed_count(&self) -> usize {
        self.summaries
            .iter()
            .filter(|s| matches!(s.status, ServerStatus::Failed { .. }))
            .count()
    }

    /// Per-server status, in TOML-key order, for the `/mcp` overlay.
    #[must_use]
    pub fn summaries(&self) -> &[ServerSummary] {
        &self.summaries
    }

    /// Tool names that would be registered by [`Self::register_all`], for
    /// diagnostics + tests.
    pub fn tool_names(&self) -> impl Iterator<Item = &str> {
        self.pending.iter().map(|t| t.full_name())
    }

    /// Snapshot every registered MCP tool as a [`McpToolInfo`] vector
    /// for consumption by `ToolSearch` (ADR-0046). The result is a
    /// point-in-time copy; resources stay owned by the manager. Tools
    /// from servers that failed handshake are not included.
    #[must_use]
    pub fn list_mcp_tools(&self) -> Vec<McpToolInfo> {
        self.pending
            .iter()
            .map(|t| McpToolInfo {
                full_name: t.full_name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema().clone(),
            })
            .collect()
    }

    /// Register every discovered MCP tool with `registry`.
    pub fn register_all(&self, registry: &mut ToolRegistry) {
        for t in &self.pending {
            registry.register(Arc::clone(t) as Arc<dyn caliban_agent_core::Tool>);
        }
    }

    /// Legacy alias for [`Self::register_all`] — kept so existing call sites
    /// in `caliban/src/main.rs` keep compiling while we transition.
    pub fn register_into(&self, registry: &mut ToolRegistry) {
        self.register_all(registry);
    }

    /// Gracefully shut down all live connections. Drops the rmcp services in
    /// the same order they were started.
    pub async fn shutdown(self) {
        for (name, conn) in self.conns {
            // We were the only Arc holder in tests; in main.rs the McpTool
            // instances may still hold an Arc<Conn>. Either way, dropping the
            // service triggers its DropGuard which cancels the inner loop and
            // kills the child via TokioChildProcess's Drop impl.
            tracing::debug!(target: caliban_common::tracing_targets::TARGET_MCP, server = %name, "shutting down");
            // Try to fall through to explicit cancel if we hold the only Arc.
            if let Ok(conn) = Arc::try_unwrap(conn) {
                let _ = conn.service.cancel().await;
            }
            // Otherwise: dropping our reference is enough; the McpTool Arcs
            // will fall away with the registry.
        }
    }
}
