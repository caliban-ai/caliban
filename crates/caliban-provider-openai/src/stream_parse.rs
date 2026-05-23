//! SSE → `caliban_provider::StreamEvent` parsing for `OpenAI`.

use std::collections::HashMap;

use bytes::Bytes;
use caliban_provider::{
    Error as ProviderError, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::error::OpenAIError;
use crate::schema::events::{NativeChunk, NativeFinishReason};

// ---------------------------------------------------------------------------
// Internal state machine
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StreamState {
    /// Whether we have emitted `MessageStart` yet.
    started: bool,
    /// Whether content block 0 (text) is currently open.
    text_block_open: bool,
    /// The content-block index assigned to the text block.
    text_block_index: u32,
    /// Map from `OpenAI`'s `tool_calls[i].index` → our content-block state.
    tool_blocks: HashMap<u32, ToolBlockState>,
    /// The next content-block index to hand out.
    next_block_index: u32,
}

struct ToolBlockState {
    /// The IR content-block index for this tool call.
    our_index: u32,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Adapt a byte-stream of SSE chunks into a [`caliban_provider::MessageStream`].
///
/// Maintains state across chunks to map `OpenAI`'s "delta only" stream into
/// the IR's content-block model.
#[allow(clippy::too_many_lines)]
pub(crate) fn map_openai_sse_to_events(
    bytes: BoxStream<'static, Result<Bytes, OpenAIError>>,
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
                    Err(ProviderError::adapter(OpenAIError::StreamParse(
                        format!("{e}"),
                    )))?;
                    unreachable!();
                }
            };

            // Skip SSE comments / keep-alive frames with no data.
            if event.data.is_empty() {
                continue;
            }

            // Terminal sentinel.
            if event.data.trim() == "[DONE]" {
                break;
            }

            let chunk: NativeChunk = serde_json::from_str(&event.data).map_err(|e| {
                ProviderError::adapter(OpenAIError::StreamParse(format!(
                    "chunk parse error: {e}; data: {}",
                    event.data
                )))
            })?;

            // 1. First chunk → MessageStart.
            if !state.started {
                state.started = true;
                yield StreamEvent::MessageStart {
                    id: chunk.id.clone(),
                    model: chunk.model.clone(),
                };
            }

            // 2-5. Process the first (and only) choice delta.
            if let Some(choice) = chunk.choices.first() {
                let delta = &choice.delta;

                // -- Text content --
                if let Some(text) = &delta.content {
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
                        delta: StreamingDelta::Text(text.clone()),
                    };
                }

                // -- Refusal (safety layer) — treated as text --
                if let Some(ref_text) = &delta.refusal {
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
                        delta: StreamingDelta::Text(ref_text.clone()),
                    };
                }

                // -- Tool calls --
                for tc in &delta.tool_calls {
                    if !state.tool_blocks.contains_key(&tc.index) {
                        // Close any open text block before opening a tool block.
                        if state.text_block_open {
                            yield StreamEvent::ContentBlockStop {
                                index: state.text_block_index,
                            };
                            state.text_block_open = false;
                        }
                        let id = tc.id.clone().unwrap_or_default();
                        let name = tc
                            .function
                            .as_ref()
                            .and_then(|f| f.name.clone())
                            .unwrap_or_default();
                        let our_index = state.next_block_index;
                        state.next_block_index += 1;
                        state.tool_blocks.insert(tc.index, ToolBlockState { our_index });
                        yield StreamEvent::ContentBlockStart {
                            index: our_index,
                            content_type: StreamingContentType::ToolUse { id, name },
                        };
                    }
                    // Accumulate arguments.
                    if let Some(block) = state.tool_blocks.get(&tc.index)
                        && let Some(func) = &tc.function
                        && let Some(args) = &func.arguments
                        && !args.is_empty()
                    {
                        yield StreamEvent::Delta {
                            index: block.our_index,
                            delta: StreamingDelta::ToolUseInputJson(args.clone()),
                        };
                    }
                }

                // -- Finish --
                if let Some(reason) = choice.finish_reason {
                    // Close any still-open text block.
                    if state.text_block_open {
                        yield StreamEvent::ContentBlockStop {
                            index: state.text_block_index,
                        };
                        state.text_block_open = false;
                    }
                    // Close tool blocks in ascending our_index order.
                    let mut tool_indices: Vec<u32> =
                        state.tool_blocks.values().map(|b| b.our_index).collect();
                    tool_indices.sort_unstable();
                    for idx in tool_indices {
                        yield StreamEvent::ContentBlockStop { index: idx };
                    }
                    state.tool_blocks.clear();

                    let stop_reason = map_finish_reason(reason);
                    let usage_delta = chunk.usage.map(|u| Usage {
                        input_tokens: u.prompt_tokens,
                        output_tokens: u.completion_tokens,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: u
                            .prompt_tokens_details
                            .filter(|d| d.cached_tokens > 0)
                            .map(|d| d.cached_tokens),
                    });
                    yield StreamEvent::MessageDelta {
                        stop_reason: Some(stop_reason),
                        usage_delta,
                    };
                    yield StreamEvent::MessageStop;
                }
            }
        }
    };

    Box::pin(s)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn map_finish_reason(r: NativeFinishReason) -> StopReason {
    match r {
        NativeFinishReason::Stop => StopReason::EndTurn,
        NativeFinishReason::Length => StopReason::MaxTokens,
        NativeFinishReason::ToolCalls | NativeFinishReason::FunctionCall => StopReason::ToolUse,
        NativeFinishReason::ContentFilter => StopReason::ContentFilter,
    }
}
