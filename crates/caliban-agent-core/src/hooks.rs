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
use tokio_util::sync::CancellationToken;

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

/// Outcome of [`Hooks::after_turn`] / [`Hooks::after_turn_failure`].
///
/// `ContinueWith` allows a hook to inject additional user messages and force
/// the loop to take another turn; the agent caps this to
/// `MAX_FORCED_CONTINUATIONS = 3` per run to avoid death-spirals.
#[derive(Debug, Clone)]
pub enum TurnDecision {
    /// Let the agent loop decide whether to continue (the default).
    Continue,
    /// Inject `Vec<Message>` into history and force another turn (capped).
    ContinueWith(Vec<caliban_provider::Message>),
    /// Halt the run immediately with `StopCondition::HookDenied`.
    Stop,
}

/// Outcome of [`Hooks::session_start`]. Carries context blocks a `SessionStart`
/// hook wants spliced into the system prompt before the first turn. Empty by
/// default (the common case: a hook with no context to contribute).
#[derive(Debug, Clone, Default)]
pub struct SessionStartOutcome {
    /// Context blocks contributed by `SessionStart` hooks, in firing order.
    /// Each entry is appended to the system prompt's session-context block.
    pub additional_context: Vec<String>,
}

/// Per-run context for the `before_run` / `after_run` lifecycle events
/// (ADR 0028). Fires once at the start / end of each `Agent::run` invocation.
#[derive(Debug)]
pub struct RunCtx<'a> {
    /// Opaque session identifier (the caliban binary supplies a UUID-ish
    /// string; tests pass an arbitrary placeholder).
    pub session_id: &'a str,
    /// Workspace root for this run.
    pub workspace_root: &'a Path,
    /// Optional reference to the user message that initiated the run. `None`
    /// when the run was triggered by a non-prompt entry-point (e.g. a
    /// programmatic resume or session-replay path).
    pub user_message: Option<&'a Message>,
    /// Monotonic prompt index within the parent session — incremented by the
    /// caller before each `before_run`. Used by `caliban-checkpoint` to name
    /// the per-prompt checkpoint directory.
    pub prompt_index: u32,
    /// Cancellation token tied to the parent run; honored by long-running
    /// hook implementations (the checkpoint hook is the canonical caller —
    /// it aborts pre-image reads when the run is cancelled).
    pub cancel: CancellationToken,
}

