//! Scripted `MockProvider` for downstream consumer tests.

use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt as _;
use futures::stream;

use crate::capabilities::{
    Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};
use crate::error::{Error, Result};
use crate::provider::Provider;
use crate::request::CompletionRequest;
use crate::response::{CompletionResponse, StopReason, Usage};
use crate::stream::{MessageStream, StreamEvent, StreamingContentType, StreamingDelta};

/// A scripted provider for testing; responses are enqueued ahead of time.
#[derive(Default)]
pub struct MockProvider {
    inner: Mutex<MockState>,
}

/// Internal entry in the `MockProvider` stream queue.
///
/// Most tests use [`MockEntry::Events`] (a pre-built event vector). The
/// [`MockEntry::Error`] variant lets `MockProvider::stream` fail before any
/// events are produced; [`MockEntry::Silent`] yields a stream that stays
/// `Pending` forever (used to drive `WatchedStream` integration tests).
enum MockEntry {
    Events(Vec<Result<StreamEvent>>),
    Error(Error),
    Silent,
    /// A stream that stays silent for `delay` (exercising the prefill budget,
    /// #263), then emits `events`.
    DelayedFirstChunk {
        delay: Duration,
        events: Vec<Result<StreamEvent>>,
    },
    /// The `stream()` *call itself* hangs for `delay` before returning — models
    /// a server that accepts the connection but never sends response headers,
    /// so `.send().await` never resolves. Exercises the agent-core first-byte
    /// timeout (#330), which sits *before* any stream watchdog can observe.
    HangingCall {
        delay: Duration,
    },
}

#[derive(Default)]
struct MockState {
    complete_queue: Vec<Result<CompletionResponse>>,
    stream_queue: Vec<MockEntry>,
    capabilities: Option<Capabilities>,
    models: Vec<ModelInfo>,
}

impl MockProvider {
    /// Create a new empty `MockProvider`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue a response to be returned by the next `complete` call.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn enqueue_complete(&self, resp: Result<CompletionResponse>) {
        self.inner
            .lock()
            .expect("MockProvider lock poisoned")
            .complete_queue
            .push(resp);
    }

    /// Enqueue a sequence of stream events for the next `stream` call.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn enqueue_stream(&self, events: Vec<Result<StreamEvent>>) {
        self.inner
            .lock()
            .expect("MockProvider lock poisoned")
            .stream_queue
            .push(MockEntry::Events(events));
    }

    /// Enqueue an error to be returned by the next `stream` call.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn enqueue_stream_error(&self, err: Error) {
        self.inner
            .lock()
            .expect("MockProvider lock poisoned")
            .stream_queue
            .push(MockEntry::Error(err));
    }

    /// Enqueue a stream that stays `Pending` forever; useful for exercising
    /// `WatchedStream`-style idle watchdogs.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn enqueue_silent_stream(&self) {
        self.inner
            .lock()
            .expect("MockProvider lock poisoned")
            .stream_queue
            .push(MockEntry::Silent);
    }

    /// Enqueue a stream that stays silent for `delay`, then emits a normal
    /// `EndTurn` text response — the non-builder analogue of
    /// [`MockProviderBuilder::with_delayed_first_chunk`]. Used to give each
    /// agent turn a measurable wall-clock cost in timing tests.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn enqueue_delayed_first_chunk(&self, delay: Duration, text: &str) {
        let events = build_text_events(text, StopReason::EndTurn, 1);
        self.inner
            .lock()
            .expect("MockProvider lock poisoned")
            .stream_queue
            .push(MockEntry::DelayedFirstChunk { delay, events });
    }

    /// Begin building a `MockProvider` with a chainable response API.
    #[must_use]
    pub fn builder() -> MockProviderBuilder {
        MockProviderBuilder::default()
    }

    /// Override the capabilities returned by this mock.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn set_capabilities(&self, caps: Capabilities) {
        self.inner
            .lock()
            .expect("MockProvider lock poisoned")
            .capabilities = Some(caps);
    }

    /// Override the model list returned by this mock.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn set_models(&self, models: Vec<ModelInfo>) {
        self.inner
            .lock()
            .expect("MockProvider lock poisoned")
            .models = models;
    }

    /// Test helper: build a `MockProvider` whose `list_models()` reports
    /// exactly the supplied ids (default capabilities; `native_id` == id).
    /// Useful for `/model` swap tests where the assertion turns on which
    /// model ids the provider claims to know.
    #[must_use]
    pub fn for_tests_with_models(ids: &[&str]) -> Self {
        let p = Self::new();
        let caps = default_capabilities();
        let models: Vec<ModelInfo> = ids
            .iter()
            .map(|id| ModelInfo {
                id: (*id).to_string(),
                native_id: (*id).to_string(),
                display_name: (*id).to_string(),
                capabilities: caps,
            })
            .collect();
        p.set_models(models);
        p
    }
}

