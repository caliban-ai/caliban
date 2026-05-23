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
    /// Whether the text content block (index 0) is currently open.
    text_block_open: bool,
    /// The number of tool-call content blocks opened.
    tool_blocks_opened: u32,
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

                // 2. Text content delta.
                if !resp.message.content.is_empty() {
                    if !state.text_block_open {
                        state.text_block_open = true;
                        yield StreamEvent::ContentBlockStart {
                            index: 0,
                            content_type: StreamingContentType::Text,
                        };
                    }
                    yield StreamEvent::Delta {
                        index: 0,
                        delta: StreamingDelta::Text(resp.message.content.clone()),
                    };
                }

                // 3. Tool calls — emit each as a complete block in one shot.
                // Ollama typically delivers all tool_calls on the final done:true chunk.
                for tc in &resp.message.tool_calls {
                    // Close the text block before opening tool blocks.
                    if state.text_block_open {
                        yield StreamEvent::ContentBlockStop { index: 0 };
                        state.text_block_open = false;
                    }

                    let block_index = 1 + state.tool_blocks_opened;
                    state.tool_blocks_opened += 1;

                    yield StreamEvent::ContentBlockStart {
                        index: block_index,
                        content_type: StreamingContentType::ToolUse {
                            id: format!("tool_{}", block_index - 1),
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

                // 4. Final chunk: close open blocks and emit MessageDelta + MessageStop.
                if resp.done {
                    if state.text_block_open {
                        yield StreamEvent::ContentBlockStop { index: 0 };
                    }

                    let stop_reason: StopReason = map_done_reason(resp.done_reason.as_deref());

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
        }

        // If the stream ended without a done:true line, close any open blocks.
        if state.text_block_open {
            yield StreamEvent::ContentBlockStop { index: 0 };
        }
        yield StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: None,
        };
        yield StreamEvent::MessageStop;
    };

    Box::pin(s)
}
