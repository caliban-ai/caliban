//! Scripted `MockProvider` for downstream consumer tests.

use std::sync::Mutex;

use async_trait::async_trait;
use futures::stream;

use crate::capabilities::{
    Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};
use crate::error::{Error, Result};
use crate::provider::Provider;
use crate::request::CompletionRequest;
use crate::response::CompletionResponse;
use crate::stream::{MessageStream, StreamEvent};

/// A scripted provider for testing; responses are enqueued ahead of time.
#[derive(Default)]
pub struct MockProvider {
    inner: Mutex<MockState>,
}

#[derive(Default)]
struct MockState {
    complete_queue: Vec<Result<CompletionResponse>>,
    stream_queue: Vec<Result<Vec<Result<StreamEvent>>>>,
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
            .push(Ok(events));
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
            .push(Err(err));
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
        let mut s = self.inner.lock().expect("MockProvider lock poisoned");
        if s.stream_queue.is_empty() {
            return Err(Error::InvalidRequest(
                "MockProvider: stream queue empty".into(),
            ));
        }
        match s.stream_queue.remove(0) {
            Err(e) => Err(e),
            Ok(events) => Ok(Box::pin(stream::iter(events))),
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
