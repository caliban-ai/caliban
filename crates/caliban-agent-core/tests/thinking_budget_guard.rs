//! #62: per-turn thinking-char cap. A model that streams thinking past
//! `max_turn_thinking_chars` without a tool call or final text terminates the
//! run with `StopCondition::ThinkingBudgetExhausted` instead of hanging.

#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use caliban_agent_core::{Agent, AgentConfig, StopCondition, TurnEvent};
use caliban_provider::{
    Message, MockProvider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

/// A thinking-only turn: N thinking deltas, then a natural end. Used to exceed
/// the cap; when the guard trips the natural end is never reached.
fn thinking_only(deltas: &[&str]) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    let mut evs = vec![
        Ok(StreamEvent::MessageStart {
            id: "msg_think".into(),
            model: "mock".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Thinking,
        }),
    ];
    for d in deltas {
        evs.push(Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Thinking((*d).to_owned()),
        }));
    }
    evs.push(Ok(StreamEvent::ContentBlockStop { index: 0 }));
    evs.push(Ok(StreamEvent::MessageDelta {
        stop_reason: Some(StopReason::EndTurn),
        usage_delta: Some(Usage::default()),
    }));
    evs.push(Ok(StreamEvent::MessageStop));
    evs
}

/// A productive turn: some thinking, then a final text answer, then `EndTurn`.
fn thinking_then_text(
    thinking: &[&str],
    final_text: &str,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    let mut evs = vec![
        Ok(StreamEvent::MessageStart {
            id: "msg_mix".into(),
            model: "mock".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Thinking,
        }),
    ];
    for d in thinking {
        evs.push(Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Thinking((*d).to_owned()),
        }));
    }
    evs.push(Ok(StreamEvent::ContentBlockStop { index: 0 }));
    evs.push(Ok(StreamEvent::ContentBlockStart {
        index: 1,
        content_type: StreamingContentType::Text,
    }));
    evs.push(Ok(StreamEvent::Delta {
        index: 1,
        delta: StreamingDelta::Text(final_text.to_owned()),
    }));
    evs.push(Ok(StreamEvent::ContentBlockStop { index: 1 }));
    evs.push(Ok(StreamEvent::MessageDelta {
        stop_reason: Some(StopReason::EndTurn),
        usage_delta: Some(Usage::default()),
    }));
    evs.push(Ok(StreamEvent::MessageStop));
    evs
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
async fn thinking_over_cap_trips_guard() {
    let provider = MockProvider::new();
    // 5 × 40 chars = 200 thinking chars, cap 100 → trips on the 3rd delta.
    let forty = "x".repeat(40);
    let deltas: Vec<&str> = vec![forty.as_str(); 5];
    provider.enqueue_stream(thinking_only(&deltas));
    let cfg = AgentConfig {
        model: "mock".into(),
        max_turn_thinking_chars: 100,
        ..Default::default()
    };
    let start = std::time::Instant::now();
    let (last_stop, _) = run(cfg, provider).await;
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "guard must abort promptly, not hang: {:?}",
        start.elapsed()
    );
    assert!(
        matches!(last_stop, Some(StopCondition::ThinkingBudgetExhausted)),
        "expected ThinkingBudgetExhausted, got {last_stop:?}"
    );
}

#[tokio::test]
async fn thinking_under_cap_completes() {
    let provider = MockProvider::new();
    // 60 thinking chars under a 100k cap, then a real answer.
    provider.enqueue_stream(thinking_then_text(
        &["thinking a bit", " more"],
        "the answer",
    ));
    let cfg = AgentConfig {
        model: "mock".into(),
        max_turn_thinking_chars: 100_000,
        ..Default::default()
    };
    let (last_stop, text) = run(cfg, provider).await;
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "under-cap productive turn should complete with EndOfTurn, got {last_stop:?}"
    );
    assert_eq!(text, "the answer", "the final text must stream through");
}

#[tokio::test]
async fn cap_zero_disables_guard() {
    let provider = MockProvider::new();
    // Lots of thinking that would trip any small cap, then an answer; cap 0 off.
    let big = "y".repeat(500);
    provider.enqueue_stream(thinking_then_text(&[big.as_str()], "done"));
    let cfg = AgentConfig {
        model: "mock".into(),
        max_turn_thinking_chars: 0,
        ..Default::default()
    };
    let (last_stop, text) = run(cfg, provider).await;
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "cap 0 disables the guard; turn should complete, got {last_stop:?}"
    );
    assert_eq!(text, "done");
}
