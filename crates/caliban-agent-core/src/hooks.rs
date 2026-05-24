//! Hooks trait — pluggable callbacks for the agent's lifecycle (ADR 0024).
//!
//! Existing in-process callers only need the four legacy events
//! (`before_turn`/`after_turn`/`before_tool`/`after_tool`). The expanded
//! taxonomy adds session, prompt, compaction, subagent, task, permission,
//! filesystem, and notification events. All new methods have default no-op
//! implementations so existing `Hooks` impls keep compiling unchanged.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use caliban_provider::{ContentBlock, Message};

use crate::AgentConfig;
use crate::error::Result;
use crate::tool::ToolError;

/// Decision returned by [`Hooks::before_tool`] and
/// [`Hooks::user_prompt_submit`].
#[derive(Debug, Clone)]
pub enum HookDecision {
    /// Proceed with the dispatch / submission as normal.
    Allow,
    /// Reject the dispatch / submission; the reason is surfaced to the user.
    Deny(String),
    /// Rewrite the input JSON (for `before_tool`) or the prompt envelope (for
    /// `user_prompt_submit`) before dispatch. The new value is threaded through
    /// composed hooks so subsequent layers see the rewritten value.
    UpdatedInput(serde_json::Value),
}

/// Per-turn context passed to turn hooks.
#[derive(Debug)]
pub struct TurnCtx<'a> {
    /// Zero-based turn index within the current run.
    pub turn_index: u32,
    /// Snapshot of the message history at the start of this turn.
    pub messages: &'a [Message],
    /// Agent configuration in effect for this turn.
    pub config: &'a AgentConfig,
}

/// Per-tool context passed to tool hooks.
#[derive(Debug)]
pub struct ToolCtx<'a> {
    /// Zero-based turn index within the current run.
    pub turn_index: u32,
    /// The model-assigned `tool_use_id` for this invocation.
    pub tool_use_id: &'a str,
    /// Name of the tool being invoked.
    pub tool_name: &'a str,
    /// Input JSON passed to the tool.
    pub input: &'a serde_json::Value,
}

/// Per-session context for `SessionStart` / `SessionEnd` events.
#[derive(Debug)]
pub struct SessionCtx<'a> {
    /// Opaque session identifier (the caliban binary supplies a UUID-ish
    /// string; tests pass an arbitrary placeholder).
    pub session_id: &'a str,
    /// Workspace root for the session.
    pub cwd: &'a Path,
    /// Provider name (e.g. `"anthropic"`).
    pub provider: &'a str,
    /// Model identifier.
    pub model: &'a str,
}

/// Outcome of a session, surfaced to `SessionEnd` hooks.
#[derive(Debug, Clone)]
pub struct SessionOutcome {
    /// Number of turns executed across the session.
    pub turn_count: u32,
    /// Total input tokens consumed.
    pub input_tokens: u32,
    /// Total output tokens generated.
    pub output_tokens: u32,
}

/// Per-prompt context for `UserPromptSubmit`. Hooks may inspect the prompt
/// text + attachments and return a [`HookDecision`] to allow, deny, or
/// rewrite the prompt.
#[derive(Debug)]
pub struct PromptCtx<'a> {
    /// Session identifier.
    pub session_id: &'a str,
    /// Workspace root.
    pub cwd: &'a Path,
    /// Zero-based turn index for this prompt (i.e. the *next* turn).
    pub turn_index: u32,
    /// The user's prompt text.
    pub prompt: &'a str,
    /// Display paths of any `@`-attached files (best-effort).
    pub attachments: &'a [String],
}

/// Per-compaction context for `PreCompact` / `PostCompact`.
#[derive(Debug)]
pub struct CompactCtx<'a> {
    /// Session identifier.
    pub session_id: &'a str,
    /// Estimated token count of the history before compaction.
    pub token_count_before: u32,
    /// Compaction strategy name (e.g. `"DropOldest"`, `"Summarizing"`).
    pub strategy: &'a str,
}

/// Outcome of a compaction pass, surfaced to `PostCompact`.
#[derive(Debug, Clone)]
pub struct CompactOutcome {
    /// Estimated token count after compaction.
    pub token_count_after: u32,
    /// `true` if compaction actually mutated the history; `false` for no-op.
    pub compacted: bool,
}

