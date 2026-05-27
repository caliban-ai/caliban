//! Refusal / `ContentFilter`: surface a synthetic assistant message and a
//! distinct `StopCondition` variant.

#![allow(missing_docs)]

use std::sync::Arc;

use caliban_agent_core::{Agent, AgentConfig, StopCondition, TurnEvent};
use caliban_provider::{ContentBlock, Message, MockProvider, Role, StopReason};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

fn agent_with(provider: MockProvider) -> Arc<Agent> {
    let cfg = AgentConfig {
        model: "mock".into(),
        ..Default::default()
    };
    Arc::new(
        Agent::builder()
            .provider(Arc::new(provider))
            .config(cfg)
            .build()
            .expect("agent"),
    )
}

#[tokio::test]
async fn refusal_emits_synthetic_message_and_distinct_stop() {
    let provider = MockProvider::builder()
        .with_response_stop_reason(StopReason::Refusal, "")
        .build();
    let agent = agent_with(provider);
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut final_history = Vec::new();
    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd {
            final_messages,
            stopped_for,
            ..
        } = ev
        {
            final_history = final_messages;
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        matches!(last_stop, Some(StopCondition::Refusal(_))),
        "expected Refusal, got {last_stop:?}"
    );
    let last = final_history.last().expect("at least one message");
    assert_eq!(last.role, Role::Assistant);
    assert!(matches!(
        &last.content[0],
        ContentBlock::Text(t) if t.text == "Model declined to respond."
    ));
}

#[tokio::test]
async fn content_filter_emits_synthetic_and_distinct_stop() {
    let provider = MockProvider::builder()
        .with_response_stop_reason(StopReason::ContentFilter, "")
        .build();
    let agent = agent_with(provider);
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());
    let mut last_stop = None;
    let mut final_history = Vec::new();
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd {
            final_messages,
            stopped_for,
            ..
        } = ev
        {
            final_history = final_messages;
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        matches!(last_stop, Some(StopCondition::ContentFilter(_))),
        "expected ContentFilter, got {last_stop:?}"
    );
    let last = final_history.last().unwrap();
    assert!(matches!(
        &last.content[0],
        ContentBlock::Text(t) if t.text == "Response blocked by content filter."
    ));
}