/// Outcome of a single agent run, surfaced to `after_run` hooks.
///
/// Distinct from [`crate::stream::RunOutcome`], which is the streaming-loop's
/// outer return value (includes the full message history). The hook-surface
/// variant is intentionally small — hooks should not depend on the full
/// transcript.
#[derive(Debug, Clone)]
pub struct RunHookOutcome {
    /// Number of turns the run actually executed.
    pub turn_count: u32,
    /// Total input tokens consumed.
    pub input_tokens: u32,
    /// Total output tokens generated.
    pub output_tokens: u32,
    /// `true` when the run terminated cleanly; `false` for cancellation,
    /// provider error, or hook denial.
    pub success: bool,
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
    /// Whether the resolved tool reports itself side-effect-free
    /// ([`crate::Tool::is_read_only`]). Populated by the dispatcher from the
    /// live registry and consumed by plan-mode gating. `false` when the tool
    /// is unknown or has side effects.
    pub is_read_only: bool,
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
    /// Fired once at the start of an `Agent::run` invocation (ADR 0028).
    ///
    /// Wraps the entire turn loop. The default no-op preserves existing
    /// `Hooks` impls. The canonical consumer is `caliban-checkpoint`, which
    /// uses this event to allocate a per-prompt manifest before any tool
    /// dispatches.
    async fn before_run(&self, _ctx: &RunCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Fired once at the end of an `Agent::run` invocation (ADR 0028).
    ///
    /// Receives the run's accumulated [`RunHookOutcome`]. The default no-op
    /// preserves existing `Hooks` impls.
    async fn after_run(&self, _ctx: &RunCtx<'_>, _outcome: &RunHookOutcome) -> Result<()> {
        Ok(())
    }

    /// Called **instead of** [`Hooks::after_run`] when the run ended in
    /// failure (any [`crate::StopCondition::is_failure`] variant).
    ///
    /// Default is a no-op; observability for failure modes. Implementors
    /// should NOT mutate session state from this method to avoid death
    /// spirals where the failure-cleanup itself triggers another run.
    async fn after_run_failure(&self, _ctx: &RunCtx<'_>, _outcome: &RunHookOutcome) -> Result<()> {
        Ok(())
    }

    /// Called before each turn begins (before compaction and the provider call).
    async fn before_turn(&self, _ctx: &TurnCtx<'_>) -> Result<()> {
        Ok(())
    }

    /// Called after each turn completes (after tool dispatch, before the next
    /// turn or the final `RunEnd` event).
    ///
    /// Returns a [`TurnDecision`] so hooks can request continuation
    /// (`ContinueWith(messages)`) or halt the run (`Stop`). The default is
    /// `Continue`, which preserves existing behavior. `ContinueWith` is
    /// capped to 3 forced continuations per run.
    async fn after_turn(
        &self,
        _ctx: &TurnCtx<'_>,
        _outcome: &crate::TurnOutcome,
    ) -> Result<TurnDecision> {
        Ok(TurnDecision::Continue)
    }

    /// Called **instead of** [`Hooks::after_turn`] when the turn ended in
    /// failure.
    async fn after_turn_failure(
        &self,
        _ctx: &TurnCtx<'_>,
        _outcome: &crate::TurnOutcome,
    ) -> Result<()> {
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
    /// first user prompt). Return [`SessionStartOutcome`] to contribute
    /// context spliced into the system prompt before turn 1.
    async fn session_start(&self, _ctx: &SessionCtx<'_>) -> Result<SessionStartOutcome> {
        Ok(SessionStartOutcome::default())
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

    /// Returns `true` when this hook implementation is guaranteed to be a
    /// no-op for every event. Used by [`CompositeHooks`] to short-circuit
    /// fan-out when every member is a no-op (avoids per-event `await` yields
    /// on the hot path).
    ///
    /// The default returns `false`; only [`NoopHooks`] overrides to `true`.
    /// Custom implementations may opt in if they truly do nothing — the
    /// composite trusts this signal and will skip calling the impl entirely.
    fn is_noop(&self) -> bool {
        false
    }
}

/// Default no-op hooks. Use this when you don't need observability callbacks.
#[derive(Debug, Default)]
pub struct NoopHooks;

#[async_trait]
impl Hooks for NoopHooks {
    fn is_noop(&self) -> bool {
        true
    }
}

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
///
/// ## All-noop short-circuit
///
/// When every layer reports `Hooks::is_noop() == true` (or the composite is
/// empty), `all_noop` is set to `true` at construction. Each event method
/// then returns the default `Ok(...)` immediately without iterating /
/// awaiting any layer. This eliminates 15+ wasted `await` yields per
/// turn-end on the common path where no hooks are configured.
///
/// The flag is monotonic in the direction of "we have a real hook": once
/// [`CompositeHooks::push`] is given a non-noop layer the flag flips to
/// `false` and stays false even if a later `push` adds a [`NoopHooks`].
pub struct CompositeHooks {
    layers: Vec<std::sync::Arc<dyn Hooks>>,
    /// `true` when every layer is a no-op (or `layers` is empty). Set at
    /// construction; updated by [`CompositeHooks::push`].
    all_noop: bool,
}

impl std::fmt::Debug for CompositeHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeHooks")
            .field("layers", &self.layers.len())
            .field("all_noop", &self.all_noop)
            .finish()
    }
}

impl CompositeHooks {
    /// Build a composite from an ordered list of layers. The first layer is
    /// the outermost (highest priority for `before_*`).
    #[must_use]
    pub fn new(layers: Vec<std::sync::Arc<dyn Hooks>>) -> Self {
        let all_noop = layers.iter().all(|h| h.is_noop());
        Self { layers, all_noop }
    }

    /// Append a layer to the composite. The flag tracking the all-noop
    /// short-circuit is updated so that adding a non-noop layer flips it to
    /// `false`; adding a [`NoopHooks`] after a real hook keeps the flag
    /// `false` (monotonic in the direction of "we have a real hook").
    pub fn push(&mut self, layer: std::sync::Arc<dyn Hooks>) {
        if !layer.is_noop() {
            self.all_noop = false;
        }
        self.layers.push(layer);
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

    /// True when every layer is a no-op (or the composite is empty). When
    /// `true`, every event method returns the default `Ok(...)` without
    /// awaiting any member.
    #[must_use]
    pub fn all_noop(&self) -> bool {
        self.all_noop
    }
}

#[async_trait]
impl Hooks for CompositeHooks {
    async fn before_run(&self, ctx: &RunCtx<'_>) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.before_run(ctx).await?;
        }
        Ok(())
    }

    async fn after_run(&self, ctx: &RunCtx<'_>, outcome: &RunHookOutcome) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in self.layers.iter().rev() {
            h.after_run(ctx, outcome).await?;
        }
        Ok(())
    }

