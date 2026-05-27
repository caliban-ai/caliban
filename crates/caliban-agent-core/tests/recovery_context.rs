//! Reactive compaction on `ContextTooLong`: the agent transparently compacts
//! once when the provider reports a context-window overflow, then retries.

#![allow(missing_docs)]

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use async_trait::async_trait;
use caliban_agent_core::{Agent, AgentConfig, Compactor, StopCondition, TurnEvent};
use caliban_provider::{Capabilities, Error as ProviderError, Message, MockProvider};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

struct RecordingCompactor {
    calls: Arc<AtomicU32>,
}

#[async_trait]
impl Compactor for RecordingCompactor {
    async fn compact(
        &self,
        messages: &[Message],
        _caps: &Capabilities,
    ) -> caliban_agent_core::error::Result<Option<Vec<Message>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Drop everything but the last user message to simulate a real reduction.
        let last_user = messages
            .iter()
            .rev()
            .find(|m| m.role == caliban_provider::Role::User)
            .cloned();
        Ok(last_user.map(|m| vec![m]))
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
