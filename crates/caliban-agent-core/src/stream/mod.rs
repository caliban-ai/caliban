//! Core agent loop driver: `stream_until_done`, `TurnEvent`, and related types.
//!
//! This module orchestrates the multi-turn provider/tool loop and re-exports
//! the public surface consumed by binaries (`caliban-agent-core::stream::*`).
//!
//! Internal layout:
//! - [`turn`] — per-turn accumulator state and TTFT/TBT timing helpers.
//! - [`parallel`] — types backing the parallel tool-dispatch phase.
//! - [`hook_dispatch`] — fan-out helpers for `Hooks` (single-tool dispatch
//!   wrapper including `UpdatedInput` threading).

mod hook_dispatch;
mod parallel;
mod recovery;
mod turn;

/// Maximum number of `TurnDecision::ContinueWith` injections per run.
///
/// `after_turn` hooks can ask the loop to take another turn with injected
/// user messages; this cap prevents death-spirals where a hook unconditionally
/// requests continuation.
pub const MAX_FORCED_CONTINUATIONS: u8 = 3;

/// Best-effort validation of a hook-rewritten tool input (`UpdatedInput`)
/// against the tool's declared JSON Schema. ADR-0024 requires the rewrite be
/// validated and a failure be a hard deny (#185 H6). Full JSON Schema
/// validation is intentionally out of scope (agent-core avoids the `jsonschema`
/// dep, as the headless validator does): we check the input is a JSON object
/// and that every `required` property is present — the realistic failure modes
/// for a malformed rewrite.
fn updated_input_is_valid(
    schema: &serde_json::Value,
    input: &serde_json::Value,
) -> std::result::Result<(), String> {
    let Some(obj) = input.as_object() else {
        return Err("rewritten input is not a JSON object".to_string());
    };
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for r in required {
            if let Some(name) = r.as_str()
                && !obj.contains_key(name)
            {
                return Err(format!("missing required field `{name}`"));
            }
        }
    }
    Ok(())
}

use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_stream::try_stream;
use async_trait::async_trait;
use caliban_provider::{
    CompletionRequest, ContentBlock, Message, RequestMetadata, Role, StopReason, StreamEvent,
    StreamingContentType, StreamingDelta, TextBlock, ToolResultBlock, Usage,
};
use futures::StreamExt as _;
use futures::stream::{FuturesUnordered, Stream};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use crate::agent::Agent;
use crate::error::Result;
use crate::hooks::{
    CompactCtx, CompactOutcome, HookDecision, RunCtx, RunHookOutcome, ToolCtx, TurnCtx,
    TurnDecision,
};
use crate::retry::with_retry;
use crate::tool::ToolError;

use hook_dispatch::dispatch_tool;
use parallel::DispatchPlan;
use turn::{ActiveBlock, MessageAccumulator, TurnTiming};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A high-level event emitted by [`Agent::stream_until_done`].
///
/// Consumers can forward these directly to a TUI/CLI renderer.
///
/// Serialized with an internal `"type"` tag so each NDJSON record is flat
/// and self-describing (the worker writes these to `stdout.ndjson`, and the
/// `agents attach` client reads them back). See #78.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TurnEvent {
    /// A new provider turn has started.
    TurnStart {
        /// Zero-based turn index.
        turn_index: u32,
        /// Provider-assigned message ID.
        message_id: String,
        /// Model that is responding.
        model: String,
    },
    /// An incremental text delta from the assistant.
    AssistantTextDelta {
        /// Zero-based turn index.
        turn_index: u32,
        /// Zero-based content-block index within the message.
        content_block_index: u32,
        /// Incremental text fragment.
        text: String,
    },
    /// An incremental thinking delta from the assistant.
    AssistantThinkingDelta {
        /// Zero-based turn index.
        turn_index: u32,
        /// Zero-based content-block index within the message.
        content_block_index: u32,
        /// Incremental thinking-text fragment.
        text: String,
    },
    /// A tool-use block has opened; the model is calling a tool.
    ToolCallStart {
        /// Zero-based turn index.
        turn_index: u32,
        /// Provider-assigned tool-use ID.
        tool_use_id: String,
        /// Name of the tool being called.
        name: String,
    },
    /// An incremental JSON fragment for a tool's input.
    ToolCallInputDelta {
        /// Zero-based turn index.
        turn_index: u32,
        /// Provider-assigned tool-use ID.
        tool_use_id: String,
        /// Partial JSON fragment.
        partial_json: String,
    },
    /// A tool invocation has completed (or been denied / errored).
    ToolCallEnd {
        /// Zero-based turn index.
        turn_index: u32,
        /// Provider-assigned tool-use ID.
        tool_use_id: String,
        /// Whether the result is an error.
        is_error: bool,
        /// Content blocks returned by the tool (or the error message).
        content: Vec<ContentBlock>,
    },
    /// The turn is complete — assistant message + any tool results are ready.
    TurnEnd {
        /// Zero-based turn index.
        turn_index: u32,
        /// The reconstructed assistant message for this turn.
        assistant_message: Message,
        /// Zero or one user message containing `ToolResult` blocks.
        tool_results: Vec<Message>,
        /// Why the model stopped.
        stop_reason: StopReason,
        /// Token usage for this turn.
        usage: Usage,
        /// Time-to-first-token for this turn. `None` when no deltas arrived.
        ttft: Option<Duration>,
        /// Mean time-between-tokens (across all deltas for this turn).
        /// `None` when fewer than two deltas arrived.
        tbt: Option<Duration>,
    },
    /// The entire run is complete.
    RunEnd {
        /// Full conversation history including initial messages, all assistant
        /// messages, and all tool-result messages.
        final_messages: Vec<Message>,
        /// Total token usage across all turns.
        total_usage: Usage,
        /// Number of turns executed.
        turn_count: u32,
        /// Why the run terminated.
        stopped_for: StopCondition,
        /// High-water mark of consecutive turns observed without a successful
        /// edit-class (non-read-only) tool call (#239).
        #[serde(default)]
        turns_without_edit: u32,
        /// Whether the no-edit-progress nudge fired at least once this run (#239).
        #[serde(default)]
        no_edit_nudge_emitted: bool,
    },
}

/// Outcome of a single agent turn (returned by `run_turn`).
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    /// The reconstructed assistant message.
    pub assistant_message: Message,
    /// Zero or one user message containing `ToolResult` blocks.
    pub tool_results: Vec<Message>,
    /// Why the model stopped this turn.
    pub stop_reason: StopReason,
    /// Token usage for this turn.
    pub usage: Usage,
    /// `true` iff `stop_reason == ToolUse` (i.e. the loop should continue).
    pub continue_loop: bool,
}

/// Outcome of a complete multi-turn run.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// Full conversation history including all assistant and tool-result messages.
    pub final_messages: Vec<Message>,
    /// Number of turns executed.
    pub turn_count: u32,
    /// Total token usage across all turns.
    pub total_usage: Usage,
    /// Why the run terminated.
    pub stopped_for: StopCondition,
    /// High-water mark of consecutive turns observed without a successful
    /// edit-class (non-read-only) tool call (#239). Consumed by the headless
    /// driver for progress telemetry.
    pub turns_without_edit: u32,
    /// Whether the no-edit-progress nudge fired at least once this run (#239).
    pub no_edit_nudge_emitted: bool,
}

/// The reason a multi-turn run stopped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StopCondition {
    /// The model reached a natural end-of-turn (`stop_reason: EndTurn`).
    EndOfTurn,
    /// The configured `max_turns` limit was reached.
    MaxTurnsReached(u32),
    /// The caller cancelled via a [`CancellationToken`].
    Cancelled,
    /// A provider error terminated the run (message included for display).
    ProviderError(String),
    /// A hook returned an error that terminated the run.
    HookDenied(String),
    /// Context compaction failed and the run cannot continue.
    CompactionFailed(String),
    /// `MaxTokens` hit and Stage A + Stage B recovery both surrendered.
    MaxTokensExhausted,
    /// `stop_reason: Refusal` from the provider; synthetic message already in `final_messages`.
    Refusal(String),
    /// `stop_reason: ContentFilter` from the provider; synthetic message already in `final_messages`.
    ContentFilter(String),
    /// SSE/HTTP stream went silent past the idle timeout.
    StreamIdle(std::time::Duration),
}