    async fn after_run_failure(&self, ctx: &RunCtx<'_>, outcome: &RunHookOutcome) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in self.layers.iter().rev() {
            h.after_run_failure(ctx, outcome).await?;
        }
        Ok(())
    }

    async fn before_turn(&self, ctx: &TurnCtx<'_>) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.before_turn(ctx).await?;
        }
        Ok(())
    }

    async fn after_turn(
        &self,
        ctx: &TurnCtx<'_>,
        outcome: &crate::TurnOutcome,
    ) -> Result<TurnDecision> {
        if self.all_noop {
            return Ok(TurnDecision::Continue);
        }
        // First non-`Continue` decision wins (Stop short-circuits;
        // ContinueWith bubbles up immediately). LIFO so the most recently
        // added observer's vote takes precedence.
        let mut latest = TurnDecision::Continue;
        for h in self.layers.iter().rev() {
            match h.after_turn(ctx, outcome).await? {
                TurnDecision::Continue => {}
                d @ (TurnDecision::ContinueWith(_) | TurnDecision::Stop) => {
                    latest = d;
                    break;
                }
            }
        }
        Ok(latest)
    }

    async fn after_turn_failure(
        &self,
        ctx: &TurnCtx<'_>,
        outcome: &crate::TurnOutcome,
    ) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in self.layers.iter().rev() {
            h.after_turn_failure(ctx, outcome).await?;
        }
        Ok(())
    }

    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        if self.all_noop {
            return Ok(HookDecision::Allow);
        }
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
                is_read_only: ctx.is_read_only,
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
        if self.all_noop {
            return Ok(());
        }
        for h in self.layers.iter().rev() {
            h.after_tool(ctx, result).await?;
        }
        Ok(())
    }

    async fn session_start(&self, ctx: &SessionCtx<'_>) -> Result<SessionStartOutcome> {
        if self.all_noop {
            return Ok(SessionStartOutcome::default());
        }
        let mut merged = SessionStartOutcome::default();
        for h in &self.layers {
            let outcome = h.session_start(ctx).await?;
            merged.additional_context.extend(outcome.additional_context);
        }
        Ok(merged)
    }

    async fn session_end(&self, ctx: &SessionCtx<'_>, outcome: &SessionOutcome) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in self.layers.iter().rev() {
            h.session_end(ctx, outcome).await?;
        }
        Ok(())
    }

    async fn user_prompt_submit(&self, ctx: &PromptCtx<'_>) -> Result<HookDecision> {
        if self.all_noop {
            return Ok(HookDecision::Allow);
        }
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
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.pre_compact(ctx).await?;
        }
        Ok(())
    }

    async fn post_compact(&self, ctx: &CompactCtx<'_>, outcome: &CompactOutcome) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in self.layers.iter().rev() {
            h.post_compact(ctx, outcome).await?;
        }
        Ok(())
    }

    async fn config_change(&self, ctx: &ConfigChangeCtx<'_>) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.config_change(ctx).await?;
        }
        Ok(())
    }

    async fn cwd_changed(&self, ctx: &CwdChangedCtx<'_>) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.cwd_changed(ctx).await?;
        }
        Ok(())
    }

    async fn file_changed(&self, ctx: &FileChangedCtx<'_>) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.file_changed(ctx).await?;
        }
        Ok(())
    }

    async fn permission_request(&self, ctx: &PermCtx<'_>) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.permission_request(ctx).await?;
        }
        Ok(())
    }

    async fn permission_denied(&self, ctx: &PermCtx<'_>) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.permission_denied(ctx).await?;
        }
        Ok(())
    }

    async fn notification(&self, ctx: &NotificationCtx<'_>) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.notification(ctx).await?;
        }
        Ok(())
    }

    async fn subagent_start(&self, ctx: &SubagentCtx<'_>) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.subagent_start(ctx).await?;
        }
        Ok(())
    }

    async fn subagent_stop(&self, ctx: &SubagentCtx<'_>, outcome: &SubagentOutcome) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in self.layers.iter().rev() {
            h.subagent_stop(ctx, outcome).await?;
        }
        Ok(())
    }

    async fn task_created(&self, ctx: &TaskCtx<'_>) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.task_created(ctx).await?;
        }
        Ok(())
    }

    async fn task_completed(&self, ctx: &TaskCtx<'_>, outcome: &TaskOutcome) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn composite_session_start_concatenates_context_in_order() {
        struct CtxHook(&'static str);
        #[async_trait]
        impl Hooks for CtxHook {
            async fn session_start(&self, _ctx: &SessionCtx<'_>) -> Result<SessionStartOutcome> {
                Ok(SessionStartOutcome {
                    additional_context: vec![self.0.to_string()],
                })
            }
        }
        let composite = CompositeHooks::new(vec![
            std::sync::Arc::new(CtxHook("first")) as std::sync::Arc<dyn Hooks>,
            std::sync::Arc::new(CtxHook("second")) as std::sync::Arc<dyn Hooks>,
        ]);
        let cwd = std::path::Path::new(".");
        let ctx = SessionCtx {
            session_id: "t",
            cwd,
            provider: "test",
            model: "m",
        };
        let out = composite.session_start(&ctx).await.unwrap();
        assert_eq!(
            out.additional_context,
            vec!["first".to_string(), "second".to_string()]
        );
    }
}
