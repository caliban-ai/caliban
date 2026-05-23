//! Built-in tools for the caliban agent harness.
//!
//! Each tool implements `caliban_agent_core::Tool` with a JSON Schema for its
//! input. All tools share a `WorkspaceRoot` for path resolution.

pub mod workspace;

pub use workspace::WorkspaceRoot;