impl StopCondition {
    /// True for stop conditions that indicate failure, not natural completion.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::ProviderError(_)
                | Self::HookDenied(_)
                | Self::CompactionFailed(_)
                | Self::MaxTokensExhausted
                | Self::Refusal(_)
                | Self::ContentFilter(_)
                | Self::StreamIdle(_)
        )
    }

    /// The canonical user-facing surface — one framed `[caliban: …]` line plus
    /// a severity — for a non-`EndOfTurn` stop, or `None` for the natural
    /// `EndOfTurn`.
    ///
    /// Single source of truth for the drivers that report *why* a run ended:
    /// the TUI (line + `level`→color), the single-prompt CLI (line on stderr),
    /// and headless text mode. These had drifted into three separate copies of
    /// this mapping with divergent wording (#154) — notably `MaxTokensExhausted`
    /// and `StreamIdle`.
    #[must_use]
    pub fn surface(&self) -> Option<StopSurface> {
        let (body, level) = match self {
            Self::EndOfTurn => return None,
            Self::ProviderError(msg) => (format!("provider error: {msg}"), StopLevel::Error),
            Self::HookDenied(msg) => (format!("hook denied: {msg}"), StopLevel::Error),
            Self::CompactionFailed(msg) => (format!("compaction failed: {msg}"), StopLevel::Error),
            Self::MaxTurnsReached(n) => (format!("max-turns ({n}) reached"), StopLevel::Info),
            Self::Cancelled => ("cancelled".to_string(), StopLevel::Info),
            Self::MaxTokensExhausted => (
                "max-tokens recovery exhausted \u{2014} try /effort low to reduce reasoning budget"
                    .to_string(),
                StopLevel::Error,
            ),
            Self::Refusal(msg) => (format!("model refusal: {msg}"), StopLevel::Error),
            Self::ContentFilter(msg) => (format!("content filter: {msg}"), StopLevel::Error),
            Self::StreamIdle(d) => (
                format!("stream idle for {}s", d.as_secs()),
                StopLevel::Error,
            ),
        };
        Some(StopSurface {
            line: format!("[caliban: {body}]"),
            level,
        })
    }
}

/// Severity of a [`StopSurface`] line — drives whether a front end renders it
/// as an error (red transcript line / toast / stderr) or a neutral info line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopLevel {
    /// A failure: provider error, hook denial, compaction failure, refusal,
    /// content filter, max-tokens exhaustion, or stream-idle timeout.
    Error,
    /// A non-failure stop: max-turns reached or caller cancellation.
    Info,
}

/// One framed, user-facing line describing a non-`EndOfTurn` [`StopCondition`],
/// plus its severity. Produced by [`StopCondition::surface`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopSurface {
    /// The message, framed `[caliban: …]`.
    pub line: String,
    /// Whether to render as an error or a neutral info line.
    pub level: StopLevel,
}

#[cfg(test)]
mod updated_input_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_rewrite_passes() {
        // #185 H6.
        let schema = json!({"type": "object", "required": ["path"]});
        assert!(updated_input_is_valid(&schema, &json!({"path": "/x", "extra": 1})).is_ok());
    }

    #[test]
    fn non_object_rewrite_is_rejected() {
        let schema = json!({"type": "object"});
        assert!(updated_input_is_valid(&schema, &json!("not an object")).is_err());
    }

    #[test]
    fn missing_required_field_is_rejected() {
        let schema = json!({"type": "object", "required": ["path"]});
        let err = updated_input_is_valid(&schema, &json!({"other": 1})).unwrap_err();
        assert!(err.contains("required field `path`"), "got: {err}");
    }
}

#[cfg(test)]
mod stop_condition_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn is_failure_classifies_correctly() {
        assert!(!StopCondition::EndOfTurn.is_failure());
        assert!(!StopCondition::MaxTurnsReached(5).is_failure());
        assert!(!StopCondition::Cancelled.is_failure());
        assert!(StopCondition::ProviderError("x".into()).is_failure());
        assert!(StopCondition::HookDenied("x".into()).is_failure());
        assert!(StopCondition::CompactionFailed("x".into()).is_failure());
        assert!(StopCondition::MaxTokensExhausted.is_failure());
        assert!(StopCondition::Refusal("x".into()).is_failure());
        assert!(StopCondition::ContentFilter("x".into()).is_failure());
        assert!(StopCondition::StreamIdle(Duration::from_secs(90)).is_failure());
    }
}

#[cfg(test)]
mod serde_tests {
    use super::*;
    use caliban_provider::{Message, StopReason, Usage};
    use std::time::Duration;

    /// Round-trip a `TurnEvent` through JSON: serialize → deserialize →
    /// re-serialize, asserting the JSON is stable. Proves the #78 derives
    /// are correct (the per-event stream the worker writes to
    /// `stdout.ndjson` must survive a read back by the `agents attach`
    /// client). `TurnEvent` isn't `PartialEq`, so we compare JSON values.
    fn round_trips(ev: &TurnEvent) -> serde_json::Value {
        let json = serde_json::to_value(ev).expect("serialize TurnEvent");
        let back: TurnEvent = serde_json::from_value(json.clone()).expect("deserialize TurnEvent");
        let again = serde_json::to_value(&back).expect("re-serialize TurnEvent");
        assert_eq!(json, again, "round-trip JSON mismatch");
        json
    }

    #[test]
    fn text_delta_round_trips_with_type_tag() {
        let ev = TurnEvent::AssistantTextDelta {
            turn_index: 1,
            content_block_index: 0,
            text: "hello".into(),
        };
        let json = round_trips(&ev);
        // `#[serde(tag = "type")]` flattens the discriminant into the object.
        assert_eq!(json["type"], "AssistantTextDelta");
        assert_eq!(json["text"], "hello");
        assert_eq!(json["turn_index"], 1);
    }

    #[test]
    fn run_end_round_trips_carrying_a_stop_condition() {
        let ev = TurnEvent::RunEnd {
            final_messages: vec![],
            total_usage: Usage::default(),
            turn_count: 3,
            stopped_for: StopCondition::MaxTurnsReached(3),
            turns_without_edit: 0,
            no_edit_nudge_emitted: false,
        };
        let json = round_trips(&ev);
        assert_eq!(json["type"], "RunEnd");
        assert_eq!(json["turn_count"], 3);
        // StopCondition has tuple/unit variants, so it stays externally
        // tagged: MaxTurnsReached(3) → {"MaxTurnsReached": 3}.
        assert_eq!(json["stopped_for"]["MaxTurnsReached"], 3);
    }

    #[test]
    fn turn_end_round_trips_with_message_and_duration_shape() {
        // Pins down the `Option<Duration>` wire shape the #79 attach client
        // must handle: serde encodes `Duration` as `{secs, nanos}`, and
        // `None` as JSON null. Also exercises a nested provider `Message`.
        let ev = TurnEvent::TurnEnd {
            turn_index: 0,
            assistant_message: Message::assistant_text("done"),
            tool_results: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            ttft: Some(Duration::from_millis(123)),
            tbt: None,
        };
        let json = round_trips(&ev);
        assert_eq!(json["type"], "TurnEnd");
        assert_eq!(json["stop_reason"], "end_turn");
        // Duration → {"secs":0,"nanos":123000000}; None → null.
        assert_eq!(json["ttft"]["secs"], 0);
        assert_eq!(json["ttft"]["nanos"], 123_000_000);
        assert!(json["tbt"].is_null());
    }

