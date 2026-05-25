//! Agent loop, tool dispatch, cancellation, retry, compaction, and hooks
//! for the caliban agent harness. Drives an LLM conversation on top of
//! `caliban-provider`.

pub mod agent;
pub(crate) mod cache;
pub mod compact;
pub mod error;
pub mod hooks;
pub mod hooks_config;
pub mod hooks_router;
pub mod permissions;
pub mod plan_mode;
pub mod post_process;
pub mod registry;
pub mod retry;
pub mod session;
pub mod stream;
pub mod todos;
pub mod tool;
pub mod turn;

pub use agent::{Agent, AgentBuilder, AgentConfig, default_parallel_tool_limit};
pub use compact::{
    Compactor, DropOldestCompactor, NoopCompactor, SummarizingCompactor, estimate_tokens,
};
pub use error::{Error, Result};
pub use hooks::{
    CompactCtx, CompactOutcome, CompositeHooks, ConfigChangeCtx, CwdChangedCtx, FileChangeKind,
    FileChangedCtx, HookDecision, Hooks, NoopHooks, NotificationCtx, NotificationLevel, PermCtx,
    PromptCtx, SessionCtx, SessionOutcome, SubagentCtx, SubagentOutcome, TaskCtx, TaskOutcome,
    ToolCtx, TurnCtx, build_envelope, envelope_with_cwd,
};
pub use hooks_config::{HookHandlerConfig, HookHandlerType, HooksConfig, HooksConfigError};
pub use hooks_router::{AgentHook, HttpHook, McpHook, PromptHook, ShellCommandHook};
pub use permissions::{
    Action, AskHandler, NonInteractiveAskHandler, PermissionsHook, PermissionsLoadError, Rule,
    default_rules, load_rules, load_rules_file,
};
pub use plan_mode::{
    PLAN_MODE_ALLOWLIST, SharedPlanMode, is_allowed_in_plan_mode, new_shared_plan_mode,
};
pub use post_process::{AssistantPostProcessor, NoopPostProcessor};
pub use registry::ToolRegistry;
pub use retry::RetryPolicy;
pub use session::Session;
pub use stream::{RunOutcome, StopCondition, TurnEvent, TurnEventStream, TurnOutcome};
pub use todos::{SharedTodos, Todo, TodoStatus, new_shared_todos};
pub use tool::{Tool, ToolContext, ToolError};

// Re-export from caliban-provider so callers can construct messages without
// pulling that crate explicitly.
pub use caliban_provider::{
    CompletionRequest, ContentBlock, Message, Role, StopReason, TextBlock, Usage,
};
