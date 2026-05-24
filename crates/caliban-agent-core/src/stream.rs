//! Core agent loop driver: `stream_until_done`, `TurnEvent`, and related types.

use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_stream::try_stream;
use caliban_provider::{
    CompletionRequest, ContentBlock, Message, RequestMetadata, Role, StopReason, StreamEvent,
    StreamingContentType, StreamingDelta, TextBlock, ThinkingBlock, ToolResultBlock, ToolUseBlock,
    Usage,
};
use futures::StreamExt as _;
use futures::stream::Stream;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use crate::agent::Agent;
use crate::error::Result;
use crate::hooks::{HookDecision, ToolCtx, TurnCtx};
use crate::retry::with_retry;
use crate::tool::{ToolContext, ToolError};

// ---------------------------------------------------------------------------
// Per-turn timing (TTFT/TBT)
// ---------------------------------------------------------------------------

/// Captures per-turn wall-clock latency markers:
/// - **TTFT** (time-to-first-token): request-sent → first delta arrived.
/// - **TBT** (time-between-tokens): mean inter-delta interval.
#[derive(Debug)]
pub(crate) struct TurnTiming {
    request_sent_at: Instant,
    first_delta_at: Option<Instant>,
    last_delta_at: Option<Instant>,
    delta_count: u32,
}

impl TurnTiming {
    pub(crate) fn start() -> Self {
        Self {
            request_sent_at: Instant::now(),
            first_delta_at: None,
            last_delta_at: None,
            delta_count: 0,
        }
    }

    pub(crate) fn observe_delta(&mut self) {
        let now = Instant::now();
        self.first_delta_at.get_or_insert(now);
        self.last_delta_at = Some(now);
        self.delta_count += 1;
    }

    pub(crate) fn ttft(&self) -> Option<Duration> {
        self.first_delta_at
            .map(|t| t.saturating_duration_since(self.request_sent_at))
    }