    #[test]
    fn unit_stop_condition_serializes_as_bare_string() {
        let json = serde_json::to_value(StopCondition::EndOfTurn).unwrap();
        assert_eq!(json, serde_json::json!("EndOfTurn"));
        let back: StopCondition = serde_json::from_value(json).unwrap();
        assert!(matches!(back, StopCondition::EndOfTurn));
    }
}

/// Boxed, pinned stream of `TurnEvent` results.
pub type TurnEventStream = Pin<Box<dyn Stream<Item = Result<TurnEvent>> + Send + 'static>>;

/// Supplies additional user input to a run that would otherwise end naturally
/// (ADR 0047 / #81).
///
/// When a run has an `InputProvider`, the loop awaits it at the end-of-run
/// boundary instead of terminating: `Some` resumes with the returned messages
/// (uncapped — a human drives it), `None` ends the run. Foreground/headless
/// runs pass no provider and are unaffected.
#[async_trait]
pub trait InputProvider: Send + Sync {
    /// Await the next user messages to inject, or `None` to end the run.
    ///
    /// Implementations should select on `cancel` and return promptly when
    /// it fires.
    async fn next_input(
        &self,
        cancel: &CancellationToken,
    ) -> Option<Vec<caliban_provider::Message>>;
}

/// Optional per-run identity that drives [`crate::hooks::Hooks::before_run`]
/// / [`crate::hooks::Hooks::after_run`] (ADR 0028).
///
/// Callers that care about checkpointing pass this via
/// [`Agent::stream_until_done_with_settings`]. The legacy [`Agent::stream_until_done`]
/// passes [`RunSettings::default()`], which fires the lifecycle events with
/// an empty `session_id` and the cwd as the workspace root.
#[derive(Clone)]
pub struct RunSettings {
    /// Opaque session identifier; surfaced in `RunCtx.session_id`.
    pub session_id: String,
    /// Workspace root; surfaced in `RunCtx.workspace_root`. Defaults to `.`.
    pub workspace_root: std::path::PathBuf,
    /// Monotonic prompt index within the parent session; defaults to 0.
    pub prompt_index: u32,
    /// Optional interactive input source (ADR 0047 / #81). When `Some`, the
    /// loop awaits it at the natural end-of-run boundary instead of ending.
    /// `None` (default) preserves run-to-completion behavior exactly.
    pub input_source: Option<std::sync::Arc<dyn InputProvider>>,
}

impl std::fmt::Debug for RunSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunSettings")
            .field("session_id", &self.session_id)
            .field("workspace_root", &self.workspace_root)
            .field("prompt_index", &self.prompt_index)
            .field(
                "input_source",
                if self.input_source.is_some() {
                    &"<set>"
                } else {
                    &"<unset>"
                },
            )
            .finish()
    }
}

impl Default for RunSettings {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            workspace_root: std::path::PathBuf::from("."),
            prompt_index: 0,
            input_source: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Agent::stream_until_done
// ---------------------------------------------------------------------------

impl Agent {
    /// Run the agent loop, streaming high-level [`TurnEvent`]s.
    ///
    /// The returned stream drives the entire multi-turn conversation:
    /// provider calls, tool dispatch, retry, compaction, and hook invocations.
    /// The stream always ends with a [`TurnEvent::RunEnd`] event.
    ///
    /// # Errors
    ///
    /// Each stream item is `Result<TurnEvent>`. Fatal errors cause a
    /// `RunEnd` with the appropriate [`StopCondition`] before the stream closes.
    ///
    /// # Panics
    ///
    /// Cannot panic in practice. The `acquire_owned` `.expect` is unreachable
    /// because the dispatch semaphore is owned by the same task and not closed
    /// until after the futures complete.
    pub fn stream_until_done(
        self: Arc<Self>,
        messages: Vec<Message>,
        cancel: CancellationToken,
    ) -> TurnEventStream {
        self.stream_until_done_with_settings(messages, cancel, RunSettings::default())
    }