fn default_capabilities() -> Capabilities {
    Capabilities {
        max_input_tokens: 100_000,
        max_output_tokens: 4_096,
        vision: false,
        tool_use: ToolUseCapability::Basic,
        thinking: false,
        prompt_caching: PromptCachingCapability::None,
        json_mode: false,
        streaming: true,
        stop_sequences: true,
        top_k: false,
        system_prompt: SystemPromptCapability::SeparateField,
        refusal_field: false,
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
        let mut s = self.inner.lock().expect("MockProvider lock poisoned");
        if s.complete_queue.is_empty() {
            return Err(Error::InvalidRequest(
                "MockProvider: complete queue empty".into(),
            ));
        }
        s.complete_queue.remove(0)
    }

    async fn stream(&self, _req: CompletionRequest) -> Result<MessageStream> {
        // Take the next entry and release the lock *before* any await — the
        // MutexGuard is not Send and `HangingCall` awaits (would make the
        // future non-Send otherwise).
        let entry = {
            let mut s = self.inner.lock().expect("MockProvider lock poisoned");
            if s.stream_queue.is_empty() {
                return Err(Error::InvalidRequest(
                    "MockProvider: stream queue empty".into(),
                ));
            }
            s.stream_queue.remove(0)
        };
        match entry {
            MockEntry::Error(e) => Err(e),
            MockEntry::Events(events) => Ok(Box::pin(stream::iter(events))),
            MockEntry::Silent => Ok(Box::pin(SilentStream)),
            MockEntry::DelayedFirstChunk { delay, events } => {
                // Sleep once (no chunk → exercises the prefill budget), then
                // emit the events. `once(...).flatten()` moves `events` into
                // the future exactly once, so no Clone bound is needed (Error
                // is not Clone).
                let s = stream::once(async move {
                    tokio::time::sleep(delay).await;
                    stream::iter(events)
                })
                .flatten();
                Ok(Box::pin(s))
            }
            MockEntry::HangingCall { delay } => {
                // The call itself blocks — response headers never arrive.
                tokio::time::sleep(delay).await;
                Ok(Box::pin(stream::empty()))
            }
        }
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        self.inner
            .lock()
            .expect("MockProvider lock poisoned")
            .capabilities
            .unwrap_or_else(default_capabilities)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        self.inner
            .lock()
            .expect("MockProvider lock poisoned")
            .models
            .clone()
    }

    fn name(&self) -> &'static str {
        "mock"
    }
}

// ---------------------------------------------------------------------------
// MockProviderBuilder + helpers
// ---------------------------------------------------------------------------

/// A stream that stays `Pending` forever; used by `MockProvider` to simulate a
/// silent SSE connection for `WatchedStream` tests.
struct SilentStream;

impl Stream for SilentStream {
    type Item = Result<StreamEvent>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Pending
    }
}

/// Fluent builder for [`MockProvider`].
///
/// Each `with_response_*` call enqueues one full streaming response. Each
/// `with_error_once` enqueues one error that will surface from the next
/// `stream` call.
#[derive(Default)]
pub struct MockProviderBuilder {
    entries: Vec<MockEntry>,
    capabilities: Option<Capabilities>,
    models: Vec<ModelInfo>,
}

impl MockProviderBuilder {
    /// Override the capabilities returned by the built provider.
    #[must_use]
    pub fn with_capabilities(mut self, caps: Capabilities) -> Self {
        self.capabilities = Some(caps);
        self
    }

    /// Override the model list returned by the built provider.
    #[must_use]
    pub fn with_models(mut self, models: Vec<ModelInfo>) -> Self {
        self.models = models;
        self
    }

