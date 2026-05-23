//! Agent loop, tool dispatch, cancellation, retry, compaction, and hooks
//! for the caliban agent harness. Drives an LLM conversation on top of
//! `caliban-provider`.

pub mod agent;
pub mod compact;
pub mod error;
pub mod hooks;
pub mod registry;
pub mod retry;
pub mod stream;
pub mod tool;
pub mod turn;

pub use agent::{Agent, AgentBuilder, AgentConfig};
pub use compact::{Compactor, DropOldestCompactor, NoopCompactor, SummarizingCompactor};
pub use error::{Error, Result};
pub use hooks::{HookDecision, Hooks, NoopHooks, ToolCtx, TurnCtx};
pub use registry::ToolRegistry;
pub use retry::RetryPolicy;
pub use stream::{RunOutcome, StopCondition, TurnEvent, TurnEventStream, TurnOutcome};
pub use tool::{Tool, ToolContext, ToolError};

// Re-export from caliban-provider so callers can construct messages without
// pulling that crate explicitly.
pub use caliban_provider::{
    CompletionRequest, ContentBlock, Message, Role, StopReason, TextBlock, Usage,
};
