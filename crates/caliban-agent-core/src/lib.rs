//! Agent loop, tool dispatch, cancellation, retry, compaction, and hooks
//! for the caliban agent harness. Drives an LLM conversation on top of
//! `caliban-provider`.

pub mod error;
pub mod registry;
pub mod tool;

pub use error::{Error, Result};
pub use registry::ToolRegistry;
pub use tool::{Tool, ToolContext, ToolError};

// Re-export from caliban-provider so callers can construct messages without
// pulling that crate explicitly.
pub use caliban_provider::{
    CompletionRequest, ContentBlock, Message, Role, StopReason, TextBlock, Usage,
};