/// Per-event context for `ConfigChange`.
#[derive(Debug)]
pub struct ConfigChangeCtx<'a> {
    /// Setting keys that changed.
    pub changed_keys: &'a [String],
    /// Short JSON-ish summary of the new settings; opaque to the trait.
    pub new_settings_summary: &'a str,
}

/// Per-event context for `CwdChanged`.
#[derive(Debug)]
pub struct CwdChangedCtx<'a> {
    /// Previous workspace root.
    pub old_cwd: &'a Path,
    /// New workspace root.
    pub new_cwd: &'a Path,
}

/// Kind of filesystem mutation observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    /// File created (didn't exist before the write).
    Created,
    /// Existing file modified.
    Modified,
    /// File deleted.
    Deleted,
}

impl FileChangeKind {
    /// Lower-case string spelling for serialization.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

/// Per-event context for `FileChanged`.
#[derive(Debug)]
pub struct FileChangedCtx<'a> {
    /// Path that was written / created / deleted.
    pub path: &'a Path,
    /// Kind of mutation.
    pub kind: FileChangeKind,
    /// Tool that caused the change (e.g. `"Write"`, `"Edit"`).
    pub tool: &'a str,
}

/// Per-event context for `PermissionRequest` / `PermissionDenied`.
#[derive(Debug)]
pub struct PermCtx<'a> {
    /// Zero-based turn index.
    pub turn_index: u32,
    /// The model-assigned `tool_use_id`.
    pub tool_use_id: &'a str,
    /// Name of the tool whose dispatch is being gated.
    pub tool_name: &'a str,
    /// Input JSON for the tool call.
    pub input: &'a serde_json::Value,
    /// The matched permission rule's action (`"allow"`/`"deny"`/`"ask"`).
    pub rule_action: &'a str,
    /// Optional rule comment.
    pub rule_comment: Option<&'a str>,
}

/// Notification severity surfaced to the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationLevel {
    /// Informational.
    Info,
    /// Warning.
    Warn,
    /// Error.
    Error,
}

impl NotificationLevel {
    /// Lower-case string spelling for serialization.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Per-event context for `Notification` (TUI banner, CLI toast, etc.).
#[derive(Debug)]
pub struct NotificationCtx<'a> {
    /// Severity.
    pub level: NotificationLevel,
    /// User-visible message.
    pub message: &'a str,
}

/// Per-event context for `SubagentStart` / `SubagentStop`.
#[derive(Debug)]
pub struct SubagentCtx<'a> {
    /// Parent turn index that spawned the sub-agent.
    pub parent_turn_index: u32,
    /// Logical sub-agent name (e.g. `"code-review"`); empty when anonymous.
    pub agent_name: &'a str,
    /// Stable identifier for this sub-agent invocation (e.g. the parent's
    /// `tool_use_id`).
    pub task_id: &'a str,
}

/// Outcome of a sub-agent invocation, surfaced to `SubagentStop`.
#[derive(Debug, Clone)]
pub struct SubagentOutcome {
    /// Whether the sub-agent exited cleanly (`false` for cancellation / error
    /// / `MaxTurnsReached`).
    pub success: bool,
    /// Final assistant text returned to the parent (may be truncated).
    pub final_text: String,
}

/// Per-event context for `TaskCreated` / `TaskCompleted`.
///
/// "Task" here is the TodoWrite-tier task (not the sub-agent invocation).
#[derive(Debug)]
pub struct TaskCtx<'a> {
    /// Task identifier (stable across status transitions).
    pub task_id: &'a str,
    /// Task description.
    pub content: &'a str,
    /// Current status as a lowercase string (e.g. `"pending"`).
    pub status: &'a str,
}

/// Outcome of a task transitioning to its terminal status (completed /
/// cancelled).
#[derive(Debug, Clone)]
pub struct TaskOutcome {
    /// Terminal status spelling (`"completed"` or `"cancelled"`).
    pub terminal_status: String,
}

