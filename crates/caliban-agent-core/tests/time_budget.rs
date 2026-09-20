//! ADR 0058 / #662: wall-clock time budget for the agent loop. A configured
//! `AgentConfig::time_budget` ends the run with `StopCondition::TimeBudgetExceeded`
//! at the top of the first turn that starts at or after the deadline. `None`
//! (default) means no deadline — today's behavior.

#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use caliban_agent_core::{Agent, AgentConfig, StopCondition, TurnEvent};
use caliban_provider::{
    Message, MockProvider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

/// A plain productive turn: one text block then a natural `EndTurn`.
fn text_turn(final_text: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "msg".into(),
            model: "mock".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text(final_text.to_owned()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

async fn run(cfg: AgentConfig, provider: MockProvider) -> (Option<StopCondition>, String) {
    let agent = Arc::new(
        Agent::builder()
            .provider(Arc::new(provider))
            .config(cfg)
            .build()
            .expect("agent"),
    );
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("go")], CancellationToken::new());
    let mut last_stop = None;
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        match item.expect("no stream error") {
            TurnEvent::AssistantTextDelta { text: frag, .. } => text.push_str(&frag),
            TurnEvent::RunEnd { stopped_for, .. } => last_stop = Some(stopped_for),
            _ => {}
        }
    }
    (last_stop, text)
}

#[tokio::test]
async fn time_budget_exceeded_stops_the_run() {
    // A zero-second budget is already over budget: the top-of-turn deadline
    // check (`elapsed >= budget`) fires before the first turn, so the provider
    // stream is never consumed and the run ends with TimeBudgetExceeded.
    let provider = MockProvider::new();
    provider.enqueue_stream(text_turn("should never be produced"));
    let cfg = AgentConfig {
        model: "mock".into(),
        time_budget: Some(Duration::ZERO),
        ..Default::default()
    };
    let start = std::time::Instant::now();
    let (last_stop, text) = run(cfg, provider).await;
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "must stop promptly, not hang: {:?}",
        start.elapsed()
    );
    assert!(
        matches!(last_stop, Some(StopCondition::TimeBudgetExceeded(d)) if d == Duration::ZERO),
        "expected TimeBudgetExceeded(0), got {last_stop:?}"
    );
    assert!(
        text.is_empty(),
        "no turn should run once the deadline has passed, got text {text:?}"
    );
}

#[tokio::test]
async fn ample_time_budget_does_not_fire() {
    // A generous budget never trips on a fast run; it completes normally.
    let provider = MockProvider::new();
    provider.enqueue_stream(text_turn("the answer"));
    let cfg = AgentConfig {
        model: "mock".into(),
        time_budget: Some(Duration::from_hours(1)),
        ..Default::default()
    };
    let (last_stop, text) = run(cfg, provider).await;
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "an ample budget must not fire; expected EndOfTurn, got {last_stop:?}"
    );
    assert_eq!(text, "the answer");
}

#[tokio::test]
async fn no_time_budget_is_the_default_and_imposes_no_deadline() {
    // `None` (the default) is today's behavior: no deadline, run completes.
    let provider = MockProvider::new();
    provider.enqueue_stream(text_turn("done"));
    let cfg = AgentConfig {
        model: "mock".into(),
        ..Default::default()
    };
    assert_eq!(cfg.time_budget, None, "time_budget defaults to None");
    let (last_stop, text) = run(cfg, provider).await;
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "no budget → normal completion, got {last_stop:?}"
    );
    assert_eq!(text, "done");
}
