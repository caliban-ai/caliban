//! Built-in tools for the caliban agent harness.
//!
//! Each tool implements `caliban_agent_core::Tool` with a JSON Schema for its
//! input. All tools share a `WorkspaceRoot` for path resolution.
//!
//! Modules are grouped by capability:
//!
//! - [`fs`] — filesystem tools (`Read`, `Write`, `Edit`, `MultiEdit`, `NotebookEdit`).
//! - [`shell`] — shell execution (`Bash`) plus background-job tools.
//! - [`web`] — `WebFetch` / `WebSearch`.
//! - [`memory`] — auto-memory `ReadMemoryTopic` / `WriteMemoryTopic`.
//! - [`agent`] — sub-agent orchestration (`AgentTool`) and `TodoWrite`.
//! - [`search`] — `Glob` and `Grep`.
//! - [`plan`] — `EnterPlanMode` / `ExitPlanMode`.
//! - [`workspace`] — shared `WorkspaceRoot` path-resolution type.

pub mod agent;
pub mod fs;
pub mod input;
pub mod memory;
pub(crate) mod parallel;
pub mod plan;
pub mod search;
pub mod shell;
pub mod tool_search;
pub mod web;
pub mod workspace;

pub use agent::{
    AgentFactory, AgentTool, AgentToolInput, BackgroundSpawnResult, BackgroundSpawner,
    IsolationMode, TodoWriteTool, WorktreeOptions,
};
pub use fs::{EditTool, MultiEditTool, NotebookEditTool, ReadTool, WriteTool};
pub use input::parse_input;
pub use memory::{ReadMemoryTopicTool, WriteMemoryTopicTool};
pub use plan::{EnterPlanModeTool, ExitPlanModeTool};
pub use search::{GlobTool, GrepTool};
pub use shell::{
    BashBgRegistry, BashJob, BashOutputTool, BashStatus, BashTool, KillShellTool, RingBuffer,
    global_registry,
};
pub use web::{Provider as WebSearchProvider, SearchHit, WebFetchTool, WebSearchTool};
pub use workspace::WorkspaceRoot;
