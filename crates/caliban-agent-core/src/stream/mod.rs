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

/// Map a provider [`StopReason`] to the OpenTelemetry `GenAI`
/// `gen_ai.response.finish_reasons` value (ADR 0053 / #378).
///
/// Centralised so the generation span and any future emitter agree on one
/// stable vocabulary. Values follow the OpenTelemetry `GenAI` semantic
/// conventions' finish-reason strings: natural stops → `stop`, tool calls →
/// `tool_calls`, the token cap → `length`, and safety/refusal outcomes →
/// `content_filter`.
fn finish_reason_str(stop_reason: StopReason) -> &'static str {
    match stop_reason {
        // A natural end-of-turn or a stop-sequence hit both read as `stop`.
        StopReason::EndTurn | StopReason::StopSequence => "stop",
        StopReason::ToolUse => "tool_calls",
        StopReason::MaxTokens => "length",
        // Refusals and content-filter trips are both safety terminations.
        StopReason::Refusal | StopReason::ContentFilter => "content_filter",
    }
}

/// Whether `GenAI` message **content** may be captured on the chat generation
/// span (`gen_ai.input.messages` / `gen_ai.output.messages`), read once from the
/// `OTEL_LOG_USER_PROMPTS` environment variable.
///
/// Read directly from the environment — **not** via `caliban-telemetry`'s
/// `log_user_prompts` config — because `caliban-agent-core` deliberately does
/// not depend on the telemetry crate (ADR 0053 / #380). The accepted truthy set
/// (`1`/`true`/`yes`/`on`, case-insensitive) mirrors that crate's
/// `env_truthy_default`, and the default is **off**: no content is emitted
/// unless the operator explicitly opts in.
static LOG_USER_PROMPTS: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    matches!(
        std::env::var("OTEL_LOG_USER_PROMPTS")
            .ok()
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
});

/// `true` iff `GenAI` message-content capture is enabled for this process
/// (`OTEL_LOG_USER_PROMPTS` truthy). See [`LOG_USER_PROMPTS`].
fn content_capture_enabled() -> bool {
    *LOG_USER_PROMPTS
}

/// Serialize a slice of provider [`Message`]s to the OpenTelemetry `GenAI`
/// structured message shape (semconv "current form") as a single JSON string,
/// suitable for the `gen_ai.input.messages` / `gen_ai.output.messages`
/// attributes (ADR 0053 / #380).
///
/// Best-effort and non-panicking: blocks with no textual/tool projection (e.g.
/// images) are dropped, and any serialization failure returns `None` so the
/// caller simply omits content for the turn rather than failing the run. Only
/// invoked when [`content_capture_enabled`] is `true`.
/// Per-part content cap (chars) for the `gen_ai.*.messages` projection (#428).
/// Bounds each text / tool blob so a single huge prompt or tool result can't
/// produce a multi-MB span attribute (collectors then drop the whole span).
const SEMCONV_PART_CAP: usize = 8 * 1024;

/// Total cap (bytes) for the serialized `gen_ai.*.messages` attribute (#428).
/// The input history grows every turn, so without a bound the attribute is
/// O(history^2) over a run; keep only the most-recent messages that fit.
const SEMCONV_TOTAL_CAP: usize = 128 * 1024;

/// Truncate `s` to at most [`SEMCONV_PART_CAP`] chars on a char boundary,
/// marking truncation. Char-based, so it never splits a UTF-8 scalar.
fn cap_semconv(s: &str) -> String {
    match s.char_indices().nth(SEMCONV_PART_CAP) {
        Some((idx, _)) => {
            let mut out = s[..idx].to_string();
            out.push_str("…[truncated]");
            out
        }
        None => s.to_string(),
    }
}

/// Cap a JSON `Value` used as a tool-call `arguments` payload: pass small values
/// through, but replace an oversized one (e.g. a `Write` body) with a truncated
/// string so it can't blow up the span attribute (#428).
fn cap_semconv_value(v: &serde_json::Value) -> serde_json::Value {
    let s = v.to_string();
    if s.len() <= SEMCONV_PART_CAP {
        v.clone()
    } else {
        serde_json::Value::String(cap_semconv(&s))
    }
}

fn messages_to_semconv_json(messages: &[Message]) -> Option<String> {
    // Project each message (per-part content already capped in
    // `content_block_to_part`), then keep only the most-recent messages whose
    // serialized size fits under the total cap (#428).
    let vals: Vec<serde_json::Value> = messages.iter().map(message_to_semconv_value).collect();
    let mut total: usize = 2; // "[]"
    let mut start = vals.len();
    for (i, v) in vals.iter().enumerate().rev() {
        let vlen = serde_json::to_string(v).map_or(0, |s| s.len() + 1);
        if total + vlen > SEMCONV_TOTAL_CAP && i + 1 < vals.len() {
            break;
        }
        total += vlen;
        start = i;
    }
    let kept: Vec<serde_json::Value> = vals[start..].to_vec();
    serde_json::to_string(&serde_json::Value::Array(kept)).ok()
}

/// Map one provider [`Message`] to a semconv `{ "role", "parts" }` object.
fn message_to_semconv_value(msg: &Message) -> serde_json::Value {
    let parts: Vec<serde_json::Value> = msg
        .content
        .iter()
        .filter_map(content_block_to_part)
        .collect();
    serde_json::json!({ "role": semconv_role(msg), "parts": parts })
}

/// Map a caliban [`Role`] to the semconv role string, promoting a `User`
/// message whose blocks are *all* tool results to the `"tool"` role — that is
/// how caliban carries tool-call responses back to the model.
fn semconv_role(msg: &Message) -> &'static str {
    match msg.role {
        Role::System => "system",
        Role::Assistant => "assistant",
        Role::User => {
            if !msg.content.is_empty()
                && msg
                    .content
                    .iter()
                    .all(|b| matches!(b, ContentBlock::ToolResult(_)))
            {
                "tool"
            } else {
                "user"
            }
        }
    }
}

