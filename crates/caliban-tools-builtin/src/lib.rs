//! Built-in tools for the caliban agent harness.
//!
//! Each tool implements `caliban_agent_core::Tool` with a JSON Schema for its
//! input. All tools share a `WorkspaceRoot` for path resolution.

pub mod bash;
pub mod edit;
pub mod read;
pub mod workspace;
pub mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use read::ReadTool;
pub use workspace::WorkspaceRoot;
pub use write::WriteTool;
