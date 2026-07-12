//! NDJSON → `caliban_provider::StreamEvent` parsing for Ollama.
//!
//! Ollama streams one JSON object per line. Each line has the same shape as
//! the non-streaming `NativeResponse`. Intermediate lines have `done: false`
//! with partial `message.content`. The final line has `done: true` with
//! `done_reason` and token-count fields populated.

use bytes::Bytes;
use caliban_provider::{
    Error as ProviderError, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::error::OllamaError;
use crate::ir_convert::map_done_reason;
use crate::schema::response::NativeResponse;

// ---------------------------------------------------------------------------
// Internal state machine
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StreamState {
    /// Whether we have emitted `MessageStart` yet.
    started: bool,
    /// Whether a text content block is currently open, and its index.
    text_block: Option<u32>,
    /// Whether a thinking content block is currently open, and its index.
    thinking_block: Option<u32>,
    /// Number of tool-call content blocks emitted so far (for synthesizing
    /// fallback `tool_{n}` ids when the wire omits one).
    tool_calls_emitted: u32,
    /// Whether at least one `tool_call` was emitted across the whole message;
    /// used to override `done_reason: "stop"` → `ToolUse` so the agent loop
    /// continues.
    saw_tool_calls: bool,
    /// Monotonically increasing block index allocator.
    next_block_index: u32,
}

impl StreamState {
    fn alloc_block_index(&mut self) -> u32 {
        let i = self.next_block_index;
        self.next_block_index += 1;
        i
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Adapt a byte-stream of NDJSON chunks into a [`caliban_provider::MessageStream`].
///
/// Each newline-delimited JSON object is a `NativeResponse`. Text content from
/// non-final chunks accumulates as `Delta` events. Tool calls (typically only
/// in the final `done: true` chunk) each get a `ContentBlockStart` + `Delta` +
/// `ContentBlockStop` emitted in sequence.
/// Max size of the incomplete-line buffer (#424). A newline-less stream would
/// otherwise grow `line_buf` without bound; 8 MiB is far above any real NDJSON
/// line yet caps a memory-exhaustion attempt.
const MAX_LINE_BUF: usize = 8 * 1024 * 1024;

#[allow(clippy::too_many_lines)]
pub(crate) fn map_ndjson_to_events(
    bytes: BoxStream<'static, Result<Bytes, OllamaError>>,
) -> caliban_provider::MessageStream {
    let mut state = StreamState::default();
    // Buffer for incomplete lines across chunk boundaries.
    let mut line_buf = String::new();

    let s = async_stream::try_stream! {
        futures::pin_mut!(bytes);
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk.map_err(ProviderError::from)?;
            let text = std::str::from_utf8(&chunk).map_err(|e| {
                ProviderError::adapter(OllamaError::StreamParse(format!("UTF-8 error: {e}")))
            })?;
            line_buf.push_str(text);

            // Process all complete lines in the buffer.
            while let Some(nl_pos) = line_buf.find('\n') {
                let line = line_buf[..nl_pos].trim().to_string();
                line_buf.drain(..=nl_pos);

                if line.is_empty() {
                    continue;
                }

                let resp: NativeResponse = serde_json::from_str(&line).map_err(|e| {
                    ProviderError::adapter(OllamaError::StreamParse(format!(
                        "chunk parse error: {e}; line: {line}"
                    )))
                })?;

                // 1. First chunk → MessageStart.
                if !state.started {
                    state.started = true;
                    yield StreamEvent::MessageStart {
                        id: resp.created_at.clone(),
                        model: resp.model.clone(),
                    };
                }

                // 2. Reasoning (thinking) delta — emitted before text so the
                //    IR reflects the model's natural order. If a text block is
                //    somehow already open (text appearing before thinking in a
                //    chunk), close it first; reasoning and content may interleave.
                if let Some(thinking) = resp
                    .message
                    .thinking
                    .as_ref()
                    .filter(|t| !t.is_empty())
                {
                    if let Some(idx) = state.text_block.take() {
                        yield StreamEvent::ContentBlockStop { index: idx };
                    }
                    let idx = if let Some(idx) = state.thinking_block {
                        idx
                    } else {
                        let idx = state.alloc_block_index();
                        state.thinking_block = Some(idx);
                        yield StreamEvent::ContentBlockStart {
                            index: idx,
                            content_type: StreamingContentType::Thinking,
                        };
                        idx
                    };
                    yield StreamEvent::Delta {
                        index: idx,
                        delta: StreamingDelta::Thinking(thinking.clone()),
                    };
                }

                // 3. Text content delta.
                if !resp.message.content.is_empty() {
                    if let Some(idx) = state.thinking_block.take() {
                        yield StreamEvent::ContentBlockStop { index: idx };
                    }
                    let idx = if let Some(idx) = state.text_block {
                        idx
                    } else {
                        let idx = state.alloc_block_index();
                        state.text_block = Some(idx);
                        yield StreamEvent::ContentBlockStart {
                            index: idx,
                            content_type: StreamingContentType::Text,
                        };
                        idx
                    };
                    yield StreamEvent::Delta {
                        index: idx,
                        delta: StreamingDelta::Text(resp.message.content.clone()),
                    };
                }

                // 4. Tool calls — emit each as a complete block in one shot.
                //    Ollama typically delivers all tool_calls on the final done:true chunk.
                for tc in &resp.message.tool_calls {
                    // Close any open content blocks before opening a tool block.
                    if let Some(idx) = state.thinking_block.take() {
                        yield StreamEvent::ContentBlockStop { index: idx };
                    }
                    if let Some(idx) = state.text_block.take() {
                        yield StreamEvent::ContentBlockStop { index: idx };
                    }

                    let block_index = state.alloc_block_index();
                    let call_idx = state.tool_calls_emitted;
                    state.tool_calls_emitted += 1;
                    state.saw_tool_calls = true;

                    // Preserve the wire id if Ollama emitted one (newer builds
                    // do, e.g. "call_xoh1i8k9"); fall back to a synthesized
                    // `tool_{n}` for older builds that omit it.
                    let id = tc
                        .id
                        .clone()
                        .unwrap_or_else(|| format!("tool_{call_idx}"));

                    yield StreamEvent::ContentBlockStart {
                        index: block_index,
                        content_type: StreamingContentType::ToolUse {
                            id,
                            name: tc.function.name.clone(),
                        },
                    };

                    // Serialize arguments as a JSON string fragment.
                    let args_json = serde_json::to_string(&tc.function.arguments).map_err(|e| {
                        ProviderError::adapter(OllamaError::StreamParse(format!(
                            "failed to serialize tool arguments: {e}"
                        )))
                    })?;
                    yield StreamEvent::Delta {
                        index: block_index,
                        delta: StreamingDelta::ToolUseInputJson(args_json),
                    };

                    yield StreamEvent::ContentBlockStop { index: block_index };
                }

                // 5. Final chunk: close open blocks and emit MessageDelta + MessageStop.
                if resp.done {
                    if let Some(idx) = state.thinking_block.take() {
                        yield StreamEvent::ContentBlockStop { index: idx };
                    }
                    if let Some(idx) = state.text_block.take() {
                        yield StreamEvent::ContentBlockStop { index: idx };
                    }

                    // Ollama reports `done_reason: "stop"` even on tool-calling
                    // turns; the presence of any tool_call across the message
                    // is the authoritative signal to keep the agent loop alive.
                    let stop_reason: StopReason = if state.saw_tool_calls {
                        StopReason::ToolUse
                    } else {
                        map_done_reason(resp.done_reason.as_deref())
                    };

                    yield StreamEvent::MessageDelta {
                        stop_reason: Some(stop_reason),
                        usage_delta: Some(Usage {
                            input_tokens: resp.prompt_eval_count,
                            output_tokens: resp.eval_count,
                            cache_creation_input_tokens: None,
                            cache_read_input_tokens: None,
                        }),
                    };
                    yield StreamEvent::MessageStop;
                    // Stop processing further chunks.
                    return;
                }
            }
            // #424: bound the incomplete-line buffer — a server (or broken
            // upstream) streaming bytes without a newline would otherwise grow
            // `line_buf` without limit (memory-exhaustion).
            if line_buf.len() > MAX_LINE_BUF {
                Err(ProviderError::adapter(OllamaError::StreamParse(format!(
                    "unterminated NDJSON line exceeds {MAX_LINE_BUF} bytes"
                ))))?;
            }
        }

        // The stream ended without a `done:true` marker (that path `return`s
        // above). Close any open blocks first.
        if let Some(idx) = state.thinking_block.take() {
            yield StreamEvent::ContentBlockStop { index: idx };
        }
        if let Some(idx) = state.text_block.take() {
            yield StreamEvent::ContentBlockStop { index: idx };
        }
        if state.saw_tool_calls {
            // Tool calls streamed → the turn legitimately continues.
            yield StreamEvent::MessageDelta {
                stop_reason: Some(StopReason::ToolUse),
                usage_delta: None,
            };
            yield StreamEvent::MessageStop;
        } else {
            // #424: no done marker and no tool calls means the generation was cut
            // short (load-shed / proxy close). Surface it as an interrupted
            // stream so the agent loop can retry, rather than presenting the
            // truncated output as a clean EndTurn.
            Err(ProviderError::adapter(OllamaError::StreamParse(
                "stream ended before a done marker (truncated generation)".into(),
            )))?;
        }
    };

    Box::pin(s)
}
