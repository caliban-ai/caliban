//! SSE → `caliban_provider::StreamEvent` parsing for Gemini.
//!
//! Each `data:` SSE line is a complete `NativeResponse` chunk (same shape as
//! non-streaming). Parts accumulate across chunks; function calls are emitted
//! atomically. The final chunk has `finishReason` set.

use bytes::Bytes;
use caliban_provider::{
    Error as ProviderError, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::error::GoogleError;
use crate::ir_convert::map_finish_reason;
use crate::schema::events::NativeResponse;
use crate::schema::request::NativePart;
use crate::schema::response::NativeFinishReason;

// ---------------------------------------------------------------------------
// Internal state machine
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StreamState {
    /// Whether we have emitted `MessageStart` yet.
    started: bool,
    /// Whether the text content block is currently open.
    text_block_open: bool,
    /// The content-block index for the text block.
    text_block_index: u32,
    /// Next content-block index to assign.
    next_block_index: u32,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Adapt a byte-stream of Gemini SSE chunks into a [`caliban_provider::MessageStream`].
///
/// Each `data:` line is a full `NativeResponse` JSON object. Parts are emitted
/// as content block events. Function calls are emitted atomically. The final
/// chunk (with `finishReason` set) emits `MessageDelta` + `MessageStop`.
#[allow(clippy::too_many_lines)]
pub(crate) fn map_gemini_sse_to_events(
    bytes: BoxStream<'static, Result<Bytes, GoogleError>>,
) -> caliban_provider::MessageStream {
    let mut state = StreamState::default();

    let sse = bytes.eventsource();
    let s = async_stream::try_stream! {
        futures::pin_mut!(sse);
        while let Some(item) = sse.next().await {
            let event = match item {
                Ok(e) => e,
                Err(eventsource_stream::EventStreamError::Transport(e)) => {
                    Err(ProviderError::network(e))?;
                    unreachable!();
                }
                Err(e) => {
                    Err(ProviderError::adapter(GoogleError::StreamParse(
                        format!("{e}"),
                    )))?;
                    unreachable!();
                }
            };

            // Skip SSE comments / keep-alive frames with no data.
            if event.data.is_empty() {
                continue;
            }

            let chunk: NativeResponse = serde_json::from_str(&event.data).map_err(|e| {
                ProviderError::adapter(GoogleError::StreamParse(format!(
                    "chunk parse error: {e}; data: {}",
                    event.data
                )))
            })?;

            // 1. First chunk → MessageStart.
            if !state.started {
                state.started = true;
                yield StreamEvent::MessageStart {
                    id: chunk.model_version.clone(),
                    model: chunk.model_version.clone(),
                };
            }

            // Get the candidate (skip if none).
            let Some(candidate) = chunk.candidates.first() else {
                continue;
            };

            let finish_reason = candidate.finish_reason;

            // 2. Process parts in this chunk.
            for part in &candidate.content.parts {
                match part {
                    NativePart::Text(s) => {
                        if s.is_empty() {
                            continue;
                        }
                        // Open a text block if not already open.
                        if !state.text_block_open {
                            state.text_block_index = state.next_block_index;
                            state.next_block_index += 1;
                            state.text_block_open = true;
                            yield StreamEvent::ContentBlockStart {
                                index: state.text_block_index,
                                content_type: StreamingContentType::Text,
                            };
                        }
                        yield StreamEvent::Delta {
                            index: state.text_block_index,
                            delta: StreamingDelta::Text(s.clone()),
                        };
                    }
                    NativePart::FunctionCall(fc) => {
                        // Close any open text block first.
                        if state.text_block_open {
                            yield StreamEvent::ContentBlockStop {
                                index: state.text_block_index,
                            };
                            state.text_block_open = false;
                        }

                        // Gemini emits function calls atomically: Start → Delta(full args) → Stop.
                        let block_index = state.next_block_index;
                        state.next_block_index += 1;

                        let tool_id = format!("toolu_{block_index}");
                        yield StreamEvent::ContentBlockStart {
                            index: block_index,
                            content_type: StreamingContentType::ToolUse {
                                id: tool_id,
                                name: fc.name.clone(),
                            },
                        };

                        let args_json = serde_json::to_string(&fc.args).map_err(|e| {
                            ProviderError::adapter(GoogleError::StreamParse(format!(
                                "failed to serialize function args: {e}"
                            )))
                        })?;
                        yield StreamEvent::Delta {
                            index: block_index,
                            delta: StreamingDelta::ToolUseInputJson(args_json),
                        };

                        yield StreamEvent::ContentBlockStop { index: block_index };
                    }
                    // InlineData, FileData, and FunctionResponse in response parts are ignored.
                    NativePart::InlineData(_)
                    | NativePart::FileData(_)
                    | NativePart::FunctionResponse(_) => {}
                }
            }

            // 3. Final chunk: close open blocks and emit MessageDelta + MessageStop.
            let is_final = finish_reason.is_some()
                && !matches!(
                    finish_reason,
                    Some(NativeFinishReason::FinishReasonUnspecified)
                );

            if is_final {
                if state.text_block_open {
                    yield StreamEvent::ContentBlockStop {
                        index: state.text_block_index,
                    };
                }

                let stop_reason: StopReason = map_finish_reason(finish_reason);
                let usage_delta = Some(Usage {
                    input_tokens: chunk.usage_metadata.prompt_token_count,
                    output_tokens: chunk.usage_metadata.candidates_token_count,
                    // See ir_convert.rs note: Gemini context caching is a
                    // separate API resource; not yet implemented here.
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                });

                yield StreamEvent::MessageDelta {
                    stop_reason: Some(stop_reason),
                    usage_delta,
                };
                yield StreamEvent::MessageStop;
                return;
            }
        }

        // Stream ended without a final chunk — close any open blocks.
        if state.text_block_open {
            yield StreamEvent::ContentBlockStop {
                index: state.text_block_index,
            };
        }
        yield StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: None,
        };
        yield StreamEvent::MessageStop;
    };

    Box::pin(s)
}
