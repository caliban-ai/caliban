//! Tool trait — implementations live in caliban-tools-builtin (D) and downstream.

use std::sync::Arc;

use async_trait::async_trait;
use caliban_provider::ContentBlock;
use tokio_util::sync::CancellationToken;

/// Context passed to a Tool's `invoke` method.
#[derive(Clone)]
pub struct ToolContext {
    /// The model-assigned `tool_use_id` this invocation corresponds to.
    pub tool_use_id: String,
    /// Cancellation token; tools must honor this for long-running work.
    pub cancel: CancellationToken,
    /// Hooks handle so tools can fire `FileChanged`, `Notification`, etc. on
    /// successful side effects. Falls back to a no-op when unset (which is the
    /// case in unit tests that don't construct an `Agent`).
    pub hooks: Option<Arc<dyn crate::hooks::Hooks + Send + Sync>>,
    /// Zero-based turn index — surfaced to hooks for correlation with the
    /// surrounding turn.
    pub turn_index: u32,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("tool_use_id", &self.tool_use_id)
            .field("turn_index", &self.turn_index)
            .field("hooks", &self.hooks.is_some())
            .finish_non_exhaustive()
    }
}

/// Errors a `Tool::invoke` can return.
#[derive(thiserror::Error, Debug)]
pub enum ToolError {
    /// The JSON input did not match the expected schema.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// The tool encountered a runtime failure.
    #[error("execution failed: {0}")]
    Execution(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The tool was cancelled before it could complete.
    #[error("cancelled")]
    Cancelled,
}

impl ToolError {
    /// Construct an `Execution` variant from any error type.
    pub fn execution(e: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Execution(Box::new(e))
    }

    /// Construct an `InvalidInput` variant.
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }
}

/// Tool implementations register with `ToolRegistry`; the agent dispatches
/// `Provider`-emitted `tool_use` blocks to the matching `Tool::invoke`.
///
/// # Errors
///
/// Implementors of `invoke` should return [`ToolError::InvalidInput`] when the
/// provided JSON does not conform to the declared schema, and
/// [`ToolError::Execution`] for runtime failures.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable, unique-within-registry name. Must match the model's
    /// expected tool name in the system prompt or schema.
    fn name(&self) -> &str;

    /// Description sent to the model.
    fn description(&self) -> &str;

    /// JSON Schema for the input. Returned by reference to avoid cloning
    /// per request.
    fn input_schema(&self) -> &serde_json::Value;

    /// Execute the tool. Returns the content blocks to splice into the
    /// `ToolResult` message.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError::InvalidInput`] if the input does not match the
    /// schema, [`ToolError::Execution`] on runtime failure, or
    /// [`ToolError::Cancelled`] if the cancellation token was fired.
    async fn invoke(
        &self,
        input: serde_json::Value,
        cx: ToolContext,
    ) -> std::result::Result<Vec<ContentBlock>, ToolError>;

    /// Returns `Some(key)` when this tool call has a conflict identity that
    /// must not run in parallel with another call sharing the same key.
    /// Returns `None` (the default) when the tool is fully parallel-safe.
    ///
    /// The dispatcher groups batched calls by key: the `None` group runs
    /// fully in parallel (subject to the existing `parallel_tools` semaphore);
    /// each non-`None` key group runs serially in submission order. Groups
    /// run in parallel against each other.
    ///
    /// Override for tools whose effect is keyed to a specific target —
    /// typically the canonicalized path of a file the tool writes, or a
    /// scope+topic string for memory-tier writes. See ADR 0016 (Revised
    /// 2026-05-26) for the rationale.
    fn parallel_conflict_key(&self, _input: &serde_json::Value) -> Option<String> {
        None
    }

    /// Whether this tool is side-effect-free — it only reads state (or injects
    /// context) and never mutates the workspace or the outside world.
    ///
    /// Defaults to `false` (assume side effects). Read-only tools (`Read`,
    /// `Grep`, `Glob`, `WebFetch`, `Skill`, …) override it to return `true`.
    /// The permission layer uses this — instead of a hardcoded central name
    /// allowlist — to decide what may run while plan mode is active, so a new
    /// read-only built-in (or an MCP tool) becomes plan-safe just by overriding
    /// this method, with no central edit.
    fn is_read_only(&self) -> bool {
        false
    }

    /// Optional downcast hook for recovering a tool's concrete type at
    /// runtime. The default returns `None`; tools that expose extra
    /// session metadata beyond the trait (e.g. the `Skill` tool surfacing
    /// its loaded skill names) override it to return `Some(self)`.
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
}
