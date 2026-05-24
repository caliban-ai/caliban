//! SSE → `caliban_provider::StreamEvent` parsing for Anthropic.

use bytes::Bytes;
use caliban_provider::{
    Error as ProviderError, Result as ProviderResult, StopReason, StreamEvent,
    StreamingContentType, StreamingDelta, Usage,
};
use eventsource_stream::{EventStreamError, Eventsource};
use futures::stream::{BoxStream, StreamExt};

use crate::error::AnthropicError;
use crate::schema::events::{NativeBlockDelta, NativeEvent};
use crate::schema::request::NativeContentBlock;
use crate::schema::response::NativeStopReason;

/// Adapt a byte-stream of SSE chunks into a [`caliban_provider::MessageStream`].
///
/// Each SSE `data:` payload is deserialized as a [`NativeEvent`] and mapped
/// to the IR [`StreamEvent`] type. Ping events are forwarded as-is so that
/// callers can implement keep-alive logic. Server-side `error` events are
/// surfaced as `Err(ProviderError::Adapter(...))`.
pub(crate) fn map_sse_to_events(
    bytes: BoxStream<'static, Result<Bytes, AnthropicError>>,
) -> caliban_provider::MessageStream {
    let s = bytes.eventsource().filter_map(|item| async move {
        match item {
            Ok(event) => {
                // Skip SSE comments and empty frames (eventsource-stream
                // may emit them for keep-alive lines that carry no data).
                if event.data.is_empty() {
                    return None;
                }
                let parsed: Result<NativeEvent, _> = serde_json::from_str(&event.data);
                match parsed {
                    Ok(ne) => {
                        let ir = native_event_to_ir(ne);
                        // Filter out Ping from the stream to keep the caller
                        // stream clean. Callers that need raw Ping can wrap
                        // the bytes stream directly.
                        match &ir {
                            Ok(StreamEvent::Ping) => None,
                            _ => Some(ir),
                        }
                    }
                    Err(e) => Some(Err(ProviderError::adapter(AnthropicError::StreamParse(
                        format!("event parse failed: {e}; data: {}", event.data),
                    )))),
                }
            }
            Err(EventStreamError::Transport(e)) => Some(Err(ProviderError::network(e))),
            Err(e) => Some(Err(ProviderError::adapter(AnthropicError::StreamParse(
                format!("{e}"),
            )))),
        }
    });
    Box::pin(s)
}

fn native_event_to_ir(e: NativeEvent) -> ProviderResult<StreamEvent> {
    Ok(match e {
        NativeEvent::MessageStart { message } => StreamEvent::MessageStart {
            id: message.id,
            model: message.model,
        },
        NativeEvent::ContentBlockStart {
            index,
            content_block,
        } => StreamEvent::ContentBlockStart {
            index,
            content_type: content_block_to_streaming_type(&content_block),
        },
        NativeEvent::ContentBlockDelta { index, delta } => {
            let ir_delta = match delta {
                NativeBlockDelta::TextDelta { text } => StreamingDelta::Text(text),
                NativeBlockDelta::InputJsonDelta { partial_json } => {
                    StreamingDelta::ToolUseInputJson(partial_json)
                }
                NativeBlockDelta::ThinkingDelta { thinking } => StreamingDelta::Thinking(thinking),
                // SignatureDelta carries a cryptographic signature appended
                // after a thinking block.  The IR has no dedicated delta type
                // for signatures; we surface it as a no-op Ping so the stream
                // stays well-formed without losing any text content.
                NativeBlockDelta::SignatureDelta { .. } => return Ok(StreamEvent::Ping),
            };
            StreamEvent::Delta {
                index,
                delta: ir_delta,
            }
        }
        NativeEvent::ContentBlockStop { index } => StreamEvent::ContentBlockStop { index },
        NativeEvent::MessageDelta { delta, usage } => StreamEvent::MessageDelta {
            stop_reason: delta.stop_reason.map(map_stop_reason),
            // Anthropic's message_delta usage contains the *cumulative* output
            // token count at the time the stream ended. We pass it through as
            // a usage_delta; callers that call collect_message will merge it
            // via Usage::merge.
            //
            // Normalize to the OpenAI convention: input_tokens is the TOTAL
            // prompt size (including any cached portion). Anthropic reports
            // these three counters disjointly, so we sum them here. The
            // separated cache counters are preserved unchanged.
            usage_delta: Some(Usage {
                input_tokens: usage.input_tokens
                    + usage.cache_creation_input_tokens.unwrap_or(0)
                    + usage.cache_read_input_tokens.unwrap_or(0),
                output_tokens: usage.output_tokens,
                cache_creation_input_tokens: usage.cache_creation_input_tokens,
                cache_read_input_tokens: usage.cache_read_input_tokens,
            }),
        },
        NativeEvent::MessageStop => StreamEvent::MessageStop,
        NativeEvent::Ping => StreamEvent::Ping,
        NativeEvent::Error { error } => {
            return Err(ProviderError::adapter(AnthropicError::StreamParse(
                format!("server-side stream error: {error}"),
            )));
        }
    })
}

fn map_stop_reason(r: NativeStopReason) -> StopReason {
    match r {
        NativeStopReason::EndTurn | NativeStopReason::PauseTurn => StopReason::EndTurn,
        NativeStopReason::MaxTokens => StopReason::MaxTokens,
        NativeStopReason::StopSequence => StopReason::StopSequence,
        NativeStopReason::ToolUse => StopReason::ToolUse,
        NativeStopReason::Refusal => StopReason::Refusal,
    }
}

fn content_block_to_streaming_type(b: &NativeContentBlock) -> StreamingContentType {
    match b {
        NativeContentBlock::Text(_) => StreamingContentType::Text,
        NativeContentBlock::Thinking(_) | NativeContentBlock::RedactedThinking { .. } => {
            StreamingContentType::Thinking
        }
        NativeContentBlock::ToolUse(tu) => StreamingContentType::ToolUse {
            id: tu.id.clone(),
            name: tu.name.clone(),
        },
        // Image blocks are unusual in streaming; fall back to Text so the
        // stream remains valid.
        NativeContentBlock::Image(_) | NativeContentBlock::ToolResult(_) => {
            StreamingContentType::Text
        }
    }
}