/// Map a [`ContentBlock`] to a semconv message part, or `None` for blocks with
/// no textual/tool projection (images — never leak their bytes).
fn content_block_to_part(block: &ContentBlock) -> Option<serde_json::Value> {
    match block {
        ContentBlock::Text(t) => Some(serde_json::json!({
            "type": "text",
            "content": cap_semconv(&t.text),
        })),
        // Extended-thinking is assistant reasoning text; project it best-effort
        // as a text part.
        ContentBlock::Thinking(th) => Some(serde_json::json!({
            "type": "text",
            "content": cap_semconv(&th.thinking),
        })),
        ContentBlock::ToolUse(tu) => Some(serde_json::json!({
            "type": "tool_call",
            "id": tu.id,
            "name": tu.name,
            "arguments": cap_semconv_value(&tu.input),
        })),
        ContentBlock::ToolResult(tr) => Some(serde_json::json!({
            "type": "tool_call_response",
            "id": tr.tool_use_id,
            "result": cap_semconv(&tool_result_text(tr)),
        })),
        ContentBlock::Image(_) => None,
    }
}

/// Global display cap (in chars) for the `result_text` carried on
/// [`TurnEvent::ToolCallEnd`]. A dashboard-preview bound, deliberately smaller
/// than and independent of the model-context cap
/// ([`crate::post_process::ToolResultCap`]): observers want a tight, cheap
/// preview, not the full body fed back to the model.
pub const STREAM_RESULT_TEXT_CAP: usize = 8 * 1024;

/// Flatten content blocks' text into a single string: text blocks joined by
/// newlines, non-text blocks ignored.
fn flatten_content_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Flatten `content` to display text and cap it to `max_chars`. Char-based, so
/// truncation never splits a UTF-8 boundary. Returns `(text, truncated)` where
/// `text` is the head (prefix) of the flattened result.
fn capped_result_text(content: &[ContentBlock], max_chars: usize) -> (String, bool) {
    let full = flatten_content_text(content);
    if full.chars().count() > max_chars {
        (full.chars().take(max_chars).collect(), true)
    } else {
        (full, false)
    }
}

/// Flatten a [`ToolResultBlock`]'s content into a single string: text blocks
/// joined by newlines, non-text blocks ignored.
fn tool_result_text(tr: &ToolResultBlock) -> String {
    flatten_content_text(&tr.content)
}

/// Build the error [`ToolResultBlock`] used for a denied tool call (plan-mode
/// gate or hook deny): a single text block flagged `is_error`.
fn denied_tool_result(tool_use_id: &str, text: String) -> ToolResultBlock {
    ToolResultBlock {
        tool_use_id: tool_use_id.to_string(),
        content: vec![ContentBlock::Text(TextBlock {
            text,
            cache_control: None,
        })],
        is_error: true,
    }
}

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
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
use tracing::{Instrument as _, field::Empty};

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

pin_project_lite::pin_project! {
    /// Stream combinator that enters `span` for the duration of every
    /// `poll_next` on the wrapped stream (#385).
    ///
    /// The run loop is a `try_stream!` future: its body executes while the
    /// stream is *polled*, not while it is *constructed*. A plain
    /// `#[instrument]` on a synchronous stream-returning fn therefore only
    /// covers construction, leaving the per-request `chat` and `execute_tool`
    /// spans created during polling with the wrong parent. Entering the run
    /// span on each poll makes it the current span while the loop runs, so
    /// those child spans nest under it with zero per-site plumbing.
    ///
    /// Entering across an `.await` is normally a hazard, but `poll_next` is
    /// synchronous: the guard is created and dropped within a single poll,
    /// never held across a suspension point.
    struct SpanStream<S> {
        #[pin]
        inner: S,
        span: tracing::Span,
    }
}

impl<S: Stream> Stream for SpanStream<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let _enter = this.span.enter();
        this.inner.poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// Neutral nudge injected by the #249 empty/degenerate-turn guard: a turn that
