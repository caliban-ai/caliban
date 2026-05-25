//! Parallel tool-dispatch types.
//!
//! The per-turn loop in `stream/mod.rs` plans tool calls serially (running the
//! `before_tool` hook gate), then dispatches the allowed ones in parallel via a
//! `FuturesUnordered` set bounded by a `Semaphore`. This module owns the small
//! plan enum that bridges the two phases.

use caliban_provider::ToolResultBlock;

// ---------------------------------------------------------------------------
// Per-turn dispatch plan
// ---------------------------------------------------------------------------

/// A single tool dispatch plan, produced by the serial `before_tool` gate.
///
/// `original_index` is the position of the corresponding `ContentBlock::ToolUse`
/// within the assistant message; it's used to reorder results back into
/// assistant-message order for history.
pub(crate) enum DispatchPlan {
    /// `before_tool` returned `Allow`; the invoke will run.
    Allowed {
        original_index: usize,
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// `before_tool` returned `Deny`; the synthesized denial `ToolResult`
    /// stands in for the invoke.
    Denied {
        original_index: usize,
        result: ToolResultBlock,
    },
}
