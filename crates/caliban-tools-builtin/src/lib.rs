//! Built-in tools for the caliban agent harness.
//!
//! Each tool implements `caliban_agent_core::Tool` with a JSON Schema for its
//! input. All tools share a `WorkspaceRoot` for path resolution.

pub mod agent_tool;
pub mod bash;
pub mod bash_bg;
pub mod edit;
pub mod glob_;
pub mod grep;
pub mod memory;
pub mod multi_edit;
pub mod notebook_edit;
pub mod plan_mode_tools;
pub mod read;
pub mod todo_write;
pub mod web_fetch;
pub mod web_search;
pub mod workspace;
pub mod write;

pub use agent_tool::{AgentFactory, AgentTool, AgentToolInput};
pub use bash::BashTool;
pub use bash_bg::{
    BashBgRegistry, BashJob, BashOutputTool, BashStatus, KillShellTool, RingBuffer, global_registry,
};
pub use edit::EditTool;
pub use glob_::GlobTool;
pub use grep::GrepTool;
pub use memory::{ReadMemoryTopicTool, WriteMemoryTopicTool};
pub use multi_edit::MultiEditTool;
pub use notebook_edit::NotebookEditTool;
pub use plan_mode_tools::{EnterPlanModeTool, ExitPlanModeTool};
pub use read::ReadTool;
pub use todo_write::TodoWriteTool;
pub use web_fetch::WebFetchTool;
pub use web_search::{Provider as WebSearchProvider, SearchHit, WebSearchTool};
pub use workspace::WorkspaceRoot;
pub use write::WriteTool;
