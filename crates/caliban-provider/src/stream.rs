//! Streaming events.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::stream::Stream;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::message::{ContentBlock, Message, Role, TextBlock};
use crate::response::{StopReason, Usage};
use crate::thinking::ThinkingBlock;
use crate::tool::ToolUseBlock;

/// A single event in a streaming completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// The message has started; carries the assigned ID and model.
    MessageStart {
        /// Provider-assigned message identifier.
        id: String,
        /// Model that is generating the message.
        model: String,
    },
    /// A content block is starting at the given index.
    ContentBlockStart {
        /// Zero-based block index.
        index: u32,
        /// The type of content block that is starting.
        content_type: StreamingContentType,
    },
    /// An incremental delta for the block at the given index.
    Delta {
        /// Zero-based block index.
        index: u32,
        /// The incremental content.
        delta: StreamingDelta,
    },
    /// The content block at the given index is complete.
    ContentBlockStop {
        /// Zero-based block index.
        index: u32,
    },
    /// End-of-message metadata delta.
    MessageDelta {
        /// Why the model stopped, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<StopReason>,
        /// Incremental usage update.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage_delta: Option<Usage>,
    },
    /// The message is fully complete.
    MessageStop,
    /// A keep-alive ping from the provider.
    Ping,
}

/// The type of content block that is opening in a stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamingContentType {
    /// A plain-text block.
    Text,
    /// A tool-use block with the call ID and tool name.
    ToolUse {
        /// Unique call identifier.
        id: String,
        /// Name of the tool being called.
        name: String,
    },
    /// An extended-thinking block.
    Thinking,
    /// An image block.
    Image,
}

/// An incremental delta for a streaming content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamingDelta {
    /// A text increment.
    Text(String),
    /// A JSON-fragment increment for a tool-use input.
    ToolUseInputJson(String),
    /// A thinking-text increment.
    Thinking(String),
}

/// Boxed dynamic stream of stream events.
pub type MessageStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send + 'static>>;

/// Consume a [`MessageStream`] and assemble the final [`Message`], [`StopReason`], and [`Usage`].
///
/// # Errors
///
/// Returns the first stream error encountered, or `Error::InvalidRequest` if
/// an unsupported block type is streamed.
#[allow(clippy::too_many_lines)]
pub async fn collect_message(mut stream: MessageStream) -> Result<(Message, StopReason, Usage)> {
    let mut blocks: Vec<ContentBlock> = Vec::new();
    let mut block_types: Vec<StreamingContentType> = Vec::new();
    let mut block_text: Vec<String> = Vec::new();
    let mut block_json: Vec<String> = Vec::new();
    let mut stop_reason: Option<StopReason> = None;
    let mut usage = Usage::default();

    while let Some(evt) = stream.next().await {
        match evt? {
            StreamEvent::MessageStart { .. } | StreamEvent::Ping | StreamEvent::MessageStop => {}
            StreamEvent::ContentBlockStart {
                index,
                content_type,
            } => {
                let i = index as usize;
                if blocks.len() <= i {
                    blocks.resize(
                        i + 1,
                        ContentBlock::Text(TextBlock {
                            text: String::new(),
                            cache_control: None,
                        }),
                    );
                    block_types.resize(i + 1, StreamingContentType::Text);
                    block_text.resize(i + 1, String::new());
                    block_json.resize(i + 1, String::new());
                }
                block_types[i] = content_type;
            }
            StreamEvent::Delta { index, delta } => {
                let i = index as usize;
                if i >= block_types.len() {
                    return Err(Error::InvalidRequest(format!(
                        "Delta event for uninitialized block index {i}"
                    )));
                }
                match delta {
                    StreamingDelta::Text(s) | StreamingDelta::Thinking(s) => {
                        block_text[i].push_str(&s);
                    }
                    StreamingDelta::ToolUseInputJson(s) => block_json[i].push_str(&s),
                }
            }
            StreamEvent::ContentBlockStop { index } => {
                let i = index as usize;
                if i >= block_types.len() {
                    return Err(Error::InvalidRequest(format!(
                        "ContentBlockStop for uninitialized block index {i}"
                    )));
                }
                let block = match &block_types[i] {
                    StreamingContentType::Text => ContentBlock::Text(TextBlock {
                        text: std::mem::take(&mut block_text[i]),
                        cache_control: None,
                    }),
                    StreamingContentType::Thinking => ContentBlock::Thinking(ThinkingBlock {
                        thinking: std::mem::take(&mut block_text[i]),
                        signature: None,
                    }),
                    StreamingContentType::ToolUse { id, name } => {
                        let json_str = std::mem::take(&mut block_json[i]);
                        let input = if json_str.is_empty() {
                            serde_json::json!({})
                        } else {
                            serde_json::from_str(&json_str).map_err(|e| {
                                Error::InvalidRequest(format!(
                                    "tool_use input json parse error: {e}"
                                ))
                            })?
                        };
                        ContentBlock::ToolUse(ToolUseBlock {
                            id: id.clone(),
                            name: name.clone(),
                            input,
                        })
                    }
                    StreamingContentType::Image => {
                        return Err(Error::InvalidRequest(
                            "streaming Image blocks are not supported in collect_message".into(),
                        ));
                    }
                };
                blocks[i] = block;
            }
            StreamEvent::MessageDelta {
                stop_reason: sr,
                usage_delta,
            } => {
                if let Some(sr) = sr {
                    stop_reason = Some(sr);
                }
                if let Some(u) = usage_delta {
                    usage.merge(u);
                }
            }
        }
    }

    let stop = stop_reason.unwrap_or(StopReason::EndTurn);
    Ok((
        Message {
            role: Role::Assistant,
            content: blocks,
        },
        stop,
        usage,
    ))
}

