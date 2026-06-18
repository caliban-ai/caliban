//! Agent loop, tool dispatch, cancellation, retry, compaction, and hooks
//! for the caliban agent harness. Drives an LLM conversation on top of
//! `caliban-provider`.

pub mod agent;
pub mod auto_mode;
pub mod cache;
pub mod compact;
pub mod decision_log;
pub mod deferred_block;
pub mod error;
pub mod hooks;
pub mod hooks_config;
pub mod hooks_router;
pub mod mcp_activation;
pub mod mode_filter;
pub mod permission_mode;
pub mod permissions;
pub mod permissions_matcher;
pub mod plan_mode;
pub mod post_process;
pub mod registry;
pub mod retry;
pub mod session;
pub mod stream;
pub mod todos;
pub mod tool;
pub mod turn;
pub mod wire_filter;

pub use agent::{Agent, AgentBuilder, AgentConfig, ModelSwapError, default_parallel_tool_limit};
pub use auto_mode::{
    AutoModeClassifier, AutoModeConfig, AutoModeDecision, AutoVerdict, CLASSIFIER_INPUT_CAP,
    DEFAULTS_TOKEN, DecisionSource, DefaultsKind, build_prompt as auto_mode_build_prompt,
    default_patterns as auto_mode_default_patterns,
    parse_classifier_response as auto_mode_parse_classifier_response,
};
pub use compact::{
    Compactor, DropOldestCompactor, MicroCompactor, NoopCompactor, SummarizingCompactor,
    estimate_tokens,
};
pub use error::{Error, Result};
pub use hooks::{
    CompactCtx, CompactOutcome, CompositeHooks, ConfigChangeCtx, CwdChangedCtx, FileChangeKind,
    FileChangedCtx, HookDecision, Hooks, NoopHooks, NotificationCtx, NotificationLevel, PermCtx,
    PromptCtx, RunCtx, RunHookOutcome, SessionCtx, SessionOutcome, SessionStartOutcome,
    SubagentCtx, SubagentOutcome, TaskCtx, TaskOutcome, ToolCtx, TurnCtx, TurnDecision,
    build_envelope, envelope_with_cwd,
};
pub use hooks_config::{HookHandlerConfig, HookHandlerType, HooksConfig, HooksConfigError};
pub use hooks_router::{
    AgentHook, HttpHook, McpHook, PromptHook, ShellCommandHook, build_config_hooks,
};
pub use mode_filter::ModeFilter;
pub use permission_mode::{
    FILE_EDIT_TOOLS, PermissionMode, SharedPermissionMode, is_file_edit_tool, resolve_startup_mode,
};
pub use permissions::{
    Action, AskHandler, NonInteractiveAskHandler, PermissionsHook, PermissionsLoadError, Rule,
    RuntimeRule, RuntimeRuleStore, default_rules, evaluate_rules,
};
// `load_rules` / `load_rules_file` are `#[deprecated]` in favor of
// `caliban-settings` (PR-T3-B). Re-exported with `#[allow(deprecated)]`
// so the consumer-side deprecation warning is the one that surfaces.
#[allow(deprecated)]
pub use permissions::{load_rules, load_rules_file};
pub use plan_mode::{SharedPlanMode, is_plan_control_tool, new_shared_plan_mode};
pub use post_process::{AssistantPostProcessor, NoopPostProcessor};
pub use registry::ToolRegistry;
pub use retry::RetryPolicy;
pub use session::Session;
pub use stream::{
    InputProvider, RunOutcome, RunSettings, StopCondition, TurnEvent, TurnEventStream, TurnOutcome,
};
pub use todos::{SharedTodos, Todo, TodoStatus, new_shared_todos};
pub use tool::{Tool, ToolContext, ToolError};

// Re-export from caliban-provider so callers can construct messages without
// pulling that crate explicitly.
pub use caliban_provider::{
    CompletionRequest, ContentBlock, Effort, Message, Role, StopReason, TextBlock, Usage,
};
