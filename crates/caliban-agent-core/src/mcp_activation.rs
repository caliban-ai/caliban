//! Sidecar activation state for lazy MCP tool loading (ADR-0046).
//!
//! [`McpActivationSet`] tracks which MCP tools are currently active for
//! the session. Activation is sticky-per-session; the set is
//! soft-capped, with LRU eviction once the cap is exceeded.
//!
//! Agent holds `Arc<ArcSwap<McpActivationSet>>` so reads (every turn
//! during request build) are cheap, and writes (when `ToolSearch`
//! activates) go through `rcu`. Sub-agent install snapshots the parent
//! set into a fresh `ArcSwap` when frontmatter `inherit_active_mcp` is
//! true (the default).

use std::collections::{BTreeSet, VecDeque};

/// Lightweight descriptor of a registered MCP tool used by the
/// `ToolSearch` built-in (ADR-0046). Defined here in agent-core so
/// `caliban-tools-builtin` can consume it without depending on
/// `caliban-mcp-client`. The MCP manager populates one per advertised
/// tool and exposes them via `list_mcp_tools()`.
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    /// Registry name in the canonical `mcp__<server>__<tool>` form.
    pub full_name: String,
    /// Description as advertised by the MCP server.
    pub description: String,
    /// JSON Schema for the tool's input.
    pub input_schema: serde_json::Value,
}

/// Sticky-per-session activation set with LRU eviction.
///
/// Invariants:
/// - `lru.len() == active.len()` at all times.
/// - `active.contains(name) ↔ lru.contains(name)`.
/// - When the set is non-empty, `lru.front()` is the most recently
///   activated entry; `lru.back()` is the next eviction candidate.
#[derive(Debug, Clone)]
pub struct McpActivationSet {
    cap: usize,
    /// Newest at front, oldest at back. Eviction pops from back.
    lru: VecDeque<String>,
    active: BTreeSet<String>,
}

impl McpActivationSet {
    /// Construct a new activation set with the given soft cap.
    ///
    /// `cap == 0` disables activation: every call to [`activate`] is a
    /// no-op. Callers SHOULD treat `cap == 0` as equivalent to
    /// `tools.lazy_mcp = false` and log a WARN at settings load.
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            lru: VecDeque::with_capacity(cap),
            active: BTreeSet::new(),
        }
    }

    /// Whether `name` is currently active.
    #[must_use]
    pub fn is_active(&self, name: &str) -> bool {
        self.active.contains(name)
    }

    /// Count of active entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.active.len()
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// Iterate active entries in MRU order (newest first).
    ///
    /// `/context` uses this so the most recently activated tool is
    /// listed first.
    pub fn iter_active(&self) -> impl Iterator<Item = &str> {
        self.lru.iter().map(String::as_str)
    }

    /// Activate `name`. Returns the evicted name when overflow
    /// triggers an LRU drop. Idempotent: re-activating a current
    /// member bumps it to the front of the LRU and returns `None`.
    pub fn activate(&mut self, name: &str) -> Option<String> {
        if self.cap == 0 {
            return None;
        }
        // Re-activation: just bump LRU position.
        if self.active.contains(name) {
            if let Some(idx) = self.lru.iter().position(|n| n == name) {
                let _ = self.lru.remove(idx);
            }
            self.lru.push_front(name.to_string());
            return None;
        }
        self.active.insert(name.to_string());
        self.lru.push_front(name.to_string());
        if self.lru.len() > self.cap
            && let Some(evicted) = self.lru.pop_back()
        {
            self.active.remove(&evicted);
            return Some(evicted);
        }
        None
    }

    /// Snapshot for sub-agent inheritance.
    ///
    /// The returned set is decoupled from `self`; mutations to either
    /// do not affect the other.
    #[must_use]
    pub fn snapshot(&self) -> Self {
        self.clone()
    }
}