/// Pluggable lifecycle callbacks for the agent loop.
///
/// All methods have default no-op implementations, so implementors only need
/// to override the hooks they care about. The default implementation is
/// [`NoopHooks`].
#[async_trait]
pub trait Hooks: Send + Sync {
    /// Called before each turn begins (before compaction and the provider call).
    async fn before_turn(&self, _ctx: &TurnCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Called after each turn completes (after tool dispatch, before the next
    /// turn or the final `RunEnd` event).
    async fn after_turn(&self, _ctx: &TurnCtx<'_>, _outcome: &crate::TurnOutcome) -> Result<()> {
        Ok(())
    }

    /// Called before each tool invocation. Return [`HookDecision::Deny`] to
    /// short-circuit the dispatch, or [`HookDecision::UpdatedInput`] to
    /// rewrite the tool's input JSON before dispatch.
    async fn before_tool(&self, _ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        Ok(HookDecision::Allow)
    }

    /// Called after each tool invocation (or denial) with the result.
    ///
    /// **Ordering note:** Under parallel tool dispatch (the default), this
    /// hook fires once per tool but **not** in assistant-message order —
    /// it fires in completion order. Each call carries the tool's
    /// `tool_use_id` and `tool_name` in [`ToolCtx`] so implementors can
    /// correlate. For denials (returned by [`Hooks::before_tool`]), this
    /// hook still fires once with `Err(ToolError::Execution(...))`.
    async fn after_tool(
        &self,
        _ctx: &ToolCtx<'_>,
        _result: &std::result::Result<Vec<ContentBlock>, ToolError>,
    ) -> Result<()> {
        Ok(())
    }

    /// Fired once when a session begins (after settings load, before the
    /// first user prompt).
    async fn session_start(&self, _ctx: &SessionCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Fired once when a session ends (after the last run, before persistence).
    async fn session_end(&self, _ctx: &SessionCtx<'_>, _outcome: &SessionOutcome) -> Result<()> {
        Ok(())
    }

    /// Fired before a user prompt is appended to history. Hooks may return
    /// [`HookDecision::Deny`] to reject the prompt or
    /// [`HookDecision::UpdatedInput`] (with a string value) to rewrite it.
    async fn user_prompt_submit(&self, _ctx: &PromptCtx<'_>) -> Result<HookDecision> {
        Ok(HookDecision::Allow)
    }

    /// Fired immediately before a compaction pass runs.
    async fn pre_compact(&self, _ctx: &CompactCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Fired immediately after a compaction pass completes.
    async fn post_compact(&self, _ctx: &CompactCtx<'_>, _outcome: &CompactOutcome) -> Result<()> {
        Ok(())
    }

    /// Fired when the active settings/config change at runtime (e.g. live
    /// reload of `permissions.toml` or `hooks.toml`).
    async fn config_change(&self, _ctx: &ConfigChangeCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Fired when the workspace root changes during a session.
    async fn cwd_changed(&self, _ctx: &CwdChangedCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Fired after a file is created, modified, or deleted by a built-in tool.
    async fn file_changed(&self, _ctx: &FileChangedCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Fired before the `Ask` modal is shown to the operator.
    async fn permission_request(&self, _ctx: &PermCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Fired after a permission rule has denied a tool dispatch.
    async fn permission_denied(&self, _ctx: &PermCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Fired when a user-visible notification (TUI banner, toast) is shown.
    async fn notification(&self, _ctx: &NotificationCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Fired before a sub-agent run begins.
    async fn subagent_start(&self, _ctx: &SubagentCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Fired after a sub-agent run completes (with or without success).
    async fn subagent_stop(
        &self,
        _ctx: &SubagentCtx<'_>,
        _outcome: &SubagentOutcome,
    ) -> Result<()> {
        Ok(())
    }

    /// Fired when a `TodoWrite` task transitions from non-existent / pending
    /// to in-progress.
    async fn task_created(&self, _ctx: &TaskCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Fired when a `TodoWrite` task transitions to a terminal status
    /// (`completed` or `cancelled`).
    async fn task_completed(&self, _ctx: &TaskCtx<'_>, _outcome: &TaskOutcome) -> Result<()> {
        Ok(())
    }
}

/// Default no-op hooks. Use this when you don't need observability callbacks.
#[derive(Debug, Default)]
pub struct NoopHooks;

#[async_trait]
impl Hooks for NoopHooks {}

// ---------------------------------------------------------------------------
// CompositeHooks
// ---------------------------------------------------------------------------

/// Fan an event out to multiple hook implementations in order.
///
/// - `before_*` events run top → bottom; the **first** `Deny` short-circuits.
/// - `after_*` events run bottom → top (LIFO) so the most recently added
///   observer sees the result first.
/// - [`HookDecision::UpdatedInput`] threads through: later hooks see the
///   rewritten input in their context (the composite owns a fresh `Value`
///   that supersedes the caller's borrow).
///
/// This lets [`crate::permissions::PermissionsHook`] compose with audit /
/// observability hooks loaded from `hooks.toml`.
pub struct CompositeHooks {
    layers: Vec<std::sync::Arc<dyn Hooks>>,
}

impl std::fmt::Debug for CompositeHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeHooks")
            .field("layers", &self.layers.len())
            .finish()
    }
}

impl CompositeHooks {
    /// Build a composite from an ordered list of layers. The first layer is
    /// the outermost (highest priority for `before_*`).
    #[must_use]
    pub fn new(layers: Vec<std::sync::Arc<dyn Hooks>>) -> Self {
        Self { layers }
    }

    /// Number of layers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    /// True when no layers are configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }
}

#[async_trait]
impl Hooks for CompositeHooks {
    async fn before_turn(&self, ctx: &TurnCtx<'_>) -> Result<()> {
        for h in &self.layers {
            h.before_turn(ctx).await?;
        }
        Ok(())
    }

    async fn after_turn(&self, ctx: &TurnCtx<'_>, outcome: &crate::TurnOutcome) -> Result<()> {
        for h in self.layers.iter().rev() {
            h.after_turn(ctx, outcome).await?;
        }
        Ok(())
    }

    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        // Thread UpdatedInput through layers by owning the latest value.
        let mut latest_input: Option<serde_json::Value> = None;
        for h in &self.layers {
            // Build a fresh ctx that points to the latest input (when present).
            let effective_input: &serde_json::Value = latest_input.as_ref().unwrap_or(ctx.input);
            let layer_ctx = ToolCtx {
                turn_index: ctx.turn_index,
                tool_use_id: ctx.tool_use_id,
                tool_name: ctx.tool_name,
                input: effective_input,
            };
            match h.before_tool(&layer_ctx).await? {
                HookDecision::Allow => {}
                HookDecision::Deny(msg) => return Ok(HookDecision::Deny(msg)),
                HookDecision::UpdatedInput(new_input) => {
                    latest_input = Some(new_input);
                }
            }
        }
        match latest_input {
            Some(v) => Ok(HookDecision::UpdatedInput(v)),
            None => Ok(HookDecision::Allow),
        }
    }

    async fn after_tool(
        &self,
        ctx: &ToolCtx<'_>,
        result: &std::result::Result<Vec<ContentBlock>, ToolError>,
    ) -> Result<()> {
        for h in self.layers.iter().rev() {
            h.after_tool(ctx, result).await?;
        }
        Ok(())
    }

    async fn session_start(&self, ctx: &SessionCtx<'_>) -> Result<()> {
        for h in &self.layers {
            h.session_start(ctx).await?;
        }
        Ok(())
    }

    async fn session_end(&self, ctx: &SessionCtx<'_>, outcome: &SessionOutcome) -> Result<()> {
        for h in self.layers.iter().rev() {
            h.session_end(ctx, outcome).await?;
        }
        Ok(())
    }

    async fn user_prompt_submit(&self, ctx: &PromptCtx<'_>) -> Result<HookDecision> {
        let mut latest_prompt: Option<String> = None;
        for h in &self.layers {
            let effective_prompt = latest_prompt.as_deref().unwrap_or(ctx.prompt);
            let layer_ctx = PromptCtx {
                session_id: ctx.session_id,
                cwd: ctx.cwd,
                turn_index: ctx.turn_index,
                prompt: effective_prompt,
                attachments: ctx.attachments,
            };
            match h.user_prompt_submit(&layer_ctx).await? {
                HookDecision::Allow => {}
                HookDecision::Deny(msg) => return Ok(HookDecision::Deny(msg)),
                HookDecision::UpdatedInput(new_input) => {
                    // Only string rewrites are meaningful for prompts.
                    if let Some(s) = new_input.as_str() {
                        latest_prompt = Some(s.to_string());
                    } else {
                        latest_prompt = Some(new_input.to_string());
                    }
                }
            }
        }
        match latest_prompt {
            Some(s) => Ok(HookDecision::UpdatedInput(serde_json::Value::String(s))),
            None => Ok(HookDecision::Allow),
        }
    }

    async fn pre_compact(&self, ctx: &CompactCtx<'_>) -> Result<()> {
        for h in &self.layers {
            h.pre_compact(ctx).await?;
        }
        Ok(())
    }

    async fn post_compact(&self, ctx: &CompactCtx<'_>, outcome: &CompactOutcome) -> Result<()> {
        for h in self.layers.iter().rev() {
            h.post_compact(ctx, outcome).await?;
        }
        Ok(())
    }

    async fn config_change(&self, ctx: &ConfigChangeCtx<'_>) -> Result<()> {
        for h in &self.layers {
            h.config_change(ctx).await?;
        }
        Ok(())
    }

    async fn cwd_changed(&self, ctx: &CwdChangedCtx<'_>) -> Result<()> {
        for h in &self.layers {
            h.cwd_changed(ctx).await?;
        }
        Ok(())
    }

    async fn file_changed(&self, ctx: &FileChangedCtx<'_>) -> Result<()> {
        for h in &self.layers {
            h.file_changed(ctx).await?;
        }
        Ok(())
    }

    async fn permission_request(&self, ctx: &PermCtx<'_>) -> Result<()> {
        for h in &self.layers {
            h.permission_request(ctx).await?;
        }
        Ok(())
    }

    async fn permission_denied(&self, ctx: &PermCtx<'_>) -> Result<()> {
        for h in &self.layers {
            h.permission_denied(ctx).await?;
        }
        Ok(())
    }

    async fn notification(&self, ctx: &NotificationCtx<'_>) -> Result<()> {
        for h in &self.layers {
            h.notification(ctx).await?;
        }
        Ok(())
    }

    async fn subagent_start(&self, ctx: &SubagentCtx<'_>) -> Result<()> {
        for h in &self.layers {
            h.subagent_start(ctx).await?;
        }
        Ok(())
    }

    async fn subagent_stop(&self, ctx: &SubagentCtx<'_>, outcome: &SubagentOutcome) -> Result<()> {
        for h in self.layers.iter().rev() {
            h.subagent_stop(ctx, outcome).await?;
        }
        Ok(())
    }

    async fn task_created(&self, ctx: &TaskCtx<'_>) -> Result<()> {
        for h in &self.layers {
            h.task_created(ctx).await?;
        }
        Ok(())
    }

    async fn task_completed(&self, ctx: &TaskCtx<'_>, outcome: &TaskOutcome) -> Result<()> {
        for h in self.layers.iter().rev() {
            h.task_completed(ctx, outcome).await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Envelope builders for external handlers (`hookEventName` / `sessionId` /
// `tool` / etc.)
// ---------------------------------------------------------------------------

/// Build the JSON envelope passed to external hook handlers. The shape
/// matches ADR 0024 §"Stdin payload shape": `hookEventName` and
/// `hookSpecificOutput` are `camelCase` (Claude Code parity); everything else
/// (`session_id`, `tool_use_id`, `turn_index`) is `snake_case`.
#[must_use]
pub fn build_envelope(event_name: &str, fields: serde_json::Value) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "hookEventName".into(),
        serde_json::Value::String(event_name.to_string()),
    );
    if let serde_json::Value::Object(map) = fields {
        for (k, v) in map {
            obj.insert(k, v);
        }
    }
    serde_json::Value::Object(obj)
}

/// Convenience: build an envelope including a `cwd` field.
#[must_use]
pub fn envelope_with_cwd(
    event_name: &str,
    cwd: &Path,
    mut fields: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    fields.insert(
        "cwd".into(),
        serde_json::Value::String(cwd.display().to_string()),
    );
    build_envelope(event_name, serde_json::Value::Object(fields))
}

// Internal: turn a PathBuf into the envelope's `path` field representation.
#[allow(dead_code)]
fn path_to_value(p: &Path) -> serde_json::Value {
    serde_json::Value::String(p.display().to_string())
}

// Suppress unused-warnings when a binary doesn't pull every helper.
#[doc(hidden)]
pub fn __noop_path_use(p: PathBuf) -> PathBuf {
    p
}
