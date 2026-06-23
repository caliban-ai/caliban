//! Hook fan-out helpers used by the turn loop.
//!
//! Today this module owns the single-tool dispatch helper, which threads the
//! `before_tool` decision (including `UpdatedInput` rewrites) into the tool
//! invocation and then drives the `after_tool` hook. Additional hook
//! orchestration may move here as the loop body is further factored.

use std::sync::Arc;
use std::time::Instant;

use caliban_common::tracing_targets::TARGET_TOOLS;
use caliban_provider::{ContentBlock, TextBlock, ToolResultBlock};
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use super::StopCondition;
use crate::agent::Agent;
use crate::hooks::ToolCtx;
use crate::tool::{ToolContext, ToolError};

// ---------------------------------------------------------------------------
// Helper: dispatch a single tool call
// ---------------------------------------------------------------------------

/// Dispatch one already-gated tool call: invoke the tool, then run the
/// `after_tool` hook. Returns the [`ToolResultBlock`] (possibly synthesized
/// for errors). Returns `Err(StopCondition)` only on cancellation.
///
/// The `before_tool` gate (permissions / Ask prompt / `UpdatedInput` rewrite)
/// already ran in the serial planning phase (`stream/mod.rs` Phase 1): `input`
/// is the effective, post-rewrite input, and denied calls were turned into
/// `DispatchPlan::Denied` and never reach here. Re-running the gate in this
/// phase would evaluate the policy twice and double-prompt the user for "Ask"
/// rules (#58), so dispatch only invokes + runs `after_tool`.
#[instrument(skip(agent, input, cancel), fields(tool = tool_name, id = tool_use_id))]
pub(crate) async fn dispatch_tool(
    agent: &Agent,
    session_id: &str,
    turn_index: u32,
    tool_use_id: &str,
    tool_name: &str,
    input: serde_json::Value,
    cancel: &CancellationToken,
) -> std::result::Result<ToolResultBlock, StopCondition> {
    if cancel.is_cancelled() {
        return Err(StopCondition::Cancelled);
    }

    // Borrow `input` for the after_tool ctx; cloned into the invoke call below.
    let tool_ctx = ToolCtx {
        session_id,
        turn_index,
        tool_use_id,
        tool_name,
        input: &input,
        is_read_only: agent.tools.get(tool_name).is_some_and(|t| t.is_read_only()),
    };

    // Per-tool dispatch record (#256). The per-turn aggregate in
    // `stream/mod.rs` only reports counts; this fires once per *call* so the
    // debug log carries a greppable, reconstructable tool timeline for EVERY
    // tool — independent of whether the tool emits its own internal tracing.
    // (Pre-#256 only Grep was visible, because its `ignore`/`globset` file
    // walk happened to emit DEBUG under the dispatch span.)
    tracing::debug!(
        target: TARGET_TOOLS,
        tool = tool_name,
        tool_use_id,
        turn_index,
        "tool dispatch start",
    );
    let started_at = Instant::now();

    let invoke_result: std::result::Result<Vec<ContentBlock>, ToolError> =
        match agent.tools.get(tool_name) {
            None => Err(ToolError::invalid_input(format!(
                "tool not found: {tool_name}"
            ))),
            Some(tool) => {
                let cx = ToolContext {
                    tool_use_id: tool_use_id.to_string(),
                    cancel: cancel.clone(),
                    hooks: Some(Arc::clone(&agent.hooks)),
                    turn_index,
                };
                tool.invoke(input.clone(), cx).await
            }
        };

    // Completion record — `status` lets a reader distinguish ok / error /
    // cancelled without parsing the result payload.
    let status = match &invoke_result {
        Ok(_) => "ok",
        Err(ToolError::Cancelled) => "cancelled",
        Err(_) => "error",
    };
    tracing::debug!(
        target: TARGET_TOOLS,
        tool = tool_name,
        tool_use_id,
        turn_index,
        status,
        duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        "tool dispatch end",
    );

    // after_tool hook (non-fatal; errors are logged by tracing, not propagated)
    if let Err(e) = agent.hooks.after_tool(&tool_ctx, &invoke_result).await {
        tracing::warn!(tool = tool_name, error = %e, "after_tool hook error (non-fatal)");
    }

    match invoke_result {
        Err(ToolError::Cancelled) => Err(StopCondition::Cancelled),
        Err(e) => Ok(ToolResultBlock {
            tool_use_id: tool_use_id.to_string(),
            content: vec![ContentBlock::Text(TextBlock {
                text: format!("Error: {e}"),
                cache_control: None,
            })],
            is_error: true,
        }),
        Ok(content) => Ok(ToolResultBlock {
            tool_use_id: tool_use_id.to_string(),
            content,
            is_error: false,
        }),
    }
}
