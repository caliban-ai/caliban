//! `McpClientManager` — owns child-process MCP server connections.
//!
//! v1 implements the config layer + a no-op manager that registers zero tools
//! (operators can edit `mcp.toml`, but no spawn happens yet). The rmcp-based
//! spawn + tool-discovery wiring will land as a follow-up PR.

use caliban_agent_core::ToolRegistry;

use crate::config::McpConfig;
use crate::error::Result;

/// Owns all MCP server connections (none in v1). Constructed once during
/// caliban startup; consumed by [`Self::register_into`] to attach any
/// discovered tools to the parent `ToolRegistry`.
#[derive(Debug, Default)]
pub struct McpClientManager {
    enabled_count: usize,
    skipped_disabled: usize,
}

impl McpClientManager {
    /// Build a manager from a parsed config.
    ///
    /// v1 does not actually spawn child processes; rmcp wiring is deferred.
    /// The returned manager records counts for the `/mcp` overlay.
    ///
    /// # Errors
    /// Currently never errors (no spawn attempted in v1). The signature
    /// returns `Result` so the v2 rmcp wiring can return errors without
    /// breaking callers.
    pub fn start(cfg: &McpConfig) -> Result<Self> {
        let enabled_count = cfg.servers.values().filter(|s| !s.disabled).count();
        let skipped_disabled = cfg.servers.values().filter(|s| s.disabled).count();
        if enabled_count > 0 {
            tracing::warn!(
                target: "caliban::mcp",
                enabled = enabled_count,
                "mcp.toml has {enabled_count} enabled server(s); spawn + tool wiring is deferred to a follow-up PR",
            );
        }
        Ok(Self {
            enabled_count,
            skipped_disabled,
        })
    }

    /// Number of enabled servers in the loaded config.
    #[must_use]
    pub fn enabled_count(&self) -> usize {
        self.enabled_count
    }

    /// Number of servers marked `disabled = true`.
    #[must_use]
    pub fn skipped_disabled(&self) -> usize {
        self.skipped_disabled
    }

    /// Append discovered MCP tools to `registry`. v1 is a no-op; v2 will
    /// register one `McpTool` per `(server, tool)` pair.
    pub fn register_into(&self, _registry: &mut ToolRegistry) {
        // No-op in v1.
    }

    /// Shut down all MCP connections (no-op in v1).
    #[allow(
        clippy::unused_async,
        reason = "v2 wires async rmcp shutdown; keep async signature stable"
    )]
    pub async fn shutdown(self) {
        // No-op in v1.
    }
}
