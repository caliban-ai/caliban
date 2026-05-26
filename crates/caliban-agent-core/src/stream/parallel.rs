//! Parallel tool-dispatch types.
//!
//! The per-turn loop in `stream/mod.rs` plans tool calls serially (running the
//! `before_tool` hook gate), then dispatches the allowed ones in parallel via a
//! `FuturesUnordered` set bounded by a `Semaphore`. This module owns the small
//! plan enum that bridges the two phases.
//!
//! Per ADR 0016 (Revised 2026-05-26), each `Allowed` plan carries an optional
//! `conflict_key`. Plans sharing the same key serialize via a per-key
//! `tokio::sync::Mutex` (acquired FIFO) so two writes to the same target can't
//! interleave non-deterministically. Plans with `conflict_key = None` (the
//! default) parallelize freely as before.

use std::collections::HashMap;
use std::sync::Arc;

use caliban_provider::ToolResultBlock;
use tokio::sync::Mutex;

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
        /// `Some(key)` when this call must serialize against other batched
        /// calls sharing the same key; `None` (the default) for fully
        /// parallel-safe calls. See `Tool::parallel_conflict_key`.
        conflict_key: Option<String>,
    },
    /// `before_tool` returned `Deny`; the synthesized denial `ToolResult`
    /// stands in for the invoke.
    Denied {
        original_index: usize,
        result: ToolResultBlock,
    },
}

/// Build per-key serialization mutexes covering every distinct non-`None`
/// `conflict_key` in the plan list. Returned map is empty when no plan has a
/// conflict key — the common case.
pub(crate) fn build_conflict_locks(plans: &[DispatchPlan]) -> HashMap<String, Arc<Mutex<()>>> {
    let mut locks: HashMap<String, Arc<Mutex<()>>> = HashMap::new();
    for plan in plans {
        if let DispatchPlan::Allowed {
            conflict_key: Some(k),
            ..
        } = plan
        {
            locks
                .entry(k.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())));
        }
    }
    locks
}
