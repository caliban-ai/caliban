//! Snapshot of server status, surfaced via `McpClientManager`.

/// Lifecycle state of one configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStatus {
    /// Server marked `disabled = true` in `mcp.toml`; not started.
    Disabled,
    /// Server connected; `n` tools registered.
    Connected {
        /// Number of tools advertised by the server and registered.
        tools: usize,
    },
    /// Server failed to start. `reason` summarizes the `McpError`.
    Failed {
        /// Human-readable error reason.
        reason: String,
    },
}

/// One entry in the `/mcp` overlay.
#[derive(Debug, Clone)]
pub struct ServerSummary {
    /// Server name as written in `mcp.toml`.
    pub name: String,
    /// Lifecycle state.
    pub status: ServerStatus,
}
