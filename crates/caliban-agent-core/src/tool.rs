//! Tool trait — implementations live in caliban-tools-builtin (D) and downstream.

use async_trait::async_trait;
use caliban_provider::ContentBlock;
use tokio_util::sync::CancellationToken;

/// Context passed to a Tool's `invoke` method.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// The model-assigned `tool_use_id` this invocation corresponds to.
    pub tool_use_id: String,
    /// Cancellation token; tools must honor this for long-running work.
    pub cancel: CancellationToken,
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
}