/// reasoned but emitted no tool call and no answer. Phrased to push the model to
/// either act (call a tool) or commit to a final textual answer. The substring
/// "no tool call" is asserted by the `empty_turn_nudge` integration tests.
const EMPTY_TURN_NUDGE: &str = "Your previous response made no tool call and gave no answer — \
it contained only internal reasoning. To make progress you must take a concrete action now: \
call a tool to work on the task, or, if the task is genuinely complete, state your final \
answer as plain text.";

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
        /// Content blocks returned by the tool (or the error message). Full and
        /// structured; the authoritative copy is also carried in
        /// [`TurnEvent::TurnEnd`]'s `tool_results`.
        content: Vec<ContentBlock>,
        /// Display-ready flattened text of the result, capped to
        /// [`STREAM_RESULT_TEXT_CAP`] chars. The head (prefix) of the result;
        /// non-text blocks (e.g. images) are omitted. For observers (e.g. the
        /// prospero dashboard) that want a cheap, bounded preview without
        /// re-flattening `content`.
        result_text: String,
        /// `true` iff `result_text` was truncated to fit the cap.
        truncated: bool,
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
    /// A single turn streamed *thinking* past `max_turn_thinking_chars` without
    /// producing a tool call or final text — a runaway reasoning spiral (#62).
    /// Independent of the idle watchdog (the spiral streams continuously) and of
    /// the `max_tokens` budget (recovery raises it).
    ThinkingBudgetExhausted,
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
                | Self::ThinkingBudgetExhausted
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
            Self::ThinkingBudgetExhausted => (
                "thinking budget exhausted \u{2014} the model kept reasoning without answering; \
                 try /effort low to reduce reasoning budget"
                    .to_string(),
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
mod finish_reason_tests {
    use super::*;

    #[test]
    fn maps_every_stop_reason_to_semconv_string() {
        // #378: the StopReason -> gen_ai.response.finish_reasons mapping.
        assert_eq!(finish_reason_str(StopReason::EndTurn), "stop");
        assert_eq!(finish_reason_str(StopReason::StopSequence), "stop");
        assert_eq!(finish_reason_str(StopReason::ToolUse), "tool_calls");
        assert_eq!(finish_reason_str(StopReason::MaxTokens), "length");
        assert_eq!(finish_reason_str(StopReason::Refusal), "content_filter");
        assert_eq!(
            finish_reason_str(StopReason::ContentFilter),
            "content_filter"
        );
    }
}

#[cfg(test)]
mod genai_content_tests {
    //! #380: semconv message-content serialization mapping.
    use super::*;
    use caliban_provider::{TextBlock, ThinkingBlock, ToolUseBlock};
    use serde_json::json;

    fn parse(messages: &[Message]) -> serde_json::Value {
        serde_json::from_str(&messages_to_semconv_json(messages).expect("serialize")).unwrap()
    }

    #[test]
    fn user_text_maps_to_user_role_text_part() {
        let v = parse(&[Message::user_text("hi there")]);
        assert_eq!(v[0]["role"], "user");
        assert_eq!(v[0]["parts"][0]["type"], "text");
        assert_eq!(v[0]["parts"][0]["content"], "hi there");
    }

    #[test]
    fn oversized_text_part_is_capped() {
        // #428: one huge message must not produce a multi-MB attribute.
        let v = parse(&[Message::user_text("a".repeat(100_000))]);
        let content = v[0]["parts"][0]["content"].as_str().unwrap();
        assert!(
            content.len() < 20_000,
            "part not capped: {} bytes",
            content.len()
        );
        assert!(
            content.ends_with("[truncated]"),
            "missing truncation marker"
        );
    }

    #[test]
    fn total_attribute_is_bounded_and_keeps_recent() {
        // #428: many large messages → the whole attribute stays under the total
        // cap, keeping the most-recent messages.
        let msgs: Vec<Message> = (0..200)
            .map(|i| Message::user_text(format!("msg{i}-{}", "x".repeat(4000))))
            .collect();
        let json = messages_to_semconv_json(&msgs).expect("serialize");
        assert!(
            json.len() <= SEMCONV_TOTAL_CAP,
            "total not bounded: {} bytes",
            json.len()
        );
        assert!(
            json.contains("msg199-"),
            "the most-recent message must be kept"
        );
    }

    #[test]
    fn assistant_tool_use_maps_to_tool_call_part() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text(TextBlock {
                    text: "let me look".into(),
                    cache_control: None,
                }),
                ContentBlock::ToolUse(ToolUseBlock {
                    id: "call-1".into(),
                    name: "Read".into(),
                    input: json!({"path": "/x"}),
                }),
            ],
        };
        let v = parse(std::slice::from_ref(&msg));
        assert_eq!(v[0]["role"], "assistant");
        assert_eq!(v[0]["parts"][0]["content"], "let me look");
        assert_eq!(v[0]["parts"][1]["type"], "tool_call");
        assert_eq!(v[0]["parts"][1]["id"], "call-1");
        assert_eq!(v[0]["parts"][1]["name"], "Read");
        assert_eq!(v[0]["parts"][1]["arguments"]["path"], "/x");
    }

    #[test]
    fn tool_result_user_message_promotes_to_tool_role() {
        // caliban carries tool results in a `User` message; it must map to the
        // semconv `"tool"` role with `tool_call_response` parts.
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: "call-1".into(),
                content: vec![ContentBlock::Text(TextBlock {
                    text: "file contents".into(),
                    cache_control: None,
                })],
                is_error: false,
            })],
        };
        let v = parse(std::slice::from_ref(&msg));
        assert_eq!(v[0]["role"], "tool");
        assert_eq!(v[0]["parts"][0]["type"], "tool_call_response");
        assert_eq!(v[0]["parts"][0]["id"], "call-1");
        assert_eq!(v[0]["parts"][0]["result"], "file contents");
    }

    #[test]
    fn thinking_maps_to_text_part() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Thinking(ThinkingBlock {
                thinking: "reasoning".into(),
                signature: None,
            })],
        };
        let v = parse(std::slice::from_ref(&msg));
        assert_eq!(v[0]["parts"][0]["type"], "text");
        assert_eq!(v[0]["parts"][0]["content"], "reasoning");
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
// Turn-loop helpers (extracted from `stream_until_done_with_settings`).
//
// These are pure-logic / non-yielding lifts of inline blocks: each one is a
// 1:1 behavior-preserving move so the macro body shrinks to an orchestration
// skeleton (#152). None of them may `yield` — that only works inside the
// `try_stream!` body — so all incremental streaming stays in the skeleton.
// ---------------------------------------------------------------------------

