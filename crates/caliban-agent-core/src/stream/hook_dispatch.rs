//! Hook fan-out helpers used by the turn loop.
//!
//! Today this module owns the single-tool dispatch helper, which threads the
//! `before_tool` decision (including `UpdatedInput` rewrites) into the tool
//! invocation and then drives the `after_tool` hook. Additional hook
//! orchestration may move here as the loop body is further factored.

use std::sync::Arc;

use caliban_provider::{ContentBlock, TextBlock, ToolResultBlock};
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use super::StopCondition;
use crate::agent::Agent;
use crate::hooks::{HookDecision, ToolCtx};
use crate::tool::{ToolContext, ToolError};

// ---------------------------------------------------------------------------
// Helper: dispatch a single tool call
// ---------------------------------------------------------------------------

/// Dispatch one tool call: run `before_tool` hook, invoke the tool, run
/// `after_tool` hook. Returns the [`ToolResultBlock`] (possibly synthesized
/// for errors / denials). Returns `Err(StopCondition)` only on cancellation
/// or a hook failure that should abort the run.
#[instrument(skip(agent, input, cancel), fields(tool = tool_name, id = tool_use_id))]
pub(crate) async fn dispatch_tool(
    agent: &Agent,
    turn_index: u32,
    tool_use_id: &str,
    tool_name: &str,
    input: serde_json::Value,
    cancel: &CancellationToken,
) -> std::result::Result<ToolResultBlock, StopCondition> {
    if cancel.is_cancelled() {
        return Err(StopCondition::Cancelled);
    }

    // Keep `input` alive through all hook calls by cloning for the invoke call.
    let tool_ctx = ToolCtx {
        turn_index,
        tool_use_id,
        tool_name,
        input: &input,
    };

    // before_tool hook
    let decision = agent
        .hooks
        .before_tool(&tool_ctx)
        .await
        .map_err(|e| StopCondition::HookDenied(format!("before_tool hook failed: {e}")))?;

    // Choose the effective input: `UpdatedInput` overrides the original.
    let mut effective_input = input.clone();
    let invoke_result: std::result::Result<Vec<ContentBlock>, ToolError> = match decision {
        HookDecision::Deny(msg) => {
            let content = vec![ContentBlock::Text(TextBlock {
                text: format!("Tool call denied: {msg}"),
                cache_control: None,
            })];
            // Inform the after_tool hook about the denial.
            let denial_err = ToolError::execution(std::io::Error::other(format!("denied: {msg}")));
            if let Err(e) = agent.hooks.after_tool(&tool_ctx, &Err(denial_err)).await {
                tracing::warn!(tool = tool_name, error = %e, "after_tool hook error (non-fatal)");
            }
            return Ok(ToolResultBlock {
                tool_use_id: tool_use_id.to_string(),
                content,
                is_error: true,
            });
        }
        HookDecision::UpdatedInput(new_input) => {
            tracing::info!(
                tool = tool_name,
                tool_use_id,
                "hook.updated_input: tool input rewritten by before_tool hook"
            );
            effective_input = new_input;
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
                    tool.invoke(effective_input.clone(), cx).await
                }
            }
        }
        HookDecision::Allow => match agent.tools.get(tool_name) {
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
                // Clone `input` so the borrow on `tool_ctx` remains valid for after_tool.
                tool.invoke(input.clone(), cx).await
            }
        },
    };
    // Reference effective_input to silence dead-code in the Deny arm.
    let _ = &effective_input;

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
