//! Confirms the agent loop fires `before_run` / `after_run` exactly once per
//! `stream_until_done` invocation (ADR 0028).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, HookDecision, Hooks, Message, Result, RunCtx, RunHookOutcome, ToolCtx, TurnCtx,
};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Counter {
    before_run: AtomicUsize,
    after_run: AtomicUsize,
}

#[async_trait]
impl Hooks for Counter {
    async fn before_run(&self, _ctx: &RunCtx<'_>) -> Result<()> {
        self.before_run.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    async fn after_run(&self, _ctx: &RunCtx<'_>, _outcome: &RunHookOutcome) -> Result<()> {
        self.after_run.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    // Implement before_turn so the auto-no-op contract works; not asserted on.
    async fn before_turn(&self, _ctx: &TurnCtx<'_>) -> Result<()> {
        Ok(())
    }
    async fn before_tool(&self, _ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        Ok(HookDecision::Allow)
    }
}

fn text_stream_events(
    id: &str,
    model: &str,
    text: &str,
    stop: StopReason,
) -> Vec<std::result::Result<StreamEvent, caliban_provider::Error>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: id.into(),
            model: model.into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text(text.into()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(stop),
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

fn provider_arc(mock: Arc<MockProvider>) -> Arc<dyn Provider + Send + Sync> {
    mock
}

#[tokio::test]
async fn before_and_after_run_fire_once_per_run() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(text_stream_events(
        "msg1",
        "mock-model",
        "hi",
        StopReason::EndTurn,
    ));
    let counter = Arc::new(Counter::default());
    let agent = Arc::new(
        Agent::builder()
            .provider(provider_arc(Arc::clone(&mock)))
            .model("mock-model")
            .max_tokens(64)
            .hooks(counter.clone() as Arc<dyn Hooks + Send + Sync>)
            .build()
            .expect("build"),
    );
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    while let Some(_evt) = stream.next().await {}
    assert_eq!(counter.before_run.load(Ordering::Relaxed), 1);
    assert_eq!(counter.after_run.load(Ordering::Relaxed), 1);
}