impl Agent {
    /// Threshold-gated autocompaction (Plan B) plus the surrounding hook calls.
    ///
    /// Mutates `history` in place when a compaction succeeds and updates
    /// `tracking` so repeated failures eventually disable autocompaction for
    /// the run. Non-fatal: hook/compactor errors are logged, never propagated.
    /// 1:1 lift of the inline compaction block.
    ///
    /// Returns any provider usage the compaction itself incurred (e.g. the
    /// summarizer call), so the caller can fold autocompact spend into the
    /// session totals (#329/#292). Zero when no compaction ran or the strategy
    /// is LLM-free.
    async fn maybe_compact(
        &self,
        history: &mut Vec<Message>,
        tracking: &mut recovery::AutoCompactTracking,
    ) -> Usage {
        let active_model_snapshot = self.active_model();
        let caps = self.provider.capabilities(active_model_snapshot.as_str());
        let token_count_before = crate::compact::estimate_tokens(history);
        let threshold = self.config.auto_compact_threshold;
        let should_attempt = threshold.is_some_and(|t| {
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss
            )]
            let utilization = token_count_before as f32 / caps.max_input_tokens.max(1) as f32;
            !tracking.disabled && utilization >= t
        });
        if !should_attempt {
            return Usage::default();
        }
        let strategy = self.compactor.strategy_name();
        let compact_ctx = CompactCtx {
            session_id: "",
            token_count_before,
            strategy,
        };
        if let Err(e) = self.hooks.pre_compact(&compact_ctx).await {
            tracing::warn!(error = %e, "pre_compact hook error (non-fatal)");
        }
        match self.compactor.compact(history, &caps).await {
            Err(e) => {
                tracing::warn!(error = %e, "autocompact failed");
                tracking.consecutive_failures = tracking.consecutive_failures.saturating_add(1);
                if tracking.consecutive_failures >= recovery::MAX_CONSECUTIVE_COMPACT_FAILURES {
                    tracking.disabled = true;
                    tracing::warn!(
                        "autocompact disabled after {} consecutive failures",
                        recovery::MAX_CONSECUTIVE_COMPACT_FAILURES
                    );
                }
                Usage::default()
            }
            Ok(Some(compaction)) => {
                tracking.consecutive_failures = 0;
                let token_count_after = crate::compact::estimate_tokens(&compaction.messages);
                *history = compaction.messages;
                let outcome = CompactOutcome {
                    token_count_after,
                    compacted: true,
                };
                if let Err(e) = self.hooks.post_compact(&compact_ctx, &outcome).await {
                    tracing::warn!(error = %e, "post_compact hook error (non-fatal)");
                }
                compaction.usage.unwrap_or_default()
            }
            Ok(None) => {
                tracking.consecutive_failures = 0;
                let outcome = CompactOutcome {
                    token_count_after: token_count_before,
                    compacted: false,
                };
                if let Err(e) = self.hooks.post_compact(&compact_ctx, &outcome).await {
                    tracing::warn!(error = %e, "post_compact hook error (non-fatal)");
                }
                Usage::default()
            }
        }
    }

    /// Build the per-turn [`CompletionRequest`] from the current `history`.
    ///
    /// Applies the MCP wire filter, deferred-block splice, and prompt cache,
    /// then snapshots the swappable effort/thinking/model controls so the
    /// in-flight request sees one coherent set even if `/effort`, `/think`, or
    /// `/model` lands mid-turn. `effective_max_tokens` is the resolved
    /// per-request budget (Stage-A escalation already applied by the caller via
    /// [`recovery::RecoveryState::effective_max_tokens`]). 1:1 lift of the
    /// inline request-construction block.
    fn build_request(&self, history: &[Message], effective_max_tokens: u32) -> CompletionRequest {
        let mut req_messages = history.to_vec();
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
        // Plan A: Stage A escalation already resolved by the caller.
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
        CompletionRequest {
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
        }
    }

    /// Collect dispatched tool results in assistant-message order, apply the
    /// tool-result size cap, build the tool-results `Message`, and append the
    /// assistant message + tool results onto `history`.
    ///
    /// Returns the tool-results messages (empty when no tools ran) and whether
    /// the turn made edit progress (`had_successful_edit_this_turn`, #239/#244:
    /// at least one dispatched *file-mutating* call returned a non-error
    /// result). 1:1 lift of the inline Phase-3 + cap + history-append block.
    ///
    /// NOTE: this contains no `yield`; the in-completion-order `ToolCallEnd`
    /// yields stay in the skeleton's dispatch drain.
    async fn finalize_tool_results(
        &self,
        history: &mut Vec<Message>,
        assistant_message: &Message,
        ordered_results: Vec<Option<ToolResultBlock>>,
        file_mutating_tool_ids: &std::collections::HashSet<String>,
        session_id: &str,
    ) -> (Vec<Message>, bool) {
        // ---- Phase 3: collect results in assistant-message order ----
        let mut tool_result_blocks: Vec<ContentBlock> = Vec::new();
        // #239 / #244: a turn counts as edit progress iff at least one
        // dispatched *file-mutating* tool call returned a *successful*
        // (non-error) result.
        let mut had_successful_edit_this_turn = false;
        for slot in ordered_results.into_iter().flatten() {
            if !slot.is_error && file_mutating_tool_ids.contains(&slot.tool_use_id) {
                had_successful_edit_this_turn = true;
            }
            tool_result_blocks.push(ContentBlock::ToolResult(slot));
        }

        // ---- Tool-result size cap (context-management spec) ----
        if self.config.tool_result_cap_chars > 0 && !tool_result_blocks.is_empty() {
            let overflow_dir = caliban_common::paths::platform_cache_dir().map_or_else(
                || std::path::PathBuf::from("/tmp/caliban-tool-overflows"),
                |d| d.join("caliban").join("tool-overflows"),
            );
            let cap = crate::post_process::ToolResultCap {
                max_chars: self.config.tool_result_cap_chars,
                overflow_dir,
                session_id: session_id.to_string(),
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

        (tool_results, had_successful_edit_this_turn)
    }

    /// Plan a single tool call (Phase 1): plan-mode gating, `before_tool` hook,
    /// `UpdatedInput` validation, and the allow/deny decision — everything
    /// except the `ToolCallEnd` yield, which the skeleton performs from the
    /// returned [`ToolPlan`] so completion-order streaming stays in the loop.
    /// 1:1 lift of the per-tool body of the Phase-1 plan loop.
    async fn plan_tool_call(
        &self,
        tu: &caliban_provider::ToolUseBlock,
        original_index: usize,
        turn_index: u32,
        session_id: &str,
    ) -> ToolPlan {
        // Plan-mode gating: when active, reject tools that are neither
        // side-effect-free (Tool::is_read_only) nor a plan-control tool, BEFORE
        // running hooks (cheaper, and the rejection still goes back to the model
        // as a normal ToolResult so it can adapt).
        let tool_is_read_only = self.tools.get(&tu.name).is_some_and(|t| t.is_read_only());
        let tool_mutates_files = self.tools.get(&tu.name).is_some_and(|t| t.mutates_files());
        let plan_mode_active = self
            .plan_mode
            .as_ref()
            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed));
        if plan_mode_active
            && !(tool_is_read_only || crate::plan_mode::is_plan_control_tool(&tu.name))
        {
            let result = denied_tool_result(
                &tu.id,
                format!(
                    "Tool '{}' is not available in plan mode. Use ExitPlanMode to proceed.",
                    tu.name
                ),
            );
            return ToolPlan::Denied {
                original_index,
                result,
            };
        }

        let tool_ctx = ToolCtx {
            session_id,
            turn_index,
            tool_use_id: &tu.id,
            tool_name: &tu.name,
            input: &tu.input,
            is_read_only: tool_is_read_only,
        };
        let decision = match self.hooks.before_tool(&tool_ctx).await {
            Ok(d) => d,
            Err(e) => {
                return ToolPlan::Fatal(StopCondition::HookDenied(format!(
                    "before_tool hook failed: {e}"
                )));
            }
        };

        // ADR-0024 / #185 H6: a hook may rewrite a tool's input via
        // UpdatedInput, but the rewrite must be validated against the tool's
        // schema; an invalid rewrite is a *hard deny* (not dispatched). Convert
        // it to Deny here so it flows through the denial path below (after_tool
        // notify + error result).
        let decision = if let HookDecision::UpdatedInput(new_input) = &decision {
            match self.tools.get(&tu.name) {
                Some(tool) => match updated_input_is_valid(tool.input_schema(), new_input) {
                    Ok(()) => decision,
                    Err(why) => HookDecision::Deny(format!(
                        "before_tool hook rewrote input to an invalid shape: {why}"
                    )),
                },
                None => decision,
            }
        } else {
            decision
        };

        match decision {
            // AskDenied (a synthesized non-interactive Ask→Deny) is handled
            // identically to Deny; the mode filter normally normalizes it away,
            // but treat it as a denial defensively.
            HookDecision::Deny(msg) | HookDecision::AskDenied(msg) => {
                // Mirror dispatch_tool: notify after_tool of the denial.
                let denial_err =
                    ToolError::execution(std::io::Error::other(format!("denied: {msg}")));
                if let Err(e) = self.hooks.after_tool(&tool_ctx, &Err(denial_err)).await {
                    tracing::warn!(
                        tool = %tu.name, error = %e,
                        "after_tool hook error (non-fatal)"
                    );
                }
                ToolPlan::Denied {
                    original_index,
                    result: denied_tool_result(&tu.id, format!("Tool call denied: {msg}")),
                }
            }
            // Allow uses the original input; UpdatedInput swaps in the rewritten
            // input (already schema-validated above). Both build an Allowed plan.
            HookDecision::Allow | HookDecision::UpdatedInput(_) => {
                let input = if let HookDecision::UpdatedInput(new_input) = decision {
                    tracing::info!(
                        tool = %tu.name,
                        tool_use_id = %tu.id,
                        "hook.updated_input: tool input rewritten by before_tool hook"
                    );
                    new_input
                } else {
                    tu.input.clone()
                };
                let conflict_key = self
                    .tools
                    .get(&tu.name)
                    .and_then(|t| t.parallel_conflict_key(&input));
                ToolPlan::Allowed {
                    plan: DispatchPlan::Allowed {
                        original_index,
                        id: tu.id.clone(),
                        name: tu.name.clone(),
                        input,
                        conflict_key,
                    },
                    mutates_files: tool_mutates_files,
                }
            }
        }
    }
}

