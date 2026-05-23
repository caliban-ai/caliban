//! Hooks trait — pluggable callbacks for pre/post turn + pre/post tool.

use async_trait::async_trait;
use caliban_provider::{ContentBlock, Message};

use crate::AgentConfig;
use crate::error::Result;
use crate::tool::ToolError;

/// Decision returned by [`Hooks::before_tool`].
#[derive(Debug, Clone)]
pub enum HookDecision {
    /// Proceed with the tool invocation as normal.
    Allow,
    /// Skip the tool; synthesize a `ToolResult` with the given denial message.
    Deny(String),
}

/// Per-turn context passed to turn hooks.
#[derive(Debug)]
pub struct TurnCtx<'a> {
    /// Zero-based turn index within the current run.
    pub turn_index: u32,
    /// Snapshot of the message history at the start of this turn.
    pub messages: &'a [Message],
    /// Agent configuration in effect for this turn.
    pub config: &'a AgentConfig,
}

/// Per-tool context passed to tool hooks.
#[derive(Debug)]
pub struct ToolCtx<'a> {
    /// Zero-based turn index within the current run.
    pub turn_index: u32,
    /// The model-assigned `tool_use_id` for this invocation.
    pub tool_use_id: &'a str,
    /// Name of the tool being invoked.
    pub tool_name: &'a str,
    /// Input JSON passed to the tool.
    pub input: &'a serde_json::Value,
}

/// Pluggable lifecycle callbacks for the agent loop.
///
/// All methods have default no-op implementations, so implementors only need
/// to override the hooks they care about. The default implementation is
/// [`NoopHooks`].
#[async_trait]
pub trait Hooks: Send + Sync {
    /// Called before each turn begins (before compaction and the provider call).
    async fn before_turn(&self, _ctx: &TurnCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Called after each turn completes (after tool dispatch, before the next
    /// turn or the final `RunEnd` event).
    async fn after_turn(&self, _ctx: &TurnCtx<'_>, _outcome: &crate::TurnOutcome) -> Result<()> {
        Ok(())
    }

    /// Called before each tool invocation. Return [`HookDecision::Deny`] to
    /// short-circuit the dispatch.
    async fn before_tool(&self, _ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        Ok(HookDecision::Allow)
    }

    /// Called after each tool invocation (or denial) with the result.
    async fn after_tool(
        &self,
        _ctx: &ToolCtx<'_>,
        _result: &std::result::Result<Vec<ContentBlock>, ToolError>,
    ) -> Result<()> {
        Ok(())
    }
}

/// Default no-op hooks. Use this when you don't need observability callbacks.
#[derive(Debug, Default)]
pub struct NoopHooks;

#[async_trait]
impl Hooks for NoopHooks {}