    /// Enqueue a turn that ends with `StopReason::MaxTokens`, emitting
    /// `output_tokens` worth of placeholder text and no `tool_use` block.
    #[must_use]
    pub fn with_response_max_tokens(self, output_tokens: u32) -> Self {
        self.with_response_stop_reason(StopReason::MaxTokens, "")
            .with_output_tokens(output_tokens)
    }

    /// Enqueue a turn that ends with `StopReason::EndTurn` and the given
    /// assistant text.
    #[must_use]
    pub fn with_response_end_turn(self, text: &str) -> Self {
        self.with_response_stop_reason(StopReason::EndTurn, text)
    }

    /// Enqueue a turn that ends with the given `stop_reason` and the given
    /// assistant text. The default usage is `input=1, output=1`; use
    /// [`Self::with_output_tokens`] to override the last entry's
    /// `output_tokens`.
    #[must_use]
    pub fn with_response_stop_reason(mut self, stop: StopReason, text: &str) -> Self {
        let events = build_text_events(text, stop, 1);
        self.entries.push(MockEntry::Events(events));
        self
    }

    /// Set the `output_tokens` for the most recently pushed text-ending entry.
    /// No-op if the last entry isn't a normal `Events` entry.
    #[must_use]
    pub fn with_output_tokens(mut self, output_tokens: u32) -> Self {
        if let Some(MockEntry::Events(events)) = self.entries.last_mut() {
            for evt in events.iter_mut() {
                if let Ok(StreamEvent::MessageDelta {
                    usage_delta: Some(u),
                    ..
                }) = evt
                {
                    u.output_tokens = output_tokens;
                }
            }
        }
        self
    }

    /// Enqueue exactly one error that will surface from the next `stream`
    /// call.
    #[must_use]
    pub fn with_error_once(mut self, err: Error) -> Self {
        self.entries.push(MockEntry::Error(err));
        self
    }

    /// Enqueue a stream whose `stream()` **call** hangs for `delay` before
    /// returning — models a server that accepts the connection but never sends
    /// response headers. Exercises the agent-core first-byte timeout (#330).
    #[must_use]
    pub fn with_hanging_stream(mut self, delay: Duration) -> Self {
        self.entries.push(MockEntry::HangingCall { delay });
        self
    }

    /// Enqueue a stream that stays `Pending` forever (after an optional
    /// minimum lifetime). The `_min_silence` argument is ignored today and
    /// reserved for future use; callers may pass `Duration::default()`.
    #[must_use]
    pub fn with_silent_stream(mut self, _min_silence: Duration) -> Self {
        self.entries.push(MockEntry::Silent);
        self
    }

    /// Enqueue a stream that stays silent for `delay` (exercising the prefill
    /// budget, #263), then emits a normal `EndTurn` response with `text`.
    #[must_use]
    pub fn with_delayed_first_chunk(mut self, delay: Duration, text: &str) -> Self {
        let events = build_text_events(text, StopReason::EndTurn, 1);
        self.entries
            .push(MockEntry::DelayedFirstChunk { delay, events });
        self
    }

    /// Finalise the builder, returning a fully-loaded [`MockProvider`].
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned (only possible after a
    /// previous test thread panicked while holding the lock).
    #[must_use]
    pub fn build(self) -> MockProvider {
        let provider = MockProvider::new();
        {
            let mut s = provider.inner.lock().expect("MockProvider lock poisoned");
            s.stream_queue = self.entries;
            s.capabilities = self.capabilities;
            s.models = self.models;
        }
        provider
    }
}

/// Build a minimal but complete event vector for a text-only turn.
fn build_text_events(text: &str, stop: StopReason, output_tokens: u32) -> Vec<Result<StreamEvent>> {
    let mut out = Vec::with_capacity(6);
    out.push(Ok(StreamEvent::MessageStart {
        id: "msg_mock".into(),
        model: "mock".into(),
    }));
    out.push(Ok(StreamEvent::ContentBlockStart {
        index: 0,
        content_type: StreamingContentType::Text,
    }));
    if !text.is_empty() {
        out.push(Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text(text.to_owned()),
        }));
    }
    out.push(Ok(StreamEvent::ContentBlockStop { index: 0 }));
    out.push(Ok(StreamEvent::MessageDelta {
        stop_reason: Some(stop),
        usage_delta: Some(Usage {
            input_tokens: 1,
            output_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
    }));
    out.push(Ok(StreamEvent::MessageStop));
    out
}