/// Outcome of [`Agent::plan_tool_call`]. The skeleton maps this onto the
/// `ToolCallEnd` yield + plan push so all streaming stays in the loop (#152).
enum ToolPlan {
    /// Tool was denied (plan-mode or hook). The skeleton yields a denied
    /// `ToolCallEnd` (in assistant-message order) and pushes a `Denied` plan.
    Denied {
        original_index: usize,
        result: ToolResultBlock,
    },
    /// Tool is allowed/rewritten. `mutates_files` marks it for the no-edit
    /// progress set (#239/#244).
    Allowed {
        plan: DispatchPlan,
        mutates_files: bool,
    },
    /// A `before_tool` hook error that must terminate the run.
    Fatal(StopCondition),
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
    // The pure-logic and recovery state machine have been lifted out (#152:
    // `maybe_compact`, `build_request`, `finalize_tool_results`,
    // `plan_tool_call`, and `RecoveryState`), shrinking this body from ~1090 to
    // ~600 lines. It cannot reach clippy's 100-line cap: the three streaming
    // phases (SSE drain, Phase-1 plan loop, Phase-2 dispatch drain) `yield`
    // `TurnEvent`s incrementally and in tool-completion order, so they MUST
    // stay inline in the `try_stream!` body — extracting them into Vec-returning
    // helpers would silently destroy incremental streaming. The allow is
    // therefore intrinsic to the function's role as the orchestration skeleton.
    #[allow(clippy::too_many_lines)]
    pub fn stream_until_done_with_settings(
        self: Arc<Self>,
        messages: Vec<Message>,
        cancel: CancellationToken,
        settings: RunSettings,
    ) -> TurnEventStream {
        // #385: the run span, created explicitly with the same name and fields
        // the former `#[instrument]` attribute produced (name = the fn name;
        // fields `model` / `session` / `prompt`). Fields are captured now,
        // before `self` and `settings` are moved into the `try_stream!` body.
        // `SpanStream` below enters this span on every poll so the loop's
        // per-request `chat` and per-call `execute_tool` spans nest under it.
        let run_span = tracing::info_span!(
            "stream_until_done_with_settings",
            model = %self.active_model(),
            session = %settings.session_id,
            prompt = settings.prompt_index,
        );
        let inner = try_stream! {
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
            // The six recovery flags (Stage-A pair, Stage-B meta count,
            // reactive-compact guard, forced-continuation cap, autocompact
            // tracking) are owned by RecoveryState; the A/B/C decision logic
            // lives on its methods (#152).
            let mut recovery = recovery::RecoveryState::default();

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

            // #249 — empty/degenerate-turn guard (per-run): counts *consecutive*
            // degenerate turns nudged so far (a turn that consumed output tokens
            // yet produced no tool call and no actionable text — e.g. an Ollama
            // reasoning model that emits only a thinking block and then stops).
            // Resets to 0 the moment a productive turn occurs. Bounded by
            // `config.empty_turn_nudge_max` so a perpetually-stalling model
            // cannot loop forever.
            let mut empty_turn_nudges: u32 = 0;

            'outer: for turn_index in 0..max_turns {
                // #245: bounded budget to re-issue THIS turn when the provider
                // stream is interrupted *before any content is emitted*. Fresh
                // per turn; persists across `continue 'inner` within a turn so
                // repeated mid-open interruptions stay bounded.
                let mut drain_retries: u32 = 0;
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
                    // #421: caps from the active model, not config.model (they
                    // diverge after a `/model` swap).
                    let caps = self.provider.capabilities(self.active_model().as_str());
                    if let Ok(Some(compaction)) = crate::compact::MicroCompactor::new()
                        .compact(&history, &caps)
                        .await
                    {
                        let freed = crate::compact::estimate_tokens(&history)
                            .saturating_sub(crate::compact::estimate_tokens(&compaction.messages));
                        tracing::debug!(
                            target: "caliban::compact",
                            freed_tokens = freed,
                            "microcompact",
                        );
                        history = compaction.messages;
                    }
                }

                // ---- Compaction (threshold-gated autocompact) ----
                // Fold any summarization spend into the session totals so
                // autocompact cost is not invisible (#329/#292).
                let compact_usage = self
                    .maybe_compact(&mut history, recovery.auto_tracking_mut())
                    .await;
                total_usage.merge(compact_usage);

                // ---- Build completion request ----
                let req = self.build_request(
                    &history,
                    recovery.effective_max_tokens(self.config.max_tokens),
                );

                // ---- OTel GenAI generation span (#378, ADR 0053) ----
                //
                // One "chat" span per provider request (semconv-only: no vendor
                // keys, no cost, no message content — content is #380). Created
                // as a child of the current span (the run span when active) and
                // instrumented around the provider request below so it times the
                // request+response. Request params are recorded now; response
                // model / finish reason / token usage are recorded after the
                // drain (fields left `Empty` until then). Dotted field names map
                // verbatim to OTel attribute keys via tracing-opentelemetry, and
                // `otel.name` / `otel.kind` set the exported span name and kind.
                let chat_span = tracing::info_span!(
                    "chat",
                    otel.name = Empty,
                    otel.kind = "client",
                    gen_ai.operation.name = "chat",
                    gen_ai.provider.name = %self.provider.name(),
                    gen_ai.request.model = %req.model,
                    gen_ai.request.temperature = Empty,
                    gen_ai.request.max_tokens = Empty,
                    gen_ai.request.top_p = Empty,
                    gen_ai.response.model = Empty,
                    gen_ai.response.finish_reasons = Empty,
                    gen_ai.usage.input_tokens = Empty,
                    gen_ai.usage.output_tokens = Empty,
                    // #380: message CONTENT, captured only when
                    // `OTEL_LOG_USER_PROMPTS` is truthy (see `content_capture_enabled`).
                    // Declared `Empty` so they are absent by default.
                    gen_ai.input.messages = Empty,
                    gen_ai.output.messages = Empty,
                );
                chat_span.record("otel.name", format!("chat {}", req.model).as_str());
                // max_tokens is always resolved on the request; temperature /
                // top_p are optional and skipped when unset (semconv: omit rather
                // than emit a null/zero).
                chat_span.record("gen_ai.request.max_tokens", i64::from(req.max_tokens));
                if let Some(t) = req.temperature {
                    chat_span.record("gen_ai.request.temperature", f64::from(t));
                }
                if let Some(p) = req.top_p {
                    chat_span.record("gen_ai.request.top_p", f64::from(p));
                }
                // #380: input message CONTENT (semconv-only), recorded now
                // because the full input is known at request-build time. Gated:
                // emitted only when `OTEL_LOG_USER_PROMPTS` is truthy, and
                // best-effort (a serialization failure simply omits content for
                // the turn rather than failing the run).
                if content_capture_enabled() {
                    if let Some(json) = messages_to_semconv_json(&req.messages) {
                        chat_span.record("gen_ai.input.messages", json.as_str());
                    } else {
                        tracing::debug!(
                            "gen_ai.input.messages: serialization failed; content omitted",
                        );
                    }
                }

                // ---- Stream from provider (with retry) ----
                // Begin per-turn timing here so TTFT reflects user-observed
                // latency including any backoff sleeps (typically 0 on the
                // first attempt).
                let mut timing = TurnTiming::start();
                let provider = Arc::clone(&self.provider);
                let req_clone = req.clone();
                let cancel_for_retry = cancel.clone();
                // #330: bound the connect→first-header wait uniformly here.
                // Transports set only a connect timeout on their streaming client
                // (no total deadline), and `WatchedStream` can't observe the
                // stream until *after* `provider.stream()`'s `.send()` returns
                // headers — so a server that accepts the connection but never
                // responds would hang forever. Wrap the whole `stream()` future
                // in a first-byte timeout, reusing the prefill budget as the
                // single time-to-first-token bound (`0` = unbounded/legacy). On
                // elapse we surface `StreamIdle` — terminal, like the prefill/
                // idle watchdog, and bounded to one budget rather than
                // retry×budget.
                let first_byte_budget = self.config.stream_prefill_timeout_ms;
                let stream_result = with_retry(&self.retry, &cancel, move || {
                    let p = Arc::clone(&provider);
                    let r = req_clone.clone();
                    let _ = &cancel_for_retry; // ensure the clone is moved
                    async move {
                        if first_byte_budget == 0 {
                            p.stream(r).await
                        } else {
                            let budget = Duration::from_millis(first_byte_budget.into());
                            match tokio::time::timeout(budget, p.stream(r)).await {
                                Ok(res) => res,
                                Err(_elapsed) => {
                                    Err(caliban_provider::Error::StreamIdle(budget))
                                }
                            }
                        }
                    }
                })
                .instrument(chat_span.clone())
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
                            if recovery.reactive_compact_available() =>
                        {
                            match recovery.on_context_too_long(&self, &mut history).await {
                                recovery::RecoveryAction::RetryTurn => continue 'inner,
                                recovery::RecoveryAction::Surrender(stop) => {
                                    stopped_for = stop;
                                    break 'outer;
                                }
                                recovery::RecoveryAction::InjectAndContinue(_) => {
                                    unreachable!(
                                        "on_context_too_long only yields RetryTurn or Surrender"
                                    )
                                }
                            }
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
                    let idle = Duration::from_millis(self.config.stream_idle_timeout_ms.into());
                    // `0` prefill → no separate grace, fall back to `idle`.
                    let prefill = if self.config.stream_prefill_timeout_ms > 0 {
                        Duration::from_millis(self.config.stream_prefill_timeout_ms.into())
                    } else {
                        idle
                    };
                    Box::pin(caliban_provider::stream::WatchedStream::new(
                        provider_stream,
                        idle,
                        prefill,
                    ))
                } else {
                    provider_stream
                };
                let mut acc = MessageAccumulator::new();
                // #245: whether this attempt has yielded any *content* (text /
                // thinking / tool deltas or a tool-call start). A retryable
                // interruption is only safe to re-issue while this is false —
                // otherwise replaying would double-emit to stream consumers.
                let mut emitted_content_this_turn = false;
                // #62: cumulative thinking chars this attempt. Bounds a runaway
                // reasoning spiral that streams continuously (so the idle
                // watchdog never fires) without producing a tool call or text.
                let max_turn_thinking_chars = self.config.max_turn_thinking_chars;
                let mut thinking_chars: usize = 0;

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
                            // #245: a transient interruption *before any content
                            // was emitted* is safe to re-issue (the consumer has
                            // seen at most a TurnStart). Discard the partial
                            // accumulator and redo the turn through the
                            // `with_retry` open path; bounded so a persistent
                            // failure still terminates. Once content has streamed
                            // we cannot replay cleanly, so fall through to a
                            // terminal error.
                            if crate::retry::is_retryable(&e)
                                && !emitted_content_this_turn
                                && drain_retries
                                    < self.retry.max_attempts.saturating_sub(1)
                            {
                                drain_retries += 1;
                                tracing::warn!(
                                    error = %e,
                                    attempt = drain_retries,
                                    "stream interrupted before content; re-issuing turn"
                                );
                                continue 'inner;
                            }
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
                                    acc.active[i] = Some(ActiveBlock::Thinking {
                                        accumulated: String::new(),
                                        signature: String::new(),
                                    });
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
                                    emitted_content_this_turn = true;
                                }
                                StreamingContentType::Image => {
                                    // Image streaming blocks are not supported; leave placeholder.
                                }
                            }
                        }
                        StreamEvent::Delta { index, delta } => {
                            timing.observe_delta();
                            emitted_content_this_turn = true;
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
                                    Some(ActiveBlock::Thinking { accumulated, .. }),
                                    StreamingDelta::Thinking(s),
                                ) => {
                                    accumulated.push_str(&s);
                                    thinking_chars =
                                        thinking_chars.saturating_add(s.chars().count());
                                    yield TurnEvent::AssistantThinkingDelta {
                                        turn_index,
                                        content_block_index: index,
                                        text: s,
                                    };
                                    // #62: trip the runaway-thinking guard.
                                    if max_turn_thinking_chars > 0
                                        && thinking_chars > max_turn_thinking_chars
                                    {
                                        stopped_for = StopCondition::ThinkingBudgetExhausted;
                                        break 'outer;
                                    }
                                }
                                (
                                    Some(ActiveBlock::Thinking { signature, .. }),
                                    StreamingDelta::Signature(s),
                                ) => {
                                    signature.push_str(&s);
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

                // ---- #378: record generation-span response attributes ----
                //
                // Done before `acc` is consumed so the response model is
                // available, and before the Stage-A / tool-dispatch phases so
                // the span's duration ends at response completion (not after
                // tool execution). Dropping `chat_span` here closes and exports
                // the OTel span for this request.
                if !acc.model.is_empty() {
                    chat_span.record("gen_ai.response.model", acc.model.as_str());
                }
                chat_span.record(
                    "gen_ai.response.finish_reasons",
                    finish_reason_str(turn_stop_reason),
                );
                // ADR 0053: the cache-token breakdown has no stable semconv key,
                // so fold it into the standard input-token count. Anthropic (and
                // the OTLP-facing providers) report `input_tokens` exclusive of
                // cached prompt tokens, so total prompt tokens =
                // input + cache_read + cache_creation.
                let genai_input_tokens = i64::from(turn_usage.input_tokens)
                    + i64::from(turn_usage.cache_read_input_tokens.unwrap_or(0))
                    + i64::from(turn_usage.cache_creation_input_tokens.unwrap_or(0));
                chat_span.record("gen_ai.usage.input_tokens", genai_input_tokens);
                chat_span.record(
                    "gen_ai.usage.output_tokens",
                    i64::from(turn_usage.output_tokens),
                );

                // Assemble the assistant message from the drained accumulator.
                // Built here (before `drop(chat_span)`) so #380 can record the
                // response content onto the still-open generation span.
                let mut assistant_message = acc.into_message();

                // #380: output message CONTENT (semconv-only) — the single
                // assistant message assembled from this response. Recorded
                // before `drop(chat_span)` closes/exports the span. Gated on
                // `OTEL_LOG_USER_PROMPTS` and best-effort like the input side.
                if content_capture_enabled() {
                    if let Some(json) =
                        messages_to_semconv_json(std::slice::from_ref(&assistant_message))
                    {
                        chat_span.record("gen_ai.output.messages", json.as_str());
                    } else {
                        tracing::debug!(
                            "gen_ai.output.messages: serialization failed; content omitted",
                        );
                    }
                }
                drop(chat_span);

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
                if let Some(action) =
                    recovery.on_max_tokens_pre_dispatch(&self.config, turn_stop_reason)
                {
                    match action {
                        recovery::RecoveryAction::RetryTurn => {
                            // Stage-A merge stays in the skeleton: the user is
                            // still billed for the failed attempt.
                            total_usage.merge(turn_usage);
                            continue 'inner;
                        }
                        recovery::RecoveryAction::InjectAndContinue(_)
                        | recovery::RecoveryAction::Surrender(_) => {
                            unreachable!("on_max_tokens_pre_dispatch only yields RetryTurn")
                        }
                    }
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
                // #239 / #244: tool_use_ids of dispatched *file-mutating*
                // (Edit/MultiEdit/Write/NotebookEdit) calls this turn. After
                // dispatch we check which produced a *successful* result to
                // decide whether the turn counts as edit progress. Keyed off
                // Tool::mutates_files (NOT !is_read_only): a side-effecting but
                // non-editing tool like Bash must not reset the no-edit counter,
                // else build-traps (heavy Bash, zero edits) mask the nudge (#244).
                // Denied calls never enter this set (they always error), so a
                // denied edit can't reset the counter.
                let mut file_mutating_tool_ids: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                for (idx, block) in assistant_message.content.iter().enumerate() {
                    if cancel.is_cancelled() {
                        stopped_for = StopCondition::Cancelled;
                        break 'outer;
                    }
                    let ContentBlock::ToolUse(tu) = block else { continue };

                    match self
                        .plan_tool_call(tu, idx, turn_index, &settings.session_id)
                        .await
                    {
                        ToolPlan::Denied {
                            original_index,
                            result,
                        } => {
                            // Emit the denied ToolCallEnd up front, in
                            // assistant-message order.
                            let (result_text, truncated) =
                                capped_result_text(&result.content, STREAM_RESULT_TEXT_CAP);
                            yield TurnEvent::ToolCallEnd {
                                turn_index,
                                tool_use_id: result.tool_use_id.clone(),
                                is_error: true,
                                content: result.content.clone(),
                                result_text,
                                truncated,
                            };
                            plans.push(DispatchPlan::Denied {
                                original_index,
                                result,
                            });
                        }
                        ToolPlan::Allowed {
                            plan,
                            mutates_files,
                        } => {
                            if mutates_files {
                                file_mutating_tool_ids.insert(tu.id.clone());
                            }
                            plans.push(plan);
                        }
                        ToolPlan::Fatal(stop) => {
                            stopped_for = stop;
                            break 'outer;
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
                            let (result_text, truncated) =
                                capped_result_text(&content, STREAM_RESULT_TEXT_CAP);
                            yield TurnEvent::ToolCallEnd {
                                turn_index,
                                tool_use_id: id,
                                is_error,
                                content,
                                result_text,
                                truncated,
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

                // ---- Phase 3: collect + cap results, append to history ----
                let (tool_results, had_successful_edit_this_turn) = self
                    .finalize_tool_results(
                        &mut history,
                        &assistant_message,
                        ordered_results,
                        &file_mutating_tool_ids,
                        &settings.session_id,
                    )
                    .await;

                // #249: classify the turn as "degenerate" before
                // `assistant_message` is moved into the TurnEnd yield. A turn is
                // degenerate when it consumed output tokens yet produced neither
                // a tool call nor any actionable (non-whitespace) text — only a
                // thinking block, or nothing at all. Such a turn ending on a
                // natural stop reason (EndTurn / StopSequence) would otherwise
                // surrender the whole run as a silent success. Tool-use turns,
                // turns with real text, and failure stop reasons (Refusal /
                // ContentFilter / MaxTokens, which have their own handling) are
                // never degenerate.
                let produced_actionable_content = assistant_message.content.iter().any(|b| {
                    matches!(b, ContentBlock::ToolUse(_))
                        || matches!(b, ContentBlock::Text(t) if !t.text.trim().is_empty())
                });
                let turn_was_degenerate = !produced_actionable_content
                    && turn_usage.output_tokens > 0
                    && matches!(
                        turn_stop_reason,
                        StopReason::EndTurn | StopReason::StopSequence
                    );

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
                    let turn_is_failure =
                        recovery.turn_is_failure(&self.config, turn_stop_reason);
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
                    // #420: never let the nudge preempt a *terminal* stop reason.
                    // The nudge `break 'inner`s before `on_stop_reason` runs, so
                    // firing it on a Refusal / ContentFilter (always terminal) or
                    // MaxTokens (recovery-owned) turn would swallow that condition
                    // — the run would silently take another turn instead of
                    // surfacing the correct StopCondition + synthetic message.
                    let stop_is_terminal = matches!(
                        turn_stop_reason,
                        StopReason::Refusal | StopReason::ContentFilter | StopReason::MaxTokens
                    );
                    if threshold > 0
                        && turns_since_last_edit >= threshold
                        && no_edit_nudge_armed
                        && !stop_is_terminal
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
                        recovery.reset_for_new_turn();
                        break 'inner; // take another turn with the nudge in scope
                    }
                }

                // ---- T11: TurnDecision from `after_turn` ----
                match after_turn_decision_for_loop {
                    TurnDecision::Continue => {
                        // Fall through to the natural continue/halt logic.
                    }
                    TurnDecision::ContinueWith(msgs) => {
                        if recovery.forced_continuation_available() {
                            history.extend(msgs);
                            recovery.record_forced_continuation();
                            // Reset Stage A so the forced turn has a fresh
                            // budget-escalation slot.
                            recovery.reset_for_new_turn();
                            break 'inner; // advance to next turn_index
                        }
                        tracing::warn!(
                            forced_continuations = recovery.forced_continuations(),
                            "after_turn ContinueWith ignored (cap reached)"
                        );
                    }
                    TurnDecision::Stop => {
                        stopped_for = StopCondition::HookDenied("after_turn: Stop".into());
                        break 'outer;
                    }
                }

                // ---- #249: empty/degenerate-turn guard ----
                //
                // A turn that reasoned (output tokens > 0) but produced no tool
                // call and no actionable text would otherwise fall through to
                // `on_stop_reason`'s EndTurn arm and surrender the run as a
                // silent success with no work done. Instead, inject one neutral
                // nudge and take another turn — bounded by `empty_turn_nudge_max`
                // consecutive nudges. The streak resets on any productive turn.
                // Skipped when an interactive input source is configured: there,
                // an empty turn naturally hands control back to the operator.
                if turn_was_degenerate
                    && settings.input_source.is_none()
                    && self.config.empty_turn_nudge_max > 0
                {
                    if empty_turn_nudges < self.config.empty_turn_nudge_max {
                        empty_turn_nudges += 1;
                        tracing::warn!(
                            target: "caliban::recovery",
                            turn = turn_index,
                            output_tokens = turn_usage.output_tokens,
                            empty_turn_nudge = empty_turn_nudges,
                            "empty-turn nudge injected (no tool call / actionable text)"
                        );
                        history.push(Message::user_text(EMPTY_TURN_NUDGE));
                        recovery.reset_for_new_turn();
                        break 'inner; // take another turn with the nudge in scope
                    }
                    // Budget exhausted: stop nudging and let the run end via the
                    // natural EndTurn path below.
                    tracing::warn!(
                        target: "caliban::recovery",
                        turn = turn_index,
                        empty_turn_nudges,
                        "empty-turn nudge budget exhausted; ending run"
                    );
                } else if !turn_was_degenerate {
                    // A productive turn resets the consecutive-degenerate streak
                    // so a later stall can be nudged afresh.
                    empty_turn_nudges = 0;
                }

                // ---- Decide whether to continue (Tasks 4–6 dispatch) ----
                //
                // `on_stop_reason` mutates `history` in place for the
                // message-pushing arms (Stage B meta prompt, Refusal /
                // ContentFilter synthetic, input-source resume) and returns the
                // control-flow action. RetryTurn (`continue 'inner`, no slot)
                // and InjectAndContinue (`break 'inner`, advance turn) stay
                // DISTINCT. Note: injected messages land in `history` and so in
                // `RunEnd.final_messages`.
                match recovery
                    .on_stop_reason(
                        turn_stop_reason,
                        &self.config,
                        &mut history,
                        settings.input_source.as_ref(),
                        &cancel,
                    )
                    .await
                {
                    // `on_stop_reason` advances turns or surrenders; it never
                    // asks for a no-slot retry (only the pre-dispatch / context
                    // arms do that).
                    recovery::RecoveryAction::InjectAndContinue(msgs) => {
                        history.extend(msgs);
                        break 'inner;
                    }
                    recovery::RecoveryAction::Surrender(stop) => {
                        stopped_for = stop;
                        break 'outer;
                    }
                    recovery::RecoveryAction::RetryTurn => {
                        unreachable!("on_stop_reason never yields RetryTurn")
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
        };
        Box::pin(SpanStream {
            inner,
            span: run_span,
        })
    }
}
