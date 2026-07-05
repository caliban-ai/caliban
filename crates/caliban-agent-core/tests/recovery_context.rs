//! Reactive compaction on `ContextTooLong`: the agent transparently compacts
//! once when the provider reports a context-window overflow, then retries.

#![allow(missing_docs)]

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use async_trait::async_trait;
use caliban_agent_core::{Agent, AgentConfig, Compactor, StopCondition, TurnEvent};
use caliban_provider::{
    Capabilities, CompletionRequest, Error as ProviderError, Message, MockProvider, ModelInfo,
    Provider,
};
use futures::StreamExt as _;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Wraps a [`MockProvider`] and records the message count of every request
/// passed to `stream`, so tests can assert what history each provider call saw.
struct RecordingProvider {
    inner: MockProvider,
    request_lengths: Arc<Mutex<Vec<usize>>>,
}

#[async_trait]
impl Provider for RecordingProvider {
    async fn complete(
        &self,
        req: CompletionRequest,
    ) -> caliban_provider::Result<caliban_provider::CompletionResponse> {
        self.inner.complete(req).await
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> caliban_provider::Result<caliban_provider::stream::MessageStream> {
        self.request_lengths
            .lock()
            .expect("lock")
            .push(req.messages.len());
        self.inner.stream(req).await
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        self.inner.capabilities(model)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        self.inner.list_models()
    }

    fn name(&self) -> &'static str {
        "recording-mock"
    }
}

struct RecordingCompactor {
    calls: Arc<AtomicU32>,
}

#[async_trait]
impl Compactor for RecordingCompactor {
    async fn compact(
        &self,
        messages: &[Message],
        _caps: &Capabilities,
    ) -> caliban_agent_core::error::Result<Option<caliban_agent_core::Compaction>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Drop everything but the last user message to simulate a real reduction.
        let last_user = messages
            .iter()
            .rev()
            .find(|m| m.role == caliban_provider::Role::User)
            .cloned();
        Ok(last_user.map(|m| caliban_agent_core::Compaction {
            messages: vec![m],
            usage: None,
        }))
    }
    fn strategy_name(&self) -> &'static str {
        "test-recording"
    }
}

fn agent_with(provider: MockProvider, compactor: Arc<RecordingCompactor>) -> Arc<Agent> {
    let cfg = AgentConfig {
        model: "mock".into(),
        ..Default::default()
    };
    Arc::new(
        Agent::builder()
            .provider(Arc::new(provider))
            .config(cfg)
            .compactor(compactor)
            .build()
            .expect("agent"),
    )
}

#[tokio::test]
async fn reactive_compacts_then_retries_once() {
    let provider = MockProvider::builder()
        .with_error_once(ProviderError::ContextTooLong {
            max_tokens: 200_000,
            requested_tokens: 210_000,
        })
        .with_response_end_turn("ok")
        .build();
    let calls = Arc::new(AtomicU32::new(0));
    let agent = agent_with(
        provider,
        Arc::new(RecordingCompactor {
            calls: calls.clone(),
        }),
    );

    let mut stream =
        agent.stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev {
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "expected EndOfTurn after reactive compaction recovered the run, got {last_stop:?}"
    );
    // The compactor is invoked both by the proactive pre-turn step and by
    // the reactive `ContextTooLong` path. We only need to confirm that the
    // reactive retry actually wired the compactor in (≥1 call) and the run
    // ultimately succeeded.
    assert!(
        calls.load(Ordering::SeqCst) >= 1,
        "compactor must be invoked at least once"
    );
}

/// Characterization (#152): after `ContextTooLong`, the *retry* request must
/// be issued with the *compacted* history, not the original. The recording
/// compactor reduces history to just the last user message, so the second
/// `stream` call must see exactly one message even though the first saw more.
#[tokio::test]
async fn reactive_retry_uses_compacted_history() {
    let provider = MockProvider::builder()
        .with_error_once(ProviderError::ContextTooLong {
            max_tokens: 200_000,
            requested_tokens: 210_000,
        })
        .with_response_end_turn("ok")
        .build();
    let request_lengths = Arc::new(Mutex::new(Vec::new()));
    let recording = RecordingProvider {
        inner: provider,
        request_lengths: request_lengths.clone(),
    };

    // Auto-compaction is off by default (no threshold), so the only compaction
    // is the reactive one. Seed a multi-message history so the compacted retry
    // is observably smaller (1 message) than the initial request (3 messages).
    let cfg = AgentConfig {
        model: "mock".into(),
        ..Default::default()
    };
    let agent = Arc::new(
        Agent::builder()
            .provider(Arc::new(recording))
            .config(cfg)
            .compactor(Arc::new(RecordingCompactor {
                calls: Arc::new(AtomicU32::new(0)),
            }))
            .build()
            .expect("agent"),
    );

    let history = vec![
        Message::user_text("first"),
        Message::assistant_text("earlier reply"),
        Message::user_text("hi"),
    ];
    let mut stream = agent.stream_until_done(history, CancellationToken::new());
    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev {
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "expected EndOfTurn after reactive compaction recovered the run, got {last_stop:?}"
    );

    let lengths = request_lengths.lock().expect("lock").clone();
    assert_eq!(
        lengths.len(),
        2,
        "expected two provider stream calls: {lengths:?}"
    );
    assert_eq!(lengths[0], 3, "first request sees the full seeded history");
    assert_eq!(
        lengths[1], 1,
        "retry must use the compacted history (last user message only): {lengths:?}"
    );
}

#[tokio::test]
async fn second_context_too_long_surrenders() {
    let provider = MockProvider::builder()
        .with_error_once(ProviderError::ContextTooLong {
            max_tokens: 200_000,
            requested_tokens: 210_000,
        })
        .with_error_once(ProviderError::ContextTooLong {
            max_tokens: 200_000,
            requested_tokens: 210_000,
        })
        .build();
    let agent = agent_with(
        provider,
        Arc::new(RecordingCompactor {
            calls: Arc::new(AtomicU32::new(0)),
        }),
    );

    let mut stream =
        agent.stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev {
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        matches!(last_stop, Some(StopCondition::ProviderError(_))),
        "expected ProviderError, got {last_stop:?}"
    );
}
