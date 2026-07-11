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
    let s = bytes.eventsource().flat_map(|item| {
        // A single native event can map to multiple IR events (e.g.
        // message_start → MessageStart + a synthetic cache usage delta,
        // #423), so flatten a per-item event list into the stream.
        let events: Vec<ProviderResult<StreamEvent>> = match item {
            Ok(event) => {
                // Skip SSE comments and empty frames (eventsource-stream
                // may emit them for keep-alive lines that carry no data).
                if event.data.is_empty() {
                    vec![]
                } else {
                    match serde_json::from_str::<NativeEvent>(&event.data) {
                        Ok(ne) => match native_event_to_ir(ne) {
                            // Filter out Ping to keep the caller stream clean.
                            Ok(evs) => evs
                                .into_iter()
                                .filter(|e| !matches!(e, StreamEvent::Ping))
                                .map(Ok)
                                .collect(),
                            Err(e) => vec![Err(e)],
                        },
                        Err(e) => vec![Err(ProviderError::adapter(AnthropicError::StreamParse(
                            format!("event parse failed: {e}; data: {}", event.data),
                        )))],
                    }
                }
            }
            Err(EventStreamError::Transport(e)) => vec![Err(ProviderError::network(e))],
            Err(e) => vec![Err(ProviderError::adapter(AnthropicError::StreamParse(
                format!("{e}"),
            )))],
        };
        futures::stream::iter(events)
    });
    Box::pin(s)
}

fn native_event_to_ir(e: NativeEvent) -> ProviderResult<Vec<StreamEvent>> {
    Ok(match e {
        NativeEvent::MessageStart { message } => {
            let mut out = vec![StreamEvent::MessageStart {
                id: message.id,
                model: message.model,
            }];
            // #423: Anthropic reports the prompt-cache breakdown
            // (cache_creation/cache_read) ONLY on `message_start.usage`, but the
            // assembled usage is taken from `message_delta.usage`, which omits
            // them — so streamed cached turns dropped the cache tokens entirely
            // and under-reported input by the full cached-prompt size (streaming
            // disagreeing with the non-streaming path). Emit the cache
            // contribution as a synthetic usage delta so `Usage::merge` folds it
            // in: `input_tokens += cached tokens`, cache counters preserved. The
            // base input + output still come from `message_delta`, so nothing is
            // double-counted.
            let cc = message.usage.cache_creation_input_tokens;
            let cr = message.usage.cache_read_input_tokens;
            if cc.is_some() || cr.is_some() {
                out.push(StreamEvent::MessageDelta {
                    stop_reason: None,
                    usage_delta: Some(Usage {
                        input_tokens: cc.unwrap_or(0) + cr.unwrap_or(0),
                        output_tokens: 0,
                        cache_creation_input_tokens: cc,
                        cache_read_input_tokens: cr,
                    }),
                });
            }
            out
        }
        NativeEvent::ContentBlockStart {
            index,
            content_block,
        } => vec![StreamEvent::ContentBlockStart {
            index,
            content_type: content_block_to_streaming_type(&content_block),
        }],
        NativeEvent::ContentBlockDelta { index, delta } => {
            let ir_delta = match delta {
                NativeBlockDelta::TextDelta { text } => StreamingDelta::Text(text),
                NativeBlockDelta::InputJsonDelta { partial_json } => {
                    StreamingDelta::ToolUseInputJson(partial_json)
                }
                NativeBlockDelta::ThinkingDelta { thinking } => StreamingDelta::Thinking(thinking),
                // SignatureDelta carries the cryptographic signature appended
                // after a thinking block. It must be preserved and re-sent on
                // the next turn or Anthropic rejects the retained thinking block
                // (400) — carry it through the IR as a Signature delta (#419).
                NativeBlockDelta::SignatureDelta { signature } => {
                    StreamingDelta::Signature(signature)
                }
            };
            vec![StreamEvent::Delta {
                index,
                delta: ir_delta,
            }]
        }
        NativeEvent::ContentBlockStop { index } => vec![StreamEvent::ContentBlockStop { index }],
        NativeEvent::MessageDelta { delta, usage } => vec![StreamEvent::MessageDelta {
            stop_reason: delta.stop_reason.map(map_stop_reason),
            // Anthropic's message_delta usage contains the *cumulative* output
            // token count at the time the stream ended. We pass it through as
            // a usage_delta; callers that call collect_message will merge it
            // via Usage::merge.
            //
            // Normalize to the OpenAI convention: input_tokens is the TOTAL
            // prompt size (including any cached portion). Anthropic reports
            // these three counters disjointly, so we sum them here. The
            // separated cache counters are preserved unchanged. (The cache
            // breakdown itself arrives on message_start — see #423 above.)
            usage_delta: Some(Usage {
                input_tokens: usage.input_tokens
                    + usage.cache_creation_input_tokens.unwrap_or(0)
                    + usage.cache_read_input_tokens.unwrap_or(0),
                output_tokens: usage.output_tokens,
                cache_creation_input_tokens: usage.cache_creation_input_tokens,
                cache_read_input_tokens: usage.cache_read_input_tokens,
            }),
        }],
        NativeEvent::MessageStop => vec![StreamEvent::MessageStop],
        NativeEvent::Ping => vec![StreamEvent::Ping],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::events::NativeMessageHeader;
    use crate::schema::response::NativeUsage;

    fn start_with_cache(cc: Option<u32>, cr: Option<u32>) -> NativeEvent {
        NativeEvent::MessageStart {
            message: NativeMessageHeader {
                id: "msg_1".into(),
                model: "claude".into(),
                usage: NativeUsage {
                    input_tokens: 12,
                    output_tokens: 1,
                    cache_creation_input_tokens: cc,
                    cache_read_input_tokens: cr,
                },
                content: vec![],
            },
        }
    }

    #[test]
    fn message_start_emits_synthetic_cache_usage_delta() {
        // #423: cache tokens live only on message_start; they must be carried
        // into the assembled usage so streamed cached turns aren't undercounted.
        let out = native_event_to_ir(start_with_cache(Some(100), Some(200))).unwrap();
        assert_eq!(out.len(), 2, "MessageStart + synthetic cache delta");
        assert!(matches!(out[0], StreamEvent::MessageStart { .. }));
        let StreamEvent::MessageDelta {
            usage_delta: Some(u),
            ..
        } = &out[1]
        else {
            panic!("expected a cache usage delta, got {:?}", out[1]);
        };
        assert_eq!(u.input_tokens, 300, "cached tokens folded into input");
        assert_eq!(u.cache_creation_input_tokens, Some(100));
        assert_eq!(u.cache_read_input_tokens, Some(200));
        assert_eq!(u.output_tokens, 0, "output is counted on message_delta");
    }

    #[test]
    fn message_start_without_cache_emits_only_start() {
        let out = native_event_to_ir(start_with_cache(None, None)).unwrap();
        assert_eq!(out.len(), 1, "no cache → no synthetic delta");
        assert!(matches!(out[0], StreamEvent::MessageStart { .. }));
    }
}
