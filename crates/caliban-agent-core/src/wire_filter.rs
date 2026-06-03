//! Per-turn wire-payload filter for lazy MCP tool loading (ADR-0046).
//!
//! [`WireFilter`] is the configuration consumed by
//! [`crate::registry::ToolRegistry::to_caliban_tools_filtered`] at the
//! per-turn request-build site. The result is the tool list that
//! actually rides the wire plus a count of MCP tools that were
//! dropped — used by the stream layer to splice a deferred-block
//! paragraph into the system message.
//!
//! Filter rules per registry entry:
//!
//! 1. Non-MCP tool (name does not start with `mcp__`) → include.
//! 2. `lazy_mcp == false` → include all.
//! 3. MCP tool whose server segment is in `eager_servers` → include.
//! 4. MCP tool whose name is in `active.iter_active()` → include.
//! 5. Otherwise → drop; bump `dropped_mcp_count`.

use std::collections::HashSet;

use crate::mcp_activation::McpActivationSet;

const MCP_PREFIX: &str = "mcp__";

/// Inputs to the per-turn wire filter. Borrowed; cheap to construct.
pub struct WireFilter<'a> {
    /// Global `tools.lazy_mcp` setting. When `false`, the filter is
    /// effectively a passthrough — all MCP tools ride the wire.
    pub lazy_mcp: bool,
    /// The session's current MCP activation set. Borrowed from a
    /// `Guard` returned by `Arc<ArcSwap<…>>::load`.
    pub active: &'a McpActivationSet,
    /// Names of MCP servers that opt out of lazy loading via per-server
    /// `lazy = false`. Tools belonging to these servers always ride
    /// the wire payload even when `lazy_mcp` is true.
    pub eager_servers: &'a HashSet<String>,
}

/// Output of
/// [`crate::registry::ToolRegistry::to_caliban_tools_filtered`].
pub struct WireFilterResult {
    /// The filtered tool list ready to hand to a [`caliban_provider::CompletionRequest`].
    pub tools: Vec<caliban_provider::Tool>,
    /// Count of MCP tools dropped by the filter. Used by the stream
    /// layer to decide whether to splice the deferred-block
    /// paragraph into the system message.
    pub dropped_mcp_count: usize,
}

/// Whether `name` follows the MCP `mcp__<server>__<tool>` convention
/// (ADR-0017).
#[must_use]
pub fn is_mcp(name: &str) -> bool {
    name.starts_with(MCP_PREFIX)
}

/// Extract the `<server>` segment from `mcp__<server>__<tool>`. Returns
/// `None` if `name` is not an MCP tool or its shape is malformed.
#[must_use]
pub fn mcp_server_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(MCP_PREFIX)?;
    let end = rest.find("__")?;
    Some(&rest[..end])
}
