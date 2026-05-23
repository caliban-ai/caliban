//! Agent loop, tool dispatch, cancellation, retry, compaction, and hooks
//! for the caliban agent harness. Drives an LLM conversation on top of
//! `caliban-provider`.

pub mod agent;
pub mod error;
pub mod hooks;
pub mod registry;
pub mod tool;

/// Compaction strategies for keeping message history within the model's context window.
///
/// This module contains placeholder types that will be replaced with full
/// implementations in Task 4.
pub mod compact {
    /// Strategy for keeping the message history under the model's input window.
    ///
    /// Placeholder trait — populated in Task 4.
    pub trait Compactor: Send + Sync {}

    /// No-op compactor that never truncates history.
    ///
    /// Placeholder — populated in Task 4.
    #[derive(Debug, Default)]
    pub struct NoopCompactor;

    impl Compactor for NoopCompactor {}
}

/// Retry policy and executor for provider calls.
///
/// This module contains a placeholder type that will be replaced with a full
/// implementation in Task 3.
pub mod retry {
    /// Configurable retry policy for provider calls.
    ///
    /// Placeholder — populated in Task 3.
    #[derive(Debug, Default)]
    pub struct RetryPolicy {}
}

/// Outcome of a single agent turn.
///
/// This is a temporary placeholder. Task 5 replaces it with the full type
/// from `src/stream.rs`.
#[derive(Debug)]
pub struct TurnOutcome {
    /// Placeholder field. Real definition arrives in Task 5.
    _placeholder: (),
}

pub use agent::{Agent, AgentBuilder, AgentConfig};
pub use error::{Error, Result};
pub use hooks::{HookDecision, Hooks, NoopHooks, ToolCtx, TurnCtx};
pub use registry::ToolRegistry;
pub use tool::{Tool, ToolContext, ToolError};

// Re-export from caliban-provider so callers can construct messages without
// pulling that crate explicitly.
pub use caliban_provider::{
    CompletionRequest, ContentBlock, Message, Role, StopReason, TextBlock, Usage,
};