// ---------------------------------------------------------------------------
// WatchedStream — stream-idle watchdog (ADR Plan A, Task 8)
// ---------------------------------------------------------------------------

/// Wraps a `Stream` and aborts with [`Error::StreamIdle`] when no chunk
/// arrives within `idle`.
///
/// Emits a `tracing::warn` at half-time (helpful operational signal for
/// observability dashboards) and `Err(Error::StreamIdle)` on full timeout.
///
/// `S` must be `Unpin` because we hold the inner stream in a `Box<dyn ...>`
/// behind a `Pin<&mut Self>`-style `poll_next`. The concrete provider streams
/// (`MessageStream = Pin<Box<dyn Stream + Send>>`) are already pinned at
/// construction; `WatchedStream` owns the pointer directly so projection
/// stays simple without pulling in `pin_project_lite`.
pub struct WatchedStream<S> {
    inner: S,
    idle: Duration,
    last_chunk_at: Instant,
    warned: bool,
}

impl<S> WatchedStream<S> {
    /// Build a new `WatchedStream`. `idle` is the maximum time the inner
    /// stream may stay silent before [`Error::StreamIdle`] is surfaced.
    pub fn new(inner: S, idle: Duration) -> Self {
        Self {
            inner,
            idle,
            last_chunk_at: Instant::now(),
            warned: false,
        }
    }
}

impl<S> Stream for WatchedStream<S>
where
    S: Stream<Item = Result<StreamEvent>> + Unpin,
{
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(item)) => {
                self.last_chunk_at = Instant::now();
                self.warned = false;
                Poll::Ready(Some(item))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => {
                let elapsed = self.last_chunk_at.elapsed();
                if elapsed >= self.idle {
                    tracing::error!(
                        target: "caliban::stream",
                        elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                        "recovery.stream_idle.abort"
                    );
                    return Poll::Ready(Some(Err(Error::StreamIdle(elapsed))));
                }
                if !self.warned && elapsed >= self.idle / 2 {
                    self.warned = true;
                    tracing::warn!(
                        target: "caliban::stream",
                        elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                        "recovery.stream_idle.warning"
                    );
                }
                // Schedule a wakeup at the remaining time so we can fire the
                // abort even if `inner` stays Pending. The spawned future is
                // a single sleep + wake; it self-terminates.
                let remaining = self.idle.checked_sub(elapsed).unwrap_or(Duration::ZERO);
                let waker = cx.waker().clone();
                tokio::spawn(async move {
                    tokio::time::sleep(remaining + Duration::from_millis(1)).await;
                    waker.wake();
                });
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod watched_tests {
    use super::*;
    use futures::stream;
    use std::time::Duration;

    #[tokio::test]
    async fn passes_through_normal_data() {
        let inner = stream::iter(vec![
            Ok(StreamEvent::MessageStop),
            Ok(StreamEvent::MessageStop),
        ]);
        let mut w = WatchedStream::new(inner, Duration::from_secs(1));
        let mut seen = 0;
        while let Some(item) = w.next().await {
            item.unwrap();
            seen += 1;
        }
        assert_eq!(seen, 2);
    }

    #[tokio::test]
    async fn aborts_after_idle_timeout() {
        let inner = stream::pending::<Result<StreamEvent>>();
        let mut w = WatchedStream::new(inner, Duration::from_millis(20));
        let r = w.next().await.expect("Some(_)");
        assert!(matches!(r, Err(Error::StreamIdle(_))));
    }
}
