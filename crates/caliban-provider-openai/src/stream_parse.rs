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

            // Extract usage up-front: the terminal chunk in an OpenAI stream
            // with `stream_options.include_usage: true` arrives as a
            // standalone frame with empty `choices: []` and a populated
            // `usage` object. Pulling this out of the choice guard ensures
            // we surface it whether or not a choice is present. When a
            // choice with a finish_reason is also on this chunk (legacy
            // shape), the usage rides along on that MessageDelta below and
            // we suppress the standalone emission.
            let usage_delta = chunk.usage.map(|u| Usage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: u
                    .prompt_tokens_details
                    .filter(|d| d.cached_tokens > 0)
                    .map(|d| d.cached_tokens),
            });

            // 2-5. Process the first (and only) choice delta.
            let mut usage_emitted_with_finish = false;
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
                    yield StreamEvent::MessageDelta {
                        stop_reason: Some(stop_reason),
                        usage_delta,
                    };
                    yield StreamEvent::MessageStop;
                    usage_emitted_with_finish = usage_delta.is_some();
                }
            }

            // Terminal usage-only chunk: OpenAI emits `choices: []` with a
            // populated `usage` as the last frame before `[DONE]` whenever
            // `stream_options.include_usage` is set. Emit a standalone
            // MessageDelta carrying the usage so the agent's `total_usage`
            // accumulator sees real numbers.
            if usage_delta.is_some() && !usage_emitted_with_finish {
                yield StreamEvent::MessageDelta {
                    stop_reason: None,
                    usage_delta,
                };
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use futures::stream;

    /// Build a `BoxStream<'static, Result<Bytes, OpenAIError>>` from an SSE
    /// transcript string, the same way the transport layer would.
    fn sse_stream(body: &'static str) -> BoxStream<'static, Result<Bytes, OpenAIError>> {
        let chunks: Vec<Result<Bytes, OpenAIError>> = vec![Ok(Bytes::from_static(body.as_bytes()))];
        Box::pin(stream::iter(chunks))
    }

    /// Run a parser stream to completion, collecting all yielded events.
    async fn collect_events(body: &'static str) -> Vec<StreamEvent> {
        let mut stream = map_openai_sse_to_events(sse_stream(body));
        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            events.push(item.expect("stream item should be Ok"));
        }
        events
    }

    /// Extract every `MessageDelta` event from the collected stream.
    fn message_deltas(events: &[StreamEvent]) -> Vec<(Option<StopReason>, Option<Usage>)> {
        events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::MessageDelta {
                    stop_reason,
                    usage_delta,
                } => Some((*stop_reason, *usage_delta)),
                _ => None,
            })
            .collect()
    }

    /// Finding 7: a terminal chunk with `choices: []` and a populated
    /// `usage` object must produce a standalone
    /// `MessageDelta { stop_reason: None, usage_delta: Some(_) }`.
    #[tokio::test]
    async fn terminal_usage_chunk_with_empty_choices_emits_message_delta() {
        // Three chunks: text delta, finish-reason (no usage), then the
        // standalone usage frame. This matches the SSE bytes captured in
        // probe H-3 / H-4 evidence.
        let body = concat!(
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},",
            "\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},",
            "\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-4o\",\"choices\":[],",
            "\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":2,\"total_tokens\":14}}\n\n",
            "data: [DONE]\n\n",
        );

        let events = collect_events(body).await;
        let deltas = message_deltas(&events);

        // Two MessageDelta events: one with the finish reason (no usage,
        // because that chunk had no `usage`), and one standalone with the
        // usage (no stop_reason).
        assert_eq!(deltas.len(), 2, "expected two MessageDelta events");
        assert_eq!(deltas[0].0, Some(StopReason::EndTurn));
        assert!(
            deltas[0].1.is_none(),
            "finish-reason chunk had no usage of its own"
        );
        assert_eq!(
            deltas[1].0, None,
            "standalone usage frame has no stop_reason"
        );
        let usage = deltas[1]
            .1
            .expect("standalone usage delta should be present");
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 2);
    }

    /// Finding 7: final chunk in a multi-chunk stream is usage-only; when
    /// the IR-level accumulator merges deltas, total usage must match.
    #[tokio::test]
    async fn final_usage_only_chunk_merges_into_total() {
        let body = concat!(
            "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"},",
            "\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"},",
            "\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},",
            "\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-4o\",\"choices\":[],",
            "\"usage\":{\"prompt_tokens\":362,\"completion_tokens\":57,\"total_tokens\":419}}\n\n",
            "data: [DONE]\n\n",
        );

        let events = collect_events(body).await;
        // Sum usage_delta across all MessageDelta events (this is what the
        // agent runloop does via `acc.usage.merge(u)`).
        let mut total_input: u32 = 0;
        let mut total_output: u32 = 0;
        for (_stop, usage) in message_deltas(&events) {
            if let Some(u) = usage {
                total_input += u.input_tokens;
                total_output += u.output_tokens;
            }
        }
        assert_eq!(
            total_input, 362,
            "prompt tokens should merge from usage chunk"
        );
        assert_eq!(
            total_output, 57,
            "completion tokens should merge from usage chunk"
        );
    }

    /// Finding 7: a standalone usage chunk with no preceding content (and
    /// no finish-reason chunk) still emits a `MessageDelta`. This is a
    /// degenerate but spec-legal shape: an error-suppressed run where the
    /// only thing the provider returns past `MessageStart` is the usage
    /// frame.
    #[tokio::test]
    async fn standalone_usage_chunk_with_no_content_still_emits() {
        let body = concat!(
            // Open the message with an empty role-only delta so MessageStart
            // is emitted; no finish_reason on this chunk.
            "data: {\"id\":\"c3\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},",
            "\"finish_reason\":null}]}\n\n",
            // Terminal usage-only frame, no choice present.
            "data: {\"id\":\"c3\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-4o\",\"choices\":[],",
            "\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":0,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );

        let events = collect_events(body).await;
        let deltas = message_deltas(&events);

        assert_eq!(
            deltas.len(),
            1,
            "exactly one MessageDelta for the usage frame"
        );
        assert_eq!(
            deltas[0].0, None,
            "no stop_reason on standalone usage frame"
        );
        let usage = deltas[0].1.expect("usage_delta should be populated");
        assert_eq!(usage.input_tokens, 5);
        assert_eq!(usage.output_tokens, 0);
    }

    /// Finding 7 regression guard: legacy shape where the finish-reason
    /// chunk *also* carries `usage` must still emit a single `MessageDelta`
    /// with both `stop_reason` and `usage_delta` populated (and not a
    /// duplicate standalone usage frame).
    #[tokio::test]
    async fn finish_chunk_carrying_usage_still_emits_combined_delta() {
        let body = concat!(
            "data: {\"id\":\"c4\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},",
            "\"finish_reason\":null}]}\n\n",
            // Combined finish+usage on one chunk (legacy/pre-include_usage
            // behavior on some providers / fixtures).
            "data: {\"id\":\"c4\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},",
            "\"finish_reason\":\"stop\"}],",
            "\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":1,\"total_tokens\":8}}\n\n",
            "data: [DONE]\n\n",
        );

        let events = collect_events(body).await;
        let deltas = message_deltas(&events);

        assert_eq!(
            deltas.len(),
            1,
            "exactly one MessageDelta — must not double-emit a standalone usage frame"
        );
        assert_eq!(deltas[0].0, Some(StopReason::EndTurn));
        let usage = deltas[0]
            .1
            .expect("usage_delta should be populated on the combined finish frame");
        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.output_tokens, 1);
    }
}
