//! Built-in tools for the caliban agent harness.
//!
//! Each tool implements `caliban_agent_core::Tool` with a JSON Schema for its
//! input. All tools share a `WorkspaceRoot` for path resolution.

pub mod bash;
pub mod edit;
pub mod glob_;
pub mod grep;
pub mod read;
pub mod workspace;
pub mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use glob_::GlobTool;
pub use grep::GrepTool;
pub use read::ReadTool;
pub use workspace::WorkspaceRoot;
pub use write::WriteTool;
