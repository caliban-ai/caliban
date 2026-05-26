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
    /// Whether a text content block is currently open.
    text_block_open: bool,
    /// The content-block index of the currently open text block (valid only
    /// while `text_block_open == true`). Each open/close cycle gets a fresh
    /// index since text and reasoning may interleave.
    text_block_index: u32,
    /// Whether a thinking (reasoning) content block is currently open.
    thinking_block_open: bool,
    /// The content-block index of the currently open thinking block (valid
    /// only while `thinking_block_open == true`).
    thinking_block_index: u32,
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

            let chunk: NativeChunk = match serde_json::from_str(&event.data) {
                Ok(c) => c,
                Err(e) => {
                    // Fallback: some OpenAI-compatible servers (notably LM
                    // Studio for context-overflow, also seen on Ollama and
                    // vLLM for some failure modes) return HTTP 200 with an
                    // `{"error": {"message": "..."}}` JSON object inside
                    // the SSE body rather than a non-2xx status. If the
                    // chunk fails NativeChunk deserialization, try parsing
                    // it as that error envelope and surface the upstream
                    // message verbatim instead of the layered chunk-parse
                    // error. See `docs/2026-05-25-lmstudio-probe-findings.md`
                    // Finding 12.
                    if let Some(msg) = extract_upstream_error(&event.data) {
                        Err(ProviderError::from(OpenAIError::UpstreamError(msg)))?;
                        unreachable!();
                    }
                    Err(ProviderError::adapter(OpenAIError::StreamParse(format!(
                        "chunk parse error: {e}; data: {}",
                        event.data
                    ))))?;
                    unreachable!();
                }
            };

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

                // -- Reasoning content (Qwen / DeepSeek reasoning families) --
                //
                // Reasoning deltas open a `Thinking` block. If a text block
                // is currently open, close it first so the IR stays
                // well-formed (a single delta may carry both fields, or
                // reasoning may resume after a content burst).
                if let Some(reasoning) = &delta.reasoning_content
                    && !reasoning.is_empty()
                {
                    if state.text_block_open {
                        yield StreamEvent::ContentBlockStop {
                            index: state.text_block_index,
                        };
                        state.text_block_open = false;
                    }
                    if !state.thinking_block_open {
                        state.thinking_block_index = state.next_block_index;
                        state.next_block_index += 1;
                        state.thinking_block_open = true;
                        yield StreamEvent::ContentBlockStart {
                            index: state.thinking_block_index,
                            content_type: StreamingContentType::Thinking,
                        };
                    }
                    yield StreamEvent::Delta {
                        index: state.thinking_block_index,
                        delta: StreamingDelta::Thinking(reasoning.clone()),
                    };
                }

                // -- Text content --
                if let Some(text) = &delta.content {
                    // Close any open thinking block before opening text;
                    // reasoning and content may interleave (DeepSeek) so we
                    // expect close-then-reopen, not a single contiguous
                    // reasoning span.
                    if state.thinking_block_open {
                        yield StreamEvent::ContentBlockStop {
                            index: state.thinking_block_index,
                        };
                        state.thinking_block_open = false;
                    }
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
                    if state.thinking_block_open {
                        yield StreamEvent::ContentBlockStop {
                            index: state.thinking_block_index,
                        };
                        state.thinking_block_open = false;
                    }
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
                        // Close any open text or thinking block before
                        // opening a tool block.
                        if state.text_block_open {
                            yield StreamEvent::ContentBlockStop {
                                index: state.text_block_index,
                            };
                            state.text_block_open = false;
                        }
                        if state.thinking_block_open {
                            yield StreamEvent::ContentBlockStop {
                                index: state.thinking_block_index,
                            };
                            state.thinking_block_open = false;
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
                    // Close any still-open thinking block.
                    if state.thinking_block_open {
                        yield StreamEvent::ContentBlockStop {
                            index: state.thinking_block_index,
                        };
                        state.thinking_block_open = false;
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

/// Attempt to recover an upstream error message from an SSE data frame that
/// failed `NativeChunk` deserialization. Recognized shapes (all carry a
/// `{"error": {"message": "..."}}` envelope, with or without sibling
/// fields like `type` and `code`):
///
/// ```json
/// {"error":{"message":"The number of tokens to keep..."}}
/// {"error":{"message":"oops","type":"invalid_request_error","code":"foo"}}
/// ```
///
/// Returns the inner `message` string when the shape matches, `None`
/// otherwise. Used by the SSE parser to surface upstream-side problems
/// (LM Studio context overflow, Ollama / vLLM error payloads, etc.) as a
/// readable [`OpenAIError::UpstreamError`] instead of a nested chunk-parse
/// error. See `docs/2026-05-25-lmstudio-probe-findings.md` Finding 12.
fn extract_upstream_error(data: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        error: Inner,
    }
    #[derive(serde::Deserialize)]
    struct Inner {
        message: String,
    }
    serde_json::from_str::<Envelope>(data)
        .ok()
        .map(|e| e.error.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::events::NativeDelta;
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

    /// Build an SSE byte stream from a list of `data: ...` chunk strings.
    /// Each entry becomes a single SSE event terminated by a blank line.
    fn sse_stream_from_chunks(chunks: &[&str]) -> BoxStream<'static, Result<Bytes, OpenAIError>> {
        let mut buf = String::new();
        for e in chunks {
            buf.push_str("data: ");
            buf.push_str(e);
            buf.push_str("\n\n");
        }
        let bytes = Bytes::from(buf);
        Box::pin(stream::iter(vec![Ok::<_, OpenAIError>(bytes)]))
    }

    /// Construct one chat.completion.chunk JSON from a delta JSON snippet and
    /// optional finish reason. Used by the F2 reasoning-content tests.
    fn chunk(delta_json: &str, finish: Option<&str>) -> String {
        let finish_field = finish.map_or_else(|| "null".to_string(), |r| format!("\"{r}\""));
        format!(
            r#"{{"id":"chatcmpl-test","object":"chat.completion.chunk","created":1700000000,"model":"qwen3.5-9b-mlx","choices":[{{"index":0,"delta":{delta_json},"finish_reason":{finish_field}}}]}}"#
        )
    }

    async fn collect_events_from_chunks(chunks: Vec<&str>) -> Vec<StreamEvent> {
        let mut s = map_openai_sse_to_events(sse_stream_from_chunks(&chunks));
        let mut out = Vec::new();
        while let Some(item) = s.next().await {
            out.push(item.expect("stream item"));
        }
        out
    }

    #[test]
    fn native_delta_deserializes_reasoning_content() {
        // Reasoning-only delta deserializes with reasoning_content populated
        // and content absent.
        let j = r#"{"reasoning_content":"Let me think..."}"#;
        let d: NativeDelta = serde_json::from_str(j).unwrap();
        assert_eq!(d.reasoning_content.as_deref(), Some("Let me think..."));
        assert!(d.content.is_none());

        // Mixed delta: both fields populated in the same chunk.
        let j2 = r#"{"content":"Hello","reasoning_content":"thinking"}"#;
        let d2: NativeDelta = serde_json::from_str(j2).unwrap();
        assert_eq!(d2.content.as_deref(), Some("Hello"));
        assert_eq!(d2.reasoning_content.as_deref(), Some("thinking"));

        // Absence is fine — round-trips to None and does not appear in output.
        let j3 = r#"{"content":"hi"}"#;
        let d3: NativeDelta = serde_json::from_str(j3).unwrap();
        assert!(d3.reasoning_content.is_none());
        let back = serde_json::to_string(&d3).unwrap();
        assert!(!back.contains("reasoning_content"));
    }

    #[tokio::test]
    async fn reasoning_only_stream_emits_thinking_block() {
        // A stream with only reasoning_content (no content) should produce:
        // MessageStart, CBStart(Thinking), Delta(Thinking), CBStop, MessageDelta(Length), MessageStop.
        let events = [
            chunk(r#"{"role":"assistant"}"#, None),
            chunk(r#"{"reasoning_content":"Thinking..."}"#, None),
            chunk(r#"{"reasoning_content":" more."}"#, None),
            chunk("{}", Some("length")),
        ];
        let events: Vec<&str> = events.iter().map(String::as_str).collect();
        let got = collect_events_from_chunks(events).await;

        assert!(matches!(got[0], StreamEvent::MessageStart { .. }));
        match &got[1] {
            StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamingContentType::Thinking,
            } => {}
            other => panic!("expected ContentBlockStart(Thinking, 0), got {other:?}"),
        }
        match &got[2] {
            StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::Thinking(t),
            } if t == "Thinking..." => {}
            other => panic!("expected Thinking delta, got {other:?}"),
        }
        match &got[3] {
            StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::Thinking(t),
            } if t == " more." => {}
            other => panic!("expected Thinking delta, got {other:?}"),
        }
        match &got[4] {
            StreamEvent::ContentBlockStop { index: 0 } => {}
            other => panic!("expected ContentBlockStop(0), got {other:?}"),
        }
        match &got[5] {
            StreamEvent::MessageDelta {
                stop_reason: Some(StopReason::MaxTokens),
                ..
            } => {}
            other => panic!("expected MessageDelta(MaxTokens), got {other:?}"),
        }
        assert!(matches!(got[6], StreamEvent::MessageStop));
    }

    #[tokio::test]
    async fn reasoning_then_content_closes_thinking_before_text() {
        // Reasoning deltas, then content deltas, then finish:
        // Thinking block must close before the Text block opens.
        let events = [
            chunk(r#"{"role":"assistant"}"#, None),
            chunk(r#"{"reasoning_content":"R1"}"#, None),
            chunk(r#"{"content":"Hello"}"#, None),
            chunk("{}", Some("stop")),
        ];
        let events: Vec<&str> = events.iter().map(String::as_str).collect();
        let got = collect_events_from_chunks(events).await;

        // Filter to block-level events for clarity.
        let blocks: Vec<&StreamEvent> = got
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    StreamEvent::ContentBlockStart { .. }
                        | StreamEvent::ContentBlockStop { .. }
                        | StreamEvent::Delta { .. }
                )
            })
            .collect();

        // Expected sequence:
        //   CBStart(Thinking, 0)
        //   Delta(Thinking, 0)
        //   CBStop(0)
        //   CBStart(Text, 1)
        //   Delta(Text, 1)
        //   CBStop(1)
        match blocks[0] {
            StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamingContentType::Thinking,
            } => {}
            other => panic!("[0] expected CBStart(Thinking,0); got {other:?}"),
        }
        match blocks[1] {
            StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::Thinking(_),
            } => {}
            other => panic!("[1] expected Delta(Thinking,0); got {other:?}"),
        }
        match blocks[2] {
            StreamEvent::ContentBlockStop { index: 0 } => {}
            other => panic!("[2] expected CBStop(0); got {other:?}"),
        }
        match blocks[3] {
            StreamEvent::ContentBlockStart {
                index: 1,
                content_type: StreamingContentType::Text,
            } => {}
            other => panic!("[3] expected CBStart(Text,1); got {other:?}"),
        }
        match blocks[4] {
            StreamEvent::Delta {
                index: 1,
                delta: StreamingDelta::Text(_),
            } => {}
            other => panic!("[4] expected Delta(Text,1); got {other:?}"),
        }
        match blocks[5] {
            StreamEvent::ContentBlockStop { index: 1 } => {}
            other => panic!("[5] expected CBStop(1); got {other:?}"),
        }
        assert_eq!(blocks.len(), 6);
    }

    #[tokio::test]
    async fn content_then_reasoning_closes_text_before_reopening_thinking() {
        // Some providers (DeepSeek) interleave: content first, then reasoning.
        // The text block must close before a new thinking block opens.
        let events = [
            chunk(r#"{"role":"assistant"}"#, None),
            chunk(r#"{"content":"Hi"}"#, None),
            chunk(r#"{"reasoning_content":"second thoughts"}"#, None),
            chunk("{}", Some("stop")),
        ];
        let events: Vec<&str> = events.iter().map(String::as_str).collect();
        let got = collect_events_from_chunks(events).await;
        let blocks: Vec<&StreamEvent> = got
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    StreamEvent::ContentBlockStart { .. }
                        | StreamEvent::ContentBlockStop { .. }
                        | StreamEvent::Delta { .. }
                )
            })
            .collect();

        match blocks[0] {
            StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamingContentType::Text,
            } => {}
            other => panic!("[0] expected CBStart(Text,0); got {other:?}"),
        }
        match blocks[1] {
            StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::Text(_),
            } => {}
            other => panic!("[1] expected Delta(Text,0); got {other:?}"),
        }
        match blocks[2] {
            StreamEvent::ContentBlockStop { index: 0 } => {}
            other => panic!("[2] expected CBStop(0); got {other:?}"),
        }
        match blocks[3] {
            StreamEvent::ContentBlockStart {
                index: 1,
                content_type: StreamingContentType::Thinking,
            } => {}
            other => panic!("[3] expected CBStart(Thinking,1); got {other:?}"),
        }
        match blocks[4] {
            StreamEvent::Delta {
                index: 1,
                delta: StreamingDelta::Thinking(_),
            } => {}
            other => panic!("[4] expected Delta(Thinking,1); got {other:?}"),
        }
        match blocks[5] {
            StreamEvent::ContentBlockStop { index: 1 } => {}
            other => panic!("[5] expected CBStop(1); got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multiple_interleaved_segments_produce_balanced_pairs() {
        // R, T, R, T, R — five alternating segments. Each open must close
        // before the next opens; finish closes the last one.
        let events = [
            chunk(r#"{"role":"assistant"}"#, None),
            chunk(r#"{"reasoning_content":"r1"}"#, None),
            chunk(r#"{"content":"t1"}"#, None),
            chunk(r#"{"reasoning_content":"r2"}"#, None),
            chunk(r#"{"content":"t2"}"#, None),
            chunk(r#"{"reasoning_content":"r3"}"#, None),
            chunk("{}", Some("stop")),
        ];
        let events: Vec<&str> = events.iter().map(String::as_str).collect();
        let got = collect_events_from_chunks(events).await;

        // Count starts and stops; they must balance per type.
        let mut thinking_opens = 0;
        let mut text_opens = 0;
        let mut stops = 0;
        // Track running open/close per index to ensure no double-open
        // and no stop without a prior start.
        let mut open_indices: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for e in &got {
            match e {
                StreamEvent::ContentBlockStart {
                    index,
                    content_type: StreamingContentType::Thinking,
                } => {
                    assert!(
                        open_indices.insert(*index),
                        "double-open of thinking idx {index}"
                    );
                    thinking_opens += 1;
                }
                StreamEvent::ContentBlockStart {
                    index,
                    content_type: StreamingContentType::Text,
                } => {
                    assert!(
                        open_indices.insert(*index),
                        "double-open of text idx {index}"
                    );
                    text_opens += 1;
                }
                StreamEvent::ContentBlockStop { index } => {
                    assert!(
                        open_indices.remove(index),
                        "stop without prior open at idx {index}"
                    );
                    stops += 1;
                }
                _ => {}
            }
        }
        assert_eq!(thinking_opens, 3, "expected 3 thinking opens");
        assert_eq!(text_opens, 2, "expected 2 text opens");
        assert_eq!(stops, 5, "expected 5 stops (one per open)");
        assert!(
            open_indices.is_empty(),
            "all blocks should be closed by finish"
        );
    }

    #[tokio::test]
    async fn no_reasoning_stream_parses_identically_to_before() {
        // Backward-compat: a vanilla content-only stream produces the same
        // event sequence it always did, with no spurious thinking events.
        let events = [
            chunk(r#"{"role":"assistant"}"#, None),
            chunk(r#"{"content":"Hello"}"#, None),
            chunk(r#"{"content":"!"}"#, None),
            chunk("{}", Some("stop")),
        ];
        let events: Vec<&str> = events.iter().map(String::as_str).collect();
        let got = collect_events_from_chunks(events).await;

        // No Thinking content type and no Thinking delta should appear.
        for e in &got {
            match e {
                StreamEvent::ContentBlockStart {
                    content_type: StreamingContentType::Thinking,
                    ..
                } => panic!("unexpected Thinking block in pure-text stream: {e:?}"),
                StreamEvent::Delta {
                    delta: StreamingDelta::Thinking(_),
                    ..
                } => panic!("unexpected Thinking delta in pure-text stream: {e:?}"),
                _ => {}
            }
        }

        // Expected event sequence: MessageStart, CBStart(Text,0),
        // Delta(Text), Delta(Text), CBStop(0), MessageDelta, MessageStop.
        assert!(matches!(got[0], StreamEvent::MessageStart { .. }));
        assert!(matches!(
            got[1],
            StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamingContentType::Text
            }
        ));
        assert!(matches!(
            got[2],
            StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::Text(_)
            }
        ));
        assert!(matches!(
            got[3],
            StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::Text(_)
            }
        ));
        assert!(matches!(got[4], StreamEvent::ContentBlockStop { index: 0 }));
        assert!(matches!(got[5], StreamEvent::MessageDelta { .. }));
        assert!(matches!(got[6], StreamEvent::MessageStop));
        assert_eq!(got.len(), 7);
    }

    #[tokio::test]
    async fn reasoning_then_tool_call_closes_thinking_before_tool() {
        // If a tool call arrives while a thinking block is open, the
        // thinking block must close before the tool block opens.
        let events = [
            chunk(r#"{"role":"assistant"}"#, None),
            chunk(r#"{"reasoning_content":"deciding to call Foo"}"#, None),
            chunk(
                r#"{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"Foo","arguments":"{}"}}]}"#,
                None,
            ),
            chunk("{}", Some("tool_calls")),
        ];
        let events: Vec<&str> = events.iter().map(String::as_str).collect();
        let got = collect_events_from_chunks(events).await;

        let blocks: Vec<&StreamEvent> = got
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    StreamEvent::ContentBlockStart { .. } | StreamEvent::ContentBlockStop { .. }
                )
            })
            .collect();

        // Expect: CBStart(Thinking,0), CBStop(0), CBStart(ToolUse,1), CBStop(1).
        match blocks[0] {
            StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamingContentType::Thinking,
            } => {}
            other => panic!("[0] expected CBStart(Thinking,0); got {other:?}"),
        }
        match blocks[1] {
            StreamEvent::ContentBlockStop { index: 0 } => {}
            other => panic!("[1] expected CBStop(0); got {other:?}"),
        }
        match blocks[2] {
            StreamEvent::ContentBlockStart {
                index: 1,
                content_type: StreamingContentType::ToolUse { name, .. },
            } if name == "Foo" => {}
            other => panic!("[2] expected CBStart(ToolUse Foo, 1); got {other:?}"),
        }
        match blocks[3] {
            StreamEvent::ContentBlockStop { index: 1 } => {}
            other => panic!("[3] expected CBStop(1); got {other:?}"),
        }
    }

    // ---- Finding 12: upstream error envelope in SSE body ----------------

    /// Helper unique to F12: collect events but capture the FIRST error
    /// (the existing `collect_events` panics on Err). Returns
    /// `(events_yielded_before_error, optional_error)`.
    async fn collect_events_or_error(
        body: &'static str,
    ) -> (Vec<StreamEvent>, Option<ProviderError>) {
        let mut stream = map_openai_sse_to_events(sse_stream(body));
        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(ev) => events.push(ev),
                Err(e) => return (events, Some(e)),
            }
        }
        (events, None)
    }

    #[test]
    fn extract_upstream_error_recognizes_basic_envelope() {
        let data = r#"{"error":{"message":"oops"}}"#;
        assert_eq!(extract_upstream_error(data).as_deref(), Some("oops"));
    }

    #[test]
    fn extract_upstream_error_recognizes_lmstudio_context_overflow_shape() {
        // Verbatim shape captured during the lmstudio probe — the actual
        // error body LM Studio returns inside the SSE for a context
        // overflow.
        let data = r#"{"error":{"message":"The number of tokens to keep from the initial prompt is greater than the context length. Try to load the model with a larger context length, or provide a shorter input"}}"#;
        let msg = extract_upstream_error(data).expect("envelope must match");
        assert!(msg.starts_with("The number of tokens"));
        assert!(msg.contains("context length"));
    }

    #[test]
    fn extract_upstream_error_tolerates_siblings() {
        // OpenAI's documented error envelope carries `type` and `code`
        // siblings; the helper must not reject them.
        let data = r#"{"error":{"message":"bad","type":"invalid_request_error","code":"foo"}}"#;
        assert_eq!(extract_upstream_error(data).as_deref(), Some("bad"));
    }

    #[test]
    fn extract_upstream_error_rejects_non_envelope() {
        // Random JSON, valid `NativeChunk` shape, and outright garbage all
        // return None so the parser falls through to the legacy chunk-parse
        // error path.
        assert!(extract_upstream_error(r#"{"id":"x","choices":[]}"#).is_none());
        assert!(extract_upstream_error(r#"{"foo":1}"#).is_none());
        assert!(extract_upstream_error("not json at all").is_none());
        // Missing inner `message` field.
        assert!(extract_upstream_error(r#"{"error":{"type":"x"}}"#).is_none());
    }

    #[tokio::test]
    async fn lmstudio_context_overflow_surfaces_as_clean_upstream_error() {
        // Reproduces the lmstudio Finding 12 trigger: HTTP 200 SSE body
        // carrying an `{"error": {"message": ...}}` envelope instead of
        // a NativeChunk. The parser must yield a clean ProviderError
        // whose Display contains the upstream message, not the layered
        // "stream parse / chunk parse / missing field 'id'" wrapping.
        let body = "data: {\"error\":{\"message\":\"The number of tokens to keep from the initial prompt is greater than the context length. Try to load the model with a larger context length, or provide a shorter input\"}}\n\n";
        let (events, err) = collect_events_or_error(body).await;
        assert!(
            events.is_empty(),
            "no IR events should fire before the upstream error",
        );
        let err = err.expect("upstream error must surface");
        let s = format!("{err}");
        assert!(
            s.contains("The number of tokens"),
            "upstream message must appear verbatim; got: {s}"
        );
        assert!(
            !s.contains("chunk parse error"),
            "legacy chunk-parse wrapping must be gone; got: {s}"
        );
        assert!(
            !s.contains("missing field"),
            "legacy serde wrapping must be gone; got: {s}"
        );
        // Maps to InvalidRequest per OpenAIError → ProviderError conversion.
        assert!(
            matches!(err, ProviderError::InvalidRequest(_)),
            "upstream-error must map to InvalidRequest; got: {err:?}"
        );
    }

    #[tokio::test]
    async fn non_envelope_chunk_parse_failure_still_uses_legacy_message() {
        // When the SSE body is NEITHER a valid NativeChunk nor an error
        // envelope, the parser must fall through to the legacy chunk-parse
        // error path (no regression for the broad case).
        let body = "data: {\"unexpected\":\"shape\"}\n\n";
        let (_events, err) = collect_events_or_error(body).await;
        let s = format!("{}", err.expect("error must surface"));
        assert!(
            s.contains("chunk parse error") || s.contains("missing field"),
            "legacy parse path must still produce its diagnostic; got: {s}"
        );
    }
}