    pub(crate) fn tbt(&self) -> Option<Duration> {
        match (self.first_delta_at, self.last_delta_at, self.delta_count) {
            (Some(f), Some(l), n) if n >= 2 => Some(l.saturating_duration_since(f) / (n - 1)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod turn_timing_tests {
    use super::TurnTiming;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn no_delta_means_no_ttft_and_no_tbt() {
        let t = TurnTiming::start();
        assert!(t.ttft().is_none());
        assert!(t.tbt().is_none());
    }

    #[test]
    fn single_delta_gives_ttft_but_no_tbt() {
        let mut t = TurnTiming::start();
        sleep(Duration::from_millis(5));
        t.observe_delta();
        assert!(t.ttft().unwrap() >= Duration::from_millis(4));
        assert!(t.tbt().is_none(), "TBT needs >= 2 deltas");
    }

    #[test]
    fn multi_delta_gives_ttft_and_tbt() {
        let mut t = TurnTiming::start();
        sleep(Duration::from_millis(5));
        t.observe_delta();
        sleep(Duration::from_millis(10));
        t.observe_delta();
        sleep(Duration::from_millis(10));
        t.observe_delta();
        assert!(t.ttft().unwrap() >= Duration::from_millis(4));
        // Two intervals of ~10ms each → mean ~10ms. Wide tolerance for CI.
        let tbt = t.tbt().unwrap();
        assert!(
            tbt >= Duration::from_millis(5) && tbt <= Duration::from_millis(50),
            "tbt was {tbt:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A high-level event emitted by [`Agent::stream_until_done`].
///
/// Consumers can forward these directly to a TUI/CLI renderer.
#[derive(Debug, Clone)]
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
}

/// The reason a multi-turn run stopped.
#[derive(Debug, Clone)]
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
}

/// Boxed, pinned stream of `TurnEvent` results.
pub type TurnEventStream = Pin<Box<dyn Stream<Item = Result<TurnEvent>> + Send + 'static>>;

// ---------------------------------------------------------------------------
// Internal accumulator state for one provider stream
// ---------------------------------------------------------------------------

/// In-progress content block being assembled from stream events.
enum ActiveBlock {
    Text {
        accumulated: String,
    },
    Thinking {
        accumulated: String,
    },
    ToolUse {
        id: String,
        name: String,
        json_buf: String,
    },
}

/// State accumulated while draining one provider `MessageStream`.
struct MessageAccumulator {
    message_id: String,
    model: String,
    blocks: Vec<ContentBlock>,
    active: Vec<Option<ActiveBlock>>,
    stop_reason: Option<StopReason>,
    usage: Usage,
}

impl MessageAccumulator {
    fn new() -> Self {
        Self {
            message_id: String::new(),
            model: String::new(),
            blocks: Vec::new(),
            active: Vec::new(),
            stop_reason: None,
            usage: Usage::default(),
        }
    }

    /// Ensure the `active` and `blocks` vecs are large enough for `index`.
    fn ensure_index(&mut self, index: usize) {
        if self.active.len() <= index {
            self.active.resize_with(index + 1, || None);
            self.blocks.resize(
                index + 1,
                ContentBlock::Text(TextBlock {
                    text: String::new(),
                    cache_control: None,
                }),
            );
        }
    }

    /// Finalize a block at `index` after `ContentBlockStop`.
    fn finalize_block(&mut self, index: usize) {
        let Some(slot) = self.active.get_mut(index) else {
            return;
        };
        let Some(active) = slot.take() else {
            return;
        };
        let block = match active {
            ActiveBlock::Text { accumulated } => ContentBlock::Text(TextBlock {
                text: accumulated,
                cache_control: None,
            }),
            ActiveBlock::Thinking { accumulated } => ContentBlock::Thinking(ThinkingBlock {
                thinking: accumulated,
                signature: None,
            }),
            ActiveBlock::ToolUse { id, name, json_buf } => {
                let input = if json_buf.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&json_buf).unwrap_or(serde_json::json!({}))
                };
                ContentBlock::ToolUse(ToolUseBlock { id, name, input })
            }
        };
        if index < self.blocks.len() {
            self.blocks[index] = block;
        }
    }

    fn into_message(self) -> Message {
        Message {
            role: Role::Assistant,
            content: self.blocks,
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: dispatch a single tool call
// ---------------------------------------------------------------------------

/// Dispatch one tool call: run `before_tool` hook, invoke the tool, run
/// `after_tool` hook. Returns the [`ToolResultBlock`] (possibly synthesized
/// for errors / denials). Returns `Err(StopCondition)` only on cancellation
/// or a hook failure that should abort the run.
#[instrument(skip(agent, input, cancel), fields(tool = tool_name, id = tool_use_id))]
async fn dispatch_tool(
    agent: &Agent,
    turn_index: u32,
    tool_use_id: &str,
    tool_name: &str,
    input: serde_json::Value,
    cancel: &CancellationToken,
) -> std::result::Result<ToolResultBlock, StopCondition> {
    if cancel.is_cancelled() {
        return Err(StopCondition::Cancelled);
    }

    // Keep `input` alive through all hook calls by cloning for the invoke call.
    let tool_ctx = ToolCtx {
        turn_index,
        tool_use_id,
        tool_name,
        input: &input,
    };

    // before_tool hook
    let decision = agent
        .hooks
        .before_tool(&tool_ctx)
        .await
        .map_err(|e| StopCondition::HookDenied(format!("before_tool hook failed: {e}")))?;

    let invoke_result: std::result::Result<Vec<ContentBlock>, ToolError> = match decision {
        HookDecision::Deny(msg) => {
            let content = vec![ContentBlock::Text(TextBlock {
                text: format!("Tool call denied: {msg}"),
                cache_control: None,
            })];
            // Inform the after_tool hook about the denial.
            let denial_err = ToolError::execution(std::io::Error::other(format!("denied: {msg}")));
            if let Err(e) = agent.hooks.after_tool(&tool_ctx, &Err(denial_err)).await {
                tracing::warn!(tool = tool_name, error = %e, "after_tool hook error (non-fatal)");
            }
            return Ok(ToolResultBlock {
                tool_use_id: tool_use_id.to_string(),
                content,
                is_error: true,
            });
        }
        HookDecision::Allow => match agent.tools.get(tool_name) {
            None => Err(ToolError::invalid_input(format!(
                "tool not found: {tool_name}"
            ))),
            Some(tool) => {
                let cx = ToolContext {
                    tool_use_id: tool_use_id.to_string(),
                    cancel: cancel.clone(),
                };
                // Clone `input` so the borrow on `tool_ctx` remains valid for after_tool.
                tool.invoke(input.clone(), cx).await
            }
        },
    };

    // after_tool hook (non-fatal; errors are logged by tracing, not propagated)
    if let Err(e) = agent.hooks.after_tool(&tool_ctx, &invoke_result).await {
        tracing::warn!(tool = tool_name, error = %e, "after_tool hook error (non-fatal)");
    }

    match invoke_result {
        Err(ToolError::Cancelled) => Err(StopCondition::Cancelled),
        Err(e) => Ok(ToolResultBlock {
            tool_use_id: tool_use_id.to_string(),
            content: vec![ContentBlock::Text(TextBlock {
                text: format!("Error: {e}"),
                cache_control: None,
            })],
            is_error: true,
        }),
        Ok(content) => Ok(ToolResultBlock {
            tool_use_id: tool_use_id.to_string(),
            content,
            is_error: false,
        }),
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
    #[allow(clippy::too_many_lines)]
    #[instrument(skip(self, messages, cancel), fields(model = %self.config.model))]
    pub fn stream_until_done(
        self: Arc<Self>,
        messages: Vec<Message>,
        cancel: CancellationToken,
    ) -> TurnEventStream {
        Box::pin(try_stream! {
            let mut history = messages;
            let mut total_usage = Usage::default();
            // Initialise to MaxTurnsReached; overridden on any natural stop or error.
            let mut stopped_for = StopCondition::MaxTurnsReached(self.config.max_turns);
            let max_turns = self.config.max_turns;
            let mut turns_completed: u32 = 0;

            'outer: for turn_index in 0..max_turns {
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

                // ---- Compaction ----
                {
                    let caps = self.provider.capabilities(&self.config.model);
                    match self.compactor.compact(&history, &caps).await {
                        Err(e) => {
                            stopped_for = StopCondition::CompactionFailed(e.to_string());
                            break 'outer;
                        }
                        Ok(Some(new)) => history = new,
                        Ok(None) => {}
                    }
                }

                // ---- Build completion request ----
                let mut req_messages = history.clone();
                let mut req_tools = self.tools.to_caliban_tools();
                if self.prompt_cache {
                    crate::cache::apply_prompt_cache(&mut req_messages, &mut req_tools);
                }
                let req = CompletionRequest {
                    model: self.config.model.clone(),
                    messages: req_messages,
                    tools: req_tools,
                    tool_choice: self.config.tool_choice.clone(),
                    max_tokens: self.config.max_tokens,
                    temperature: self.config.temperature,
                    top_p: self.config.top_p,
                    top_k: None,
                    stop_sequences: self.config.stop_sequences.clone(),
                    thinking: self.config.thinking,
                    metadata: RequestMetadata {
                        user_id: self.config.user_id.clone(),
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

                let mut provider_stream = match stream_result {
                    Ok(s) => s,
                    Err(e) => {
                        if matches!(e, caliban_provider::Error::Cancelled) {
                            stopped_for = StopCondition::Cancelled;
                        } else {
                            stopped_for = StopCondition::ProviderError(e.to_string());
                        }
                        break 'outer;
                    }
                };

                // ---- Drain provider stream ----
                let mut acc = MessageAccumulator::new();

                while let Some(evt_result) = provider_stream.next().await {
                    if cancel.is_cancelled() {
                        stopped_for = StopCondition::Cancelled;
                        break 'outer;
                    }
                    let evt = match evt_result {
                        Ok(e) => e,
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
                let assistant_message = acc.into_message();

                // ---- Dispatch tools sequentially ----
                let mut tool_result_blocks: Vec<ContentBlock> = Vec::new();

                for block in &assistant_message.content {
                    if let ContentBlock::ToolUse(tu) = block {
                        if cancel.is_cancelled() {
                            stopped_for = StopCondition::Cancelled;
                            break 'outer;
                        }

                        let dispatch = dispatch_tool(
                            &self,
                            turn_index,
                            &tu.id,
                            &tu.name,
                            tu.input.clone(),
                            &cancel,
                        )
                        .await;

                        match dispatch {
                            Err(stop) => {
                                stopped_for = stop;
                                break 'outer;
                            }
                            Ok(tool_result) => {
                                let is_error = tool_result.is_error;
                                let content = tool_result.content.clone();
                                let id = tool_result.tool_use_id.clone();
                                yield TurnEvent::ToolCallEnd {
                                    turn_index,
                                    tool_use_id: id,
                                    is_error,
                                    content,
                                };
                                tool_result_blocks
                                    .push(ContentBlock::ToolResult(tool_result));
                            }
                        }
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

                // ---- after_turn hook ----
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
                    let hook_result = self.hooks.after_turn(&turn_ctx, &turn_outcome).await;
                    if let Err(e) = hook_result {
                        stopped_for =
                            StopCondition::HookDenied(format!("after_turn: {e}"));
                        // Emit TurnEnd before aborting so callers have the data.
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
                        target: "caliban::timing",
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
                        target: "caliban::cache",
                        cache_read,
                        cache_creation,
                        turn = turn_index,
                        "prompt cache stats",
                    );
                }

                total_usage.merge(turn_usage);
                turns_completed += 1;

                // ---- Decide whether to continue ----
                if turn_stop_reason != StopReason::ToolUse {
                    stopped_for = StopCondition::EndOfTurn;
                    break 'outer;
                }
                // stop_reason == ToolUse → continue loop
            }

            yield TurnEvent::RunEnd {
                final_messages: history,
                total_usage,
                turn_count: turns_completed,
                stopped_for,
            };
        })
    }
}