    /// Like [`Agent::stream_until_done`] but carries a [`RunSettings`] so the
    /// `before_run` / `after_run` hooks (ADR 0028) receive a meaningful
    /// session identity. Used by the caliban binary's TUI / headless front-ends.
    ///
    /// # Panics
    ///
    /// Cannot panic in practice; see [`Agent::stream_until_done`] for the
    /// detailed safety note.
    #[allow(clippy::too_many_lines)]
    #[instrument(
        skip(self, messages, cancel, settings),
        fields(model = %self.active_model(), session = %settings.session_id, prompt = settings.prompt_index)
    )]
    pub fn stream_until_done_with_settings(
        self: Arc<Self>,
        messages: Vec<Message>,
        cancel: CancellationToken,
        settings: RunSettings,
    ) -> TurnEventStream {
        Box::pin(try_stream! {
            let mut history = messages;
            let mut total_usage = Usage::default();
            // ---- before_run hook (ADR 0028) ----
            // Capture the most recent user message (best-effort) for the
            // run context. Used by caliban-checkpoint to label the prompt.
            let user_msg_owned: Option<Message> = history
                .iter()
                .rev()
                .find(|m| m.role == Role::User)
                .cloned();
            {
                let run_ctx = RunCtx {
                    session_id: &settings.session_id,
                    workspace_root: &settings.workspace_root,
                    user_message: user_msg_owned.as_ref(),
                    prompt_index: settings.prompt_index,
                    cancel: cancel.clone(),
                };
                if let Err(e) = self.hooks.before_run(&run_ctx).await {
                    // Surface as a RunEnd terminating the stream cleanly.
                    yield TurnEvent::RunEnd {
                        final_messages: history,
                        total_usage,
                        turn_count: 0,
                        stopped_for: StopCondition::HookDenied(format!("before_run: {e}")),
                        turns_without_edit: 0,
                        no_edit_nudge_emitted: false,
                    };
                    return;
                }
            }
            // Initialise to MaxTurnsReached; overridden on any natural stop or error.
            let mut stopped_for = StopCondition::MaxTurnsReached(self.config.max_turns);
            let max_turns = self.config.max_turns;
            let mut turns_completed: u32 = 0;

            // ---- Per-run state (Plan A recovery + Plan B autocompact) ----
            // Plan A — recovery state (Tasks 4–7 + 11):
            // Stage A budget escalation: tracks whether we already retried THIS
            // turn with `escalated_max_tokens`. Reset on every fresh turn.
            let mut stage_a_attempted_this_turn = false;
            // Per-turn override for `max_tokens` on the next request build. None
            // means "use `self.config.max_tokens`".
            let mut override_max_tokens_for_request: Option<u32> = None;
            // Stage B meta-continuation count (per-run, not per-turn).
            let mut meta_continuation_count: u8 = 0;
            // One-shot reactive compaction guard (Task 7).
            let mut attempted_reactive_compact = false;
            // T11: cap on `TurnDecision::ContinueWith` injections per run.
            let mut forced_continuations: u8 = 0;

            // Plan B — autocompact tracking (per-run):
            const MAX_CONSECUTIVE_COMPACT_FAILURES: u8 = 2;
            #[derive(Debug, Default)]
            struct AutoCompactTracking {
                consecutive_failures: u8,
                disabled: bool,
            }
            let mut auto_tracking = AutoCompactTracking::default();

            // #239 — no-edit-progress tracking (per-run):
            // `turns_since_last_edit` counts consecutive completed turns with
            // no successful non-read-only tool call; it resets to 0 (re-arming
            // the nudge) the moment one succeeds. `turns_without_edit` is the
            // high-water mark reported on RunOutcome/RunEnd. `no_edit_nudge_*`
            // ensure at most one nudge per no-edit streak.
            let mut turns_since_last_edit: u32 = 0;
            let mut turns_without_edit: u32 = 0;
            let mut no_edit_nudge_armed = true;
            let mut no_edit_nudge_emitted = false;

            'outer: for turn_index in 0..max_turns {
                // The inner loop lets recovery flows (Stage A budget
                // escalation, reactive compaction) re-enter the turn body
                // without consuming a slot from the outer turn counter. The
                // body breaks `'inner` once it has produced a turn outcome;
                // it `continue 'inner` to redo the turn with adjusted state.
                'inner: loop {
                // ---- Cancellation check ----
                if cancel.is_cancelled() {
                    stopped_for = StopCondition::Cancelled;
                    break 'outer;
                }

                // ---- before_turn hook ----
                {
                    let turn_ctx = TurnCtx {
                        turn_index,
                        messages: &history,
                        config: &self.config,
                    };
                    if let Err(e) = self.hooks.before_turn(&turn_ctx).await {
                        stopped_for = StopCondition::HookDenied(format!("before_turn: {e}"));
                        break 'outer;
                    }
                }

                // ---- Microcompact (per-turn, LLM-free) ----
                if self.config.micro_compact_enabled {
                    use crate::compact::Compactor as _;
                    let caps = self.provider.capabilities(&self.config.model);
                    if let Ok(Some(new)) = crate::compact::MicroCompactor::new()
                        .compact(&history, &caps)
                        .await
                    {
                        let freed = crate::compact::estimate_tokens(&history)
                            .saturating_sub(crate::compact::estimate_tokens(&new));
                        tracing::debug!(
                            target: "caliban::compact",
                            freed_tokens = freed,
                            "microcompact",
                        );
                        history = new;
                    }
                }

                // ---- Compaction (threshold-gated autocompact) ----
                {
                    let active_model_snapshot = self.active_model();
                    let caps = self.provider.capabilities(active_model_snapshot.as_str());
                    let token_count_before = crate::compact::estimate_tokens(&history);
                    let threshold = self.config.auto_compact_threshold;
                    let should_attempt = threshold.is_some_and(|t| {
                        #[allow(
                            clippy::cast_precision_loss,
                            clippy::cast_possible_truncation,
                            clippy::cast_sign_loss
                        )]
                        let utilization =
                            token_count_before as f32 / caps.max_input_tokens.max(1) as f32;
                        !auto_tracking.disabled && utilization >= t
                    });
                    if should_attempt {
                        let strategy = self.compactor.strategy_name();
                        let compact_ctx = CompactCtx {
                            session_id: "",
                            token_count_before,
                            strategy,
                        };
                        if let Err(e) = self.hooks.pre_compact(&compact_ctx).await {
                            tracing::warn!(error = %e, "pre_compact hook error (non-fatal)");
                        }
                        match self.compactor.compact(&history, &caps).await {
                            Err(e) => {
                                tracing::warn!(error = %e, "autocompact failed");
                                auto_tracking.consecutive_failures =
                                    auto_tracking.consecutive_failures.saturating_add(1);
                                if auto_tracking.consecutive_failures
                                    >= MAX_CONSECUTIVE_COMPACT_FAILURES
                                {
                                    auto_tracking.disabled = true;
                                    tracing::warn!(
                                        "autocompact disabled after {MAX_CONSECUTIVE_COMPACT_FAILURES} consecutive failures"
                                    );
                                }
                            }
                            Ok(Some(new)) => {
                                auto_tracking.consecutive_failures = 0;
                                let token_count_after = crate::compact::estimate_tokens(&new);
                                history = new;
                                let outcome = CompactOutcome {
                                    token_count_after,
                                    compacted: true,
                                };
                                if let Err(e) =
                                    self.hooks.post_compact(&compact_ctx, &outcome).await
                                {
                                    tracing::warn!(
                                        error = %e,
                                        "post_compact hook error (non-fatal)"
                                    );
                                }
                            }
                            Ok(None) => {
                                auto_tracking.consecutive_failures = 0;
                                let outcome = CompactOutcome {
                                    token_count_after: token_count_before,
                                    compacted: false,
                                };
                                if let Err(e) =
                                    self.hooks.post_compact(&compact_ctx, &outcome).await
                                {
                                    tracing::warn!(
                                        error = %e,
                                        "post_compact hook error (non-fatal)"
                                    );
                                }
                            }
                        }
                    }
                }

                // ---- Build completion request ----
                let mut req_messages = history.clone();
                // ADR-0046: apply the per-turn MCP wire filter. When
                // `config.lazy_mcp` is false this is a passthrough.
                let active_guard = self.mcp_active.load();
                let filter = crate::wire_filter::WireFilter {
                    lazy_mcp: self.config.lazy_mcp,
                    active: &active_guard,
                    eager_servers: &self.mcp_eager_servers,
                };
                let crate::wire_filter::WireFilterResult {
                    tools: mut req_tools,
                    dropped_mcp_count,
                } = self.tools.to_caliban_tools_filtered(&filter);
                // Splice the deferred-block paragraph into the system
                // message when lazy mode is active and the filter dropped
                // at least one MCP tool (ADR-0046).
                crate::deferred_block::splice_into_messages(
                    &mut req_messages,
                    self.config.lazy_mcp,
                    dropped_mcp_count,
                );
                if self.prompt_cache {
                    crate::cache::apply_prompt_cache(
                        &mut req_messages,
                        &mut req_tools,
                        self.config.min_cache_block_tokens,
                    );
                }
                // Plan A: Stage A override → escalated max_tokens for retry.
                let effective_max_tokens =
                    override_max_tokens_for_request.unwrap_or(self.config.max_tokens);
                // Plan C: snapshot the swappable effort level once per turn
                // so the in-flight request sees a single coherent value even
                // if `/effort` lands between turns.
                let effort_snapshot = self.config.effort.load_full();
                // #100: likewise snapshot the swappable extended-thinking
                // control so a `/think` change between turns applies as one
                // coherent value to the in-flight request.
                let thinking_snapshot = self.config.thinking.load_full();
                // Plan C: likewise for the model id — a `/model` swap that
                // lands between request build and provider call must not
                // split model + capabilities + effort across two ids.
                let active_model_for_req = self.active_model();
                let req = CompletionRequest {
                    model: active_model_for_req.as_str().to_string(),
                    messages: req_messages,
                    tools: req_tools,
                    tool_choice: self.config.tool_choice.clone(),
                    max_tokens: effective_max_tokens,
                    temperature: self.config.temperature,
                    top_p: self.config.top_p,
                    top_k: None,
                    stop_sequences: self.config.stop_sequences.clone(),
                    thinking: *thinking_snapshot,
                    effort: Some(*effort_snapshot),
                    metadata: RequestMetadata {
                        user_id: self.config.user_id.clone(),
                        purpose: Some(caliban_provider::RequestPurpose::MainLoop),
                    },
                };

                // ---- Stream from provider (with retry) ----
                // Begin per-turn timing here so TTFT reflects user-observed
                // latency including any backoff sleeps (typically 0 on the
                // first attempt).
                let mut timing = TurnTiming::start();
                let provider = Arc::clone(&self.provider);
                let req_clone = req.clone();
                let cancel_for_retry = cancel.clone();
                let stream_result = with_retry(&self.retry, &cancel, move || {
                    let p = Arc::clone(&provider);
                    let r = req_clone.clone();
                    let _ = &cancel_for_retry; // ensure the clone is moved
                    async move { p.stream(r).await }
                })
                .await;

                let provider_stream = match stream_result {
                    Ok(s) => s,
                    Err(e) => match e {
                        caliban_provider::Error::Cancelled => {
                            stopped_for = StopCondition::Cancelled;
                            break 'outer;
                        }
                        caliban_provider::Error::StreamIdle(d) => {
                            stopped_for = StopCondition::StreamIdle(d);
                            break 'outer;
                        }
                        caliban_provider::Error::ContextTooLong { .. }
                            if !attempted_reactive_compact =>
                        {
                            tracing::warn!(
                                target: "caliban::recovery",
                                "recovery.reactive_compact.fired"
                            );
                            attempted_reactive_compact = true;
                            let caps = self.provider.capabilities(&self.config.model);
                            if let Ok(Some(new)) =
                                self.compactor.compact(&history, &caps).await
                            {
                                history = new;
                                // Redo this turn with the compacted
                                // history; don't consume a turn slot.
                                continue 'inner;
                            }
                            stopped_for = StopCondition::ProviderError(
                                "context too long; compactor declined".into(),
                            );
                            break 'outer;
                        }
                        other => {
                            stopped_for = StopCondition::ProviderError(other.to_string());
                            break 'outer;
                        }
                    },
                };

                // ---- Drain provider stream ----
                //
                // Wrap the provider's `MessageStream` in `WatchedStream` when
                // an idle timeout is configured (>0). The watchdog yields
                // `Err(Error::StreamIdle(d))` when no chunk arrives within
                // `stream_idle_timeout_ms`, which the per-event error arm
                // below maps to `StopCondition::StreamIdle(d)`. The wrapper
                // is a no-op on the hot path (single comparison per chunk).
                let mut provider_stream: Pin<
                    Box<dyn Stream<Item = caliban_provider::Result<StreamEvent>> + Send>,
                > = if self.config.stream_idle_timeout_ms > 0 {
                    Box::pin(caliban_provider::stream::WatchedStream::new(
                        provider_stream,
                        Duration::from_millis(self.config.stream_idle_timeout_ms.into()),
                    ))
                } else {
                    provider_stream
                };
                let mut acc = MessageAccumulator::new();

                while let Some(evt_result) = provider_stream.next().await {
                    if cancel.is_cancelled() {
                        stopped_for = StopCondition::Cancelled;
                        break 'outer;
                    }
                    let evt = match evt_result {
                        Ok(e) => e,
                        Err(caliban_provider::Error::StreamIdle(d)) => {
                            stopped_for = StopCondition::StreamIdle(d);
                            break 'outer;
                        }
                        Err(e) => {
                            stopped_for = StopCondition::ProviderError(e.to_string());
                            break 'outer;
                        }
                    };

                    match evt {
                        StreamEvent::MessageStart { id, model } => {
                            acc.message_id.clone_from(&id);
                            acc.model.clone_from(&model);
                            yield TurnEvent::TurnStart {
                                turn_index,
                                message_id: id,
                                model,
                            };
                        }
                        StreamEvent::ContentBlockStart { index, content_type } => {
                            let i = index as usize;
                            acc.ensure_index(i);
                            match &content_type {
                                StreamingContentType::Text => {
                                    acc.active[i] =
                                        Some(ActiveBlock::Text { accumulated: String::new() });
                                }
                                StreamingContentType::Thinking => {
                                    acc.active[i] =
                                        Some(ActiveBlock::Thinking { accumulated: String::new() });
                                }
                                StreamingContentType::ToolUse { id, name } => {
                                    let id = id.clone();
                                    let name = name.clone();
                                    acc.active[i] = Some(ActiveBlock::ToolUse {
                                        id: id.clone(),
                                        name: name.clone(),
                                        json_buf: String::new(),
                                    });
                                    yield TurnEvent::ToolCallStart {
                                        turn_index,
                                        tool_use_id: id,
                                        name,
                                    };
                                }
                                StreamingContentType::Image => {
                                    // Image streaming blocks are not supported; leave placeholder.
                                }
                            }
                        }
                        StreamEvent::Delta { index, delta } => {
                            timing.observe_delta();
                            let i = index as usize;
                            if i >= acc.active.len() {
                                continue;
                            }
                            match (&mut acc.active[i], delta) {
                                (
                                    Some(ActiveBlock::Text { accumulated }),
                                    StreamingDelta::Text(s),
                                ) => {
                                    accumulated.push_str(&s);
                                    yield TurnEvent::AssistantTextDelta {
                                        turn_index,
                                        content_block_index: index,
                                        text: s,
                                    };
                                }
                                (
                                    Some(ActiveBlock::Thinking { accumulated }),
                                    StreamingDelta::Thinking(s),
                                ) => {
                                    accumulated.push_str(&s);
                                    yield TurnEvent::AssistantThinkingDelta {
                                        turn_index,
                                        content_block_index: index,
                                        text: s,
                                    };
                                }
                                (
                                    Some(ActiveBlock::ToolUse { id, json_buf, .. }),
                                    StreamingDelta::ToolUseInputJson(s),
                                ) => {
                                    json_buf.push_str(&s);
                                    let id_clone = id.clone();
                                    yield TurnEvent::ToolCallInputDelta {
                                        turn_index,
                                        tool_use_id: id_clone,
                                        partial_json: s,
                                    };
                                }
                                _ => {
                                    // Mismatched delta type — ignore gracefully.
                                }
                            }
                        }
                        StreamEvent::ContentBlockStop { index } => {
                            acc.finalize_block(index as usize);
                        }
                        StreamEvent::MessageDelta {
                            stop_reason,
                            usage_delta,
                        } => {
                            if let Some(sr) = stop_reason {
                                acc.stop_reason = Some(sr);
                            }
                            if let Some(u) = usage_delta {
                                acc.usage.merge(u);
                            }
                        }
                        StreamEvent::MessageStop | StreamEvent::Ping => {}
                    }
                }

                let turn_stop_reason = acc.stop_reason.unwrap_or(StopReason::EndTurn);
                let turn_usage = acc.usage;
                let mut assistant_message = acc.into_message();

                // ---- Plan A — Stage A: silent budget-escalation retry ----
                //
                // When recovery is enabled and the provider stopped on
                // `MaxTokens` (and we haven't already escalated this
                // turn), redo the request with `escalated_max_tokens`.
                // The retry is invisible to the consumer:
                //   - the truncated assistant_message is NOT pushed to
                //     history (it's a failed attempt);
                //   - tool-dispatch phases are skipped (the assistant
                //     may have produced partial tool_use blocks);
                //   - no `after_turn` hook fires;
                //   - no `TurnEnd` event is yielded;
                //   - `turns_completed` is NOT incremented.
                //
                // We do merge `turn_usage` so the user is still billed
                // for the failed attempt. Same shape as the reactive-
                // compact arm above (see line ~595).
                //
                // PR #68 disabled this feature because the previous
                // implementation ran AFTER yield/counter — hoisting it
                // here is the fix that lets us flip the default to
                // true.
                if self.config.max_tokens_recovery
                    && turn_stop_reason == StopReason::MaxTokens
                    && !stage_a_attempted_this_turn
                {
                    tracing::warn!(
                        target: "caliban::recovery",
                        from = self.config.max_tokens,
                        to = self.config.escalated_max_tokens,
                        "recovery.max_tokens.stage_a"
                    );
                    stage_a_attempted_this_turn = true;
                    override_max_tokens_for_request =
                        Some(self.config.escalated_max_tokens);
                    total_usage.merge(turn_usage);
                    continue 'inner;
                }

                // Apply the assistant-text post-processor (set via
                // `AgentBuilder::post_processor`; defaults to identity). The
                // canonical use today is the `Learning` output style, which
                // inserts `TODO(human)` markers at inflection points.
                for block in &mut assistant_message.content {
                    if let ContentBlock::Text(t) = block {
                        let processed = self.post_processor.process(&t.text);
                        if let std::borrow::Cow::Owned(new_text) = processed {
                            t.text = new_text;
                        }
                    }
                }

                // ---- Phase 1: plan (serial before_tool gate) ----
                let mut plans: Vec<DispatchPlan> = Vec::new();
                // #239: tool_use_ids of dispatched non-read-only ("edit-class")
                // calls this turn. After dispatch we check which of these
                // produced a *successful* result to decide whether the turn
                // counts as edit progress. Denied calls never enter this set
                // (they always error), so a denied edit can't reset the
                // no-edit counter.
                let mut non_read_only_tool_ids: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                for (idx, block) in assistant_message.content.iter().enumerate() {
                    if cancel.is_cancelled() {
                        stopped_for = StopCondition::Cancelled;
                        break 'outer;
                    }
                    let ContentBlock::ToolUse(tu) = block else { continue };

                    // Plan-mode gating: when active, reject tools that are
                    // neither side-effect-free (Tool::is_read_only) nor a
                    // plan-control tool, BEFORE running hooks (cheaper, and the
                    // rejection still goes back to the model as a normal
                    // ToolResult so it can adapt).
                    let tool_is_read_only =
                        self.tools.get(&tu.name).is_some_and(|t| t.is_read_only());
                    let plan_mode_active = self
                        .plan_mode
                        .as_ref()
                        .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed));
                    if plan_mode_active
                        && !(tool_is_read_only
                            || crate::plan_mode::is_plan_control_tool(&tu.name))
                    {
                        let msg = format!(
                            "Tool '{}' is not available in plan mode. Use ExitPlanMode to proceed.",
                            tu.name
                        );
                        let content = vec![ContentBlock::Text(TextBlock {
                            text: msg.clone(),
                            cache_control: None,
                        })];
                        let result = ToolResultBlock {
                            tool_use_id: tu.id.clone(),
                            content,
                            is_error: true,
                        };
                        yield TurnEvent::ToolCallEnd {
                            turn_index,
                            tool_use_id: result.tool_use_id.clone(),
                            is_error: true,
                            content: result.content.clone(),
                        };
                        plans.push(DispatchPlan::Denied {
                            original_index: idx,
                            result,
                        });
                        continue;
                    }

                    let tool_ctx = ToolCtx {
                        session_id: &settings.session_id,
                        turn_index,
                        tool_use_id: &tu.id,
                        tool_name: &tu.name,
                        input: &tu.input,
                        is_read_only: tool_is_read_only,
                    };
                    let decision = match self.hooks.before_tool(&tool_ctx).await {
                        Ok(d) => d,
                        Err(e) => {
                            stopped_for = StopCondition::HookDenied(
                                format!("before_tool hook failed: {e}"),
                            );
                            break 'outer;
                        }
                    };

                    // ADR-0024 / #185 H6: a hook may rewrite a tool's input via
                    // UpdatedInput, but the rewrite must be validated against
                    // the tool's schema; an invalid rewrite is a *hard deny*
                    // (not dispatched). Convert it to Deny here so it flows
                    // through the denial path below (after_tool notify + error
                    // result).
                    let decision = if let HookDecision::UpdatedInput(new_input) = &decision {
                        match self.tools.get(&tu.name) {
                            Some(tool) => {
                                match updated_input_is_valid(tool.input_schema(), new_input) {
                                    Ok(()) => decision,
                                    Err(why) => HookDecision::Deny(format!(
                                        "before_tool hook rewrote input to an invalid shape: {why}"
                                    )),
                                }
                            }
                            None => decision,
                        }
                    } else {
                        decision
                    };

                    match decision {
                        // AskDenied (a synthesized non-interactive Ask→Deny) is
                        // handled identically to Deny; the mode filter normally
                        // normalizes it away, but treat it as a denial defensively.
                        HookDecision::Deny(msg) | HookDecision::AskDenied(msg) => {
                            let content = vec![ContentBlock::Text(TextBlock {
                                text: format!("Tool call denied: {msg}"),
                                cache_control: None,
                            })];
                            // Mirror dispatch_tool: notify after_tool of the denial.
                            let denial_err = ToolError::execution(std::io::Error::other(
                                format!("denied: {msg}"),
                            ));
                            if let Err(e) =
                                self.hooks.after_tool(&tool_ctx, &Err(denial_err)).await
                            {
                                tracing::warn!(
                                    tool = %tu.name, error = %e,
                                    "after_tool hook error (non-fatal)"
                                );
                            }
                            let result = ToolResultBlock {
                                tool_use_id: tu.id.clone(),
                                content,
                                is_error: true,
                            };
                            // Emit the denied ToolCallEnd up front, in
                            // assistant-message order.
                            yield TurnEvent::ToolCallEnd {
                                turn_index,
                                tool_use_id: result.tool_use_id.clone(),
                                is_error: true,
                                content: result.content.clone(),
                            };
                            plans.push(DispatchPlan::Denied {
                                original_index: idx,
                                result,
                            });
                        }
                        HookDecision::Allow => {
                            if !tool_is_read_only {
                                non_read_only_tool_ids.insert(tu.id.clone());
                            }
                            let conflict_key = self
                                .tools
                                .get(&tu.name)
                                .and_then(|t| t.parallel_conflict_key(&tu.input));
                            plans.push(DispatchPlan::Allowed {
                                original_index: idx,
                                id: tu.id.clone(),
                                name: tu.name.clone(),
                                input: tu.input.clone(),
                                conflict_key,
                            });
                        }
                        HookDecision::UpdatedInput(new_input) => {
                            if !tool_is_read_only {
                                non_read_only_tool_ids.insert(tu.id.clone());
                            }
                            tracing::info!(
                                tool = %tu.name,
                                tool_use_id = %tu.id,
                                "hook.updated_input: tool input rewritten by before_tool hook"
                            );
                            let conflict_key = self
                                .tools
                                .get(&tu.name)
                                .and_then(|t| t.parallel_conflict_key(&new_input));
                            plans.push(DispatchPlan::Allowed {
                                original_index: idx,
                                id: tu.id.clone(),
                                name: tu.name.clone(),
                                input: new_input,
                                conflict_key,
                            });
                        }
                    }
                }

                // ---- Phase 2: dispatch (parallel invoke + after_tool) ----
                let permits = if self.parallel_tools {
                    self.parallel_tool_limit.get()
                } else {
                    1
                };
                let sem = Arc::new(Semaphore::new(permits));
                let dispatch_started_at = Instant::now();
                let agent_ref = &self;

                let mut ordered_results: Vec<Option<ToolResultBlock>> =
                    vec![None; assistant_message.content.len()];
                let mut denied_count: usize = 0;
                let mut dispatched_count: usize = 0;

                // Per-key serialization locks (ADR 0016 Revised 2026-05-26).
                // Calls with the same conflict_key acquire the same Mutex
                // before the semaphore, so two writes to the same target
                // serialize while different-target writes still parallelize.
                let conflict_locks = parallel::build_conflict_locks(&plans);
                let mut pending = FuturesUnordered::new();
                for plan in plans {
                    match plan {
                        DispatchPlan::Denied { original_index, result } => {
                            denied_count += 1;
                            ordered_results[original_index] = Some(result);
                        }
                        DispatchPlan::Allowed {
                            original_index,
                            id,
                            name,
                            input,
                            conflict_key,
                        } => {
                            if cancel.is_cancelled() {
                                stopped_for = StopCondition::Cancelled;
                                break;
                            }
                            dispatched_count += 1;
                            let sem_for_tool = Arc::clone(&sem);
                            let cancel_for_tool = cancel.clone();
                            let session_for_tool = settings.session_id.clone();
                            let lock_for_tool = conflict_key
                                .as_ref()
                                .and_then(|k| conflict_locks.get(k).map(Arc::clone));
                            pending.push(async move {
                                // Per-key serialization: when set, this future
                                // FIFO-blocks against other same-key futures.
                                // Lock held across the full dispatch so the
                                // next caller sees the side-effects of this
                                // one. Tokio Mutex is FIFO.
                                let _key_guard = match lock_for_tool {
                                    Some(m) => Some(m.lock_owned().await),
                                    None => None,
                                };
                                // Acquire inside the future so concurrent
                                // futures actually progress. (Pre-acquiring
                                // in the for-loop would deadlock when
                                // permits < plans because the next acquire
                                // would block on a future that hasn't been
                                // polled yet.)
                                let _permit = sem_for_tool
                                    .acquire_owned()
                                    .await
                                    .expect("semaphore not closed");
                                let res = dispatch_tool(
                                    agent_ref,
                                    &session_for_tool,
                                    turn_index,
                                    &id,
                                    &name,
                                    input,
                                    &cancel_for_tool,
                                )
                                .await;
                                (original_index, id, res)
                            });
                        }
                    }
                }

                // Drive the pending set. ToolCallEnd events fire in
                // completion order; ordered_results preserves history order.
                let mut fatal_stop: Option<StopCondition> = None;
                while let Some((idx, id, dispatch_res)) = pending.next().await {
                    match dispatch_res {
                        Err(stop) => {
                            fatal_stop = Some(stop);
                            // Continue draining the loop so no future escapes.
                        }
                        Ok(tool_result) => {
                            let is_error = tool_result.is_error;
                            let content = tool_result.content.clone();
                            yield TurnEvent::ToolCallEnd {
                                turn_index,
                                tool_use_id: id,
                                is_error,
                                content,
                            };
                            ordered_results[idx] = Some(tool_result);
                        }
                    }
                }

                let dispatch_elapsed = dispatch_started_at.elapsed();
                tracing::info!(
                    target: caliban_common::tracing_targets::TARGET_TOOLS,
                    turn = turn_index,
                    parallel_tools = self.parallel_tools,
                    parallel_tool_limit = self.parallel_tool_limit.get(),
                    dispatched = dispatched_count,
                    denied = denied_count,
                    total_wall_ms = u64::try_from(dispatch_elapsed.as_millis())
                        .unwrap_or(u64::MAX),
                    "parallel tool dispatch",
                );

                if let Some(stop) = fatal_stop {
                    stopped_for = stop;
                    break 'outer;
                }

                // ---- Phase 3: collect results in assistant-message order ----
                let mut tool_result_blocks: Vec<ContentBlock> = Vec::new();
                // #239: a turn counts as edit progress iff at least one
                // dispatched non-read-only tool call returned a *successful*
                // (non-error) result.
                let mut had_successful_edit_this_turn = false;
                for slot in ordered_results.into_iter().flatten() {
                    if !slot.is_error && non_read_only_tool_ids.contains(&slot.tool_use_id) {
                        had_successful_edit_this_turn = true;
                    }
                    tool_result_blocks.push(ContentBlock::ToolResult(slot));
                }

                // ---- Tool-result size cap (context-management spec) ----
                if self.config.tool_result_cap_chars > 0 && !tool_result_blocks.is_empty() {
                    let overflow_dir = directories::ProjectDirs::from("dev", "caliban", "caliban")
                        .map_or_else(
                            || std::path::PathBuf::from("/tmp/caliban-tool-overflows"),
                            |d| d.cache_dir().join("tool-overflows"),
                        );
                    let cap = crate::post_process::ToolResultCap {
                        max_chars: self.config.tool_result_cap_chars,
                        overflow_dir,
                        session_id: settings.session_id.clone(),
                    };
                    if let Err(e) = cap.cap(&mut tool_result_blocks).await {
                        tracing::warn!(
                            error = %e,
                            "ToolResultCap io error (non-fatal); inline content kept",
                        );
                    }
                }

                // Build the tool-results message (if any tools were called).
                let tool_results: Vec<Message> = if tool_result_blocks.is_empty() {
                    vec![]
                } else {
                    vec![Message {
                        role: Role::User,
                        content: tool_result_blocks,
                    }]
                };

                // Append to history.
                history.push(assistant_message.clone());
                for tr_msg in &tool_results {
                    history.push(tr_msg.clone());
                }

                // T11: scratch slot for the after_turn hook's decision; the
                // loop reads this AFTER yielding TurnEnd so the consumer sees
                // the turn data before any injected continuation messages.
                #[allow(unused_assignments)]
                let mut after_turn_decision_for_loop: TurnDecision = TurnDecision::Continue;

                // ---- after_turn / after_turn_failure hook ----
                //
                // Plan A T10: failing turn outcomes (Refusal, ContentFilter,
                // and MaxTokens at cap) route to `after_turn_failure` so
                // observability hooks can distinguish recoverable turns from
                // crashes without driving death spirals.
                {
                    let turn_outcome = TurnOutcome {
                        assistant_message: assistant_message.clone(),
                        tool_results: tool_results.clone(),
                        stop_reason: turn_stop_reason,
                        usage: turn_usage,
                        continue_loop: turn_stop_reason == StopReason::ToolUse,
                    };
                    let turn_ctx = TurnCtx {
                        turn_index,
                        messages: &history,
                        config: &self.config,
                    };
                    // Failure signal: Refusal / ContentFilter always fail; a
                    // MaxTokens turn fails only when Stage B has exhausted
                    // its budget (cap reached) — i.e. there are no more
                    // recoveries to attempt.
                    let turn_is_failure = matches!(
                        turn_stop_reason,
                        StopReason::Refusal | StopReason::ContentFilter
                    ) || (turn_stop_reason == StopReason::MaxTokens
                        && self.config.max_tokens_recovery
                        && stage_a_attempted_this_turn
                        && meta_continuation_count >= self.config.max_meta_continuations);
                    // Drive the hook. `after_turn_failure` returns `Result<()>`
                    // (no decision surface for failure paths); `after_turn`
                    // returns `Result<TurnDecision>`.
                    let after_turn_decision: TurnDecision = if turn_is_failure {
                        match self.hooks.after_turn_failure(&turn_ctx, &turn_outcome).await {
                            Ok(()) => TurnDecision::Continue,
                            Err(e) => {
                                tracing::warn!(error = %e, "after_turn_failure hook error (non-fatal)");
                                TurnDecision::Continue
                            }
                        }
                    } else {
                        match self.hooks.after_turn(&turn_ctx, &turn_outcome).await {
                            Ok(d) => d,
                            Err(e) => {
                                // Preserve original behavior: hook errors on
                                // `after_turn` are fatal (HookDenied path).
                                stopped_for =
                                    StopCondition::HookDenied(format!("after_turn: {e}"));
                                yield TurnEvent::TurnEnd {
                                    turn_index,
                                    assistant_message,
                                    tool_results,
                                    stop_reason: turn_stop_reason,
                                    usage: turn_usage,
                                    ttft: timing.ttft(),
                                    tbt: timing.tbt(),
                                };
                                total_usage.merge(turn_usage);
                                turns_completed += 1;
                                break 'outer;
                            }
                        }
                    };

                    // Apply the decision once the TurnEnd event has been
                    // emitted further below. Save it for the post-TurnEnd
                    // dispatch block.
                    after_turn_decision_for_loop = after_turn_decision;
                }

                let ttft = timing.ttft();
                let tbt = timing.tbt();

                yield TurnEvent::TurnEnd {
                    turn_index,
                    assistant_message,
                    tool_results,
                    stop_reason: turn_stop_reason,
                    usage: turn_usage,
                    ttft,
                    tbt,
                };

                if let Some(t) = ttft {
                    tracing::info!(
                        target: caliban_common::tracing_targets::TARGET_TIMING,
                        ttft_ms = u64::try_from(t.as_millis()).unwrap_or(u64::MAX),
                        tbt_ms = tbt.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
                        delta_count = timing.delta_count,
                        turn = turn_index,
                        "turn timing",
                    );
                }

                let cache_read = turn_usage.cache_read_input_tokens.unwrap_or(0);
                let cache_creation = turn_usage.cache_creation_input_tokens.unwrap_or(0);
                if cache_read > 0 || cache_creation > 0 {
                    tracing::info!(
                        target: caliban_common::tracing_targets::TARGET_CACHE,
                        cache_read,
                        cache_creation,
                        turn = turn_index,
                        "prompt cache stats",
                    );
                }

                total_usage.merge(turn_usage);
                turns_completed += 1;

                // ---- #239: no-edit-progress tracking + neutral nudge ----
                if had_successful_edit_this_turn {
                    // A successful edit-class call resets the streak and re-arms
                    // the nudge (so a later streak can fire again).
                    turns_since_last_edit = 0;
                    no_edit_nudge_armed = true;
                } else {
                    turns_since_last_edit += 1;
                    turns_without_edit = turns_without_edit.max(turns_since_last_edit);
                    let threshold = self.config.no_edit_nudge_threshold;
                    if threshold > 0
                        && turns_since_last_edit >= threshold
                        && no_edit_nudge_armed
                    {
                        tracing::info!(turns_since_last_edit, "no-edit nudge injected");
                        history.push(Message::user_text(format!(
                            "You have taken {turns_since_last_edit} turns without editing \
                             any files. If you have already identified the change you need \
                             to make, make the edit now rather than continuing to \
                             investigate. If you are still investigating, you can \
                             disregard this note."
                        )));
                        no_edit_nudge_emitted = true;
                        no_edit_nudge_armed = false;
                        // Re-arm Stage A for the nudged turn and advance.
                        stage_a_attempted_this_turn = false;
                        override_max_tokens_for_request = None;
                        break 'inner; // take another turn with the nudge in scope
                    }
                }

                // ---- T11: TurnDecision from `after_turn` ----
                match after_turn_decision_for_loop {
                    TurnDecision::Continue => {
                        // Fall through to the natural continue/halt logic.
                    }
                    TurnDecision::ContinueWith(msgs) => {
                        if forced_continuations < MAX_FORCED_CONTINUATIONS {
                            history.extend(msgs);
                            forced_continuations += 1;
                            // Reset Stage A so the forced turn has a fresh
                            // budget-escalation slot.
                            stage_a_attempted_this_turn = false;
                            override_max_tokens_for_request = None;
                            break 'inner; // advance to next turn_index
                        }
                        tracing::warn!(
                            forced_continuations,
                            "after_turn ContinueWith ignored (cap reached)"
                        );
                    }
                    TurnDecision::Stop => {
                        stopped_for = StopCondition::HookDenied("after_turn: Stop".into());
                        break 'outer;
                    }
                }

                // ---- Decide whether to continue (Tasks 4–6 dispatch) ----
                match turn_stop_reason {
                    StopReason::ToolUse => {
                        // Tool calls came back; reset Stage-A flag so the next
                        // turn has a fresh budget-escalation budget.
                        stage_a_attempted_this_turn = false;
                        override_max_tokens_for_request = None;
                        break 'inner;
                    }
                    StopReason::MaxTokens if self.config.max_tokens_recovery => {
                        // Stage A handled earlier (silent retry above
                        // tool-dispatch / TurnEnd yield / counter inc).
                        // If we reach this arm it's because Stage A
                        // already fired this turn and the retry still
                        // hit MaxTokens — try Stage B.
                        debug_assert!(
                            stage_a_attempted_this_turn,
                            "Stage A must have fired before we land here"
                        );
                        if meta_continuation_count < self.config.max_meta_continuations {
                            // Stage B: inject the meta-continuation prompt
                            // and advance to the next turn.
                            tracing::warn!(
                                target: "caliban::recovery",
                                meta_continuation = meta_continuation_count + 1,
                                "recovery.max_tokens.stage_b"
                            );
                            history.push(Message::user_text(
                                crate::stream::recovery::META_CONTINUATION_PROMPT,
                            ));
                            meta_continuation_count += 1;
                            stage_a_attempted_this_turn = false;
                            override_max_tokens_for_request = None;
                            break 'inner; // next turn iteration
                        }
                        // Stage C: surrender.
                        tracing::error!(
                            target: "caliban::recovery",
                            "recovery.max_tokens.stage_c"
                        );
                        stopped_for = StopCondition::MaxTokensExhausted;
                        break 'outer;
                    }
                    StopReason::Refusal => {
                        tracing::warn!(
                            target: "caliban::recovery",
                            "recovery.refusal"
                        );
                        history.push(Message::assistant_text(
                            crate::stream::recovery::REFUSAL_SYNTHETIC,
                        ));
                        stopped_for = StopCondition::Refusal(
                            crate::stream::recovery::REFUSAL_SYNTHETIC.into(),
                        );
                        break 'outer;
                    }
                    StopReason::ContentFilter => {
                        tracing::warn!(
                            target: "caliban::recovery",
                            "recovery.content_filter"
                        );
                        history.push(Message::assistant_text(
                            crate::stream::recovery::CONTENT_FILTER_SYNTHETIC,
                        ));
                        stopped_for = StopCondition::ContentFilter(
                            crate::stream::recovery::CONTENT_FILTER_SYNTHETIC.into(),
                        );
                        break 'outer;
                    }
                    StopReason::MaxTokens => {
                        // Recovery disabled — surface as a distinct stop
                        // condition so the TUI / headless driver can tell a
                        // budget blowout from a clean end-of-turn.
                        tracing::warn!(
                            target: "caliban::recovery",
                            "max_tokens.halt"
                        );
                        stopped_for = StopCondition::MaxTokensExhausted;
                        break 'outer;
                    }
                    _ => {
                        // EndTurn or StopSequence — natural completion. If an
                        // interactive input source is configured, await the
                        // next operator message instead of ending the run
                        // (ADR 0047 / #81). Human-driven, so NOT subject to
                        // MAX_FORCED_CONTINUATIONS.
                        //
                        // Note: injected messages land in `history` and therefore
                        // in `RunEnd.final_messages`. Live per-event display of
                        // injected user messages is deferred to the worker/attach
                        // tickets (2/3).
                        if let Some(provider) = settings.input_source.clone() {
                            let next = provider.next_input(&cancel).await;
                            if cancel.is_cancelled() {
                                stopped_for = StopCondition::Cancelled;
                                break 'outer;
                            }
                            match next {
                                Some(msgs) if !msgs.is_empty() => {
                                    history.extend(msgs);
                                    stage_a_attempted_this_turn = false;
                                    override_max_tokens_for_request = None;
                                    break 'inner; // take another turn
                                }
                                _ => {
                                    // None / empty → end of input.
                                    stopped_for = StopCondition::EndOfTurn;
                                    break 'outer;
                                }
                            }
                        }
                        stopped_for = StopCondition::EndOfTurn;
                        break 'outer;
                    }
                }
                } // end 'inner loop
            }

            // ---- after_run / after_run_failure hook (ADR 0028 + Plan A T10) ----
            //
            // Best-effort: errors logged but not allowed to override an
            // existing terminal `stopped_for`. Failure paths route through
            // `after_run_failure` so observability hooks can distinguish
            // crashes from natural completions without driving death spirals.
            {
                let is_failure = stopped_for.is_failure();
                let outcome = RunHookOutcome {
                    turn_count: turns_completed,
                    input_tokens: total_usage.input_tokens,
                    output_tokens: total_usage.output_tokens,
                    success: !is_failure,
                };
                let run_ctx = RunCtx {
                    session_id: &settings.session_id,
                    workspace_root: &settings.workspace_root,
                    user_message: user_msg_owned.as_ref(),
                    prompt_index: settings.prompt_index,
                    cancel: cancel.clone(),
                };
                let dispatch = if is_failure {
                    self.hooks.after_run_failure(&run_ctx, &outcome).await
                } else {
                    self.hooks.after_run(&run_ctx, &outcome).await
                };
                if let Err(e) = dispatch {
                    tracing::warn!(error = %e, "after_run* hook error (non-fatal)");
                }
            }

            yield TurnEvent::RunEnd {
                final_messages: history,
                total_usage,
                turn_count: turns_completed,
                stopped_for,
                turns_without_edit,
                no_edit_nudge_emitted,
            };
        })
    }
}
