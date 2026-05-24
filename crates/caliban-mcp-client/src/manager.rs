//! `McpClientManager` — owns child-process MCP server connections and
//! registers their advertised tools into the agent's `ToolRegistry`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use caliban_agent_core::ToolRegistry;

use crate::client::{Conn, Transport};
use crate::config::McpConfig;
use crate::error::McpError;
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
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
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
        let startup_timeout = opts.startup_timeout;
        let tool_timeout = opts.tool_timeout;

        let mut mgr = Self::default();

        for (name, server) in &cfg.servers {
            if server.disabled {
                mgr.summaries.push(ServerSummary {
                    name: name.clone(),
                    status: ServerStatus::Disabled,
                });
                continue;
            }

            let transport = Transport::from_config(server);
            match Conn::start(name.clone(), transport, startup_timeout).await {
                Ok(conn) => {
                    let conn = Arc::new(conn);
                    // Pull the advertised tool list. `list_tools(None)` fetches
                    // the first page; for Phase A we don't paginate (most
                    // stdio servers return their full list in one response).
                    let list_result = conn.peer().list_tools(None).await;
                    match list_result {
                        Ok(listing) => {
                            let advertised = listing.tools;
                            let count = advertised.len();
                            for adv in &advertised {
                                let mcp_tool =
                                    McpTool::new(name, Arc::clone(&conn), adv, tool_timeout);
                                mgr.pending.push(Arc::new(mcp_tool));
                            }
                            mgr.conns.insert(name.clone(), Arc::clone(&conn));
                            mgr.summaries.push(ServerSummary {
                                name: name.clone(),
                                status: ServerStatus::Connected { tools: count },
                            });
                            tracing::info!(
                                target: "caliban::mcp",
                                server = %name,
                                tools = count,
                                "mcp server connected",
                            );
                        }
                        Err(e) => {
                            let reason = format!("list_tools failed: {e}");
                            tracing::warn!(
                                target: "caliban::mcp",
                                server = %name,
                                error = %e,
                                "mcp server list_tools failed; skipping",
                            );
                            mgr.summaries.push(ServerSummary {
                                name: name.clone(),
                                status: ServerStatus::Failed { reason },
                            });
                            // Drop conn so the child shuts down.
                            drop(conn);
                        }
                    }
                }
                Err(e) => {
                    let reason = e.to_string();
                    tracing::warn!(
                        target: "caliban::mcp",
                        server = %name,
                        error = %e,
                        "mcp server failed to start; skipping",
                    );
                    mgr.summaries.push(ServerSummary {
                        name: name.clone(),
                        status: ServerStatus::Failed { reason },
                    });
                }
            }
        }

        Ok(mgr)
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
            tracing::debug!(target: "caliban::mcp", server = %name, "shutting down");
            // Try to fall through to explicit cancel if we hold the only Arc.
            if let Ok(conn) = Arc::try_unwrap(conn) {
                let _ = conn.service.cancel().await;
            }
            // Otherwise: dropping our reference is enough; the McpTool Arcs
            // will fall away with the registry.
        }
    }
}
