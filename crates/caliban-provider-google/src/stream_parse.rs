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
// In-band error envelope
// ---------------------------------------------------------------------------

/// Gemini can deliver a fault mid-stream as HTTP 200 + an `{"error": {...}}`
/// object on a `data:` line instead of as a non-2xx status. Because every
/// field of `NativeResponse` is `#[serde(default)]`, such a payload would
/// otherwise deserialize into an *empty* chunk and be silently dropped. We
/// detect the envelope first and route its message through the `GoogleError`
/// classifier (context overflow → `ContextTooLong`, server fault →
/// `UpstreamServerFault`, else `InvalidRequest`).
#[derive(serde::Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorBody>,
}

#[derive(serde::Deserialize)]
struct ErrorBody {
    #[serde(default)]
    message: String,
}

/// If `data` is an in-band error envelope, return its message; otherwise
/// `None` (it is a normal response chunk).
fn in_band_error_message(data: &str) -> Option<String> {
    serde_json::from_str::<ErrorEnvelope>(data)
        .ok()
        .and_then(|env| env.error)
        .map(|err| err.message)
}

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

            // An in-band `{"error": {...}}` envelope is a server-side fault
            // delivered over a 200 stream. Classify and surface it instead of
            // letting it deserialize into an empty (silently dropped) chunk.
            if let Some(msg) = in_band_error_message(&event.data) {
                Err(ProviderError::from(GoogleError::UpstreamError(msg)))?;
                unreachable!();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a byte-stream of SSE `data:` frames from raw JSON payloads.
    fn sse_bytes(frames: &[&str]) -> BoxStream<'static, Result<Bytes, GoogleError>> {
        let mut body = String::new();
        for f in frames {
            body.push_str("data: ");
            body.push_str(f);
            body.push_str("\n\n");
        }
        futures::stream::iter(vec![Ok(Bytes::from(body))]).boxed()
    }

    async fn collect_events(
        bytes: BoxStream<'static, Result<Bytes, GoogleError>>,
    ) -> Vec<Result<StreamEvent, ProviderError>> {
        map_gemini_sse_to_events(bytes).collect().await
    }

    #[tokio::test]
    async fn in_band_internal_fault_yields_server_fault_error() {
        // Gemini delivers a mid-stream 500 as HTTP 200 + an in-band error
        // object. Today this parses into an empty NativeResponse and is
        // silently swallowed; it must instead surface as UpstreamServerFault.
        let frame =
            r#"{"error":{"code":500,"message":"Internal error encountered.","status":"INTERNAL"}}"#;
        let events = collect_events(sse_bytes(&[frame])).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Err(ProviderError::UpstreamServerFault(_)))),
            "expected an UpstreamServerFault error, got {events:?}"
        );
    }

    #[tokio::test]
    async fn in_band_context_overflow_yields_context_too_long_error() {
        // A context overflow detected after the stream opens must route to
        // ContextTooLong so the agent loop's reactive compaction fires.
        let frame = r#"{"error":{"code":400,"message":"The input token count (5200) exceeds the maximum number of tokens allowed (4096).","status":"INVALID_ARGUMENT"}}"#;
        let events = collect_events(sse_bytes(&[frame])).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Err(ProviderError::ContextTooLong { .. }))),
            "expected a ContextTooLong error, got {events:?}"
        );
    }

    #[tokio::test]
    async fn normal_chunk_is_unaffected_by_error_envelope_check() {
        // Regression: a normal response chunk (no `error` key) must still
        // produce MessageStart + a text delta and no error.
        let frame = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"hi"}]},"finishReason":"STOP"}],"usageMetadata":{},"modelVersion":"gemini-2.0"}"#;
        let events = collect_events(sse_bytes(&[frame])).await;
        assert!(
            events.iter().all(std::result::Result::is_ok),
            "normal chunk must not error, got {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Ok(StreamEvent::MessageStart { .. }))),
            "expected MessageStart, got {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                Ok(StreamEvent::Delta {
                    delta: StreamingDelta::Text(t),
                    ..
                }) if t == "hi"
            )),
            "expected a text delta 'hi', got {events:?}"
        );
    }
}
