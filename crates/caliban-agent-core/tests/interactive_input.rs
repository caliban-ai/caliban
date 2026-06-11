//! Tests for `InputProvider` run mode (ADR 0047 / #81).
//!
//! Verifies that:
//! - `None` → run ends after one natural turn.
//! - `Some` → run resumes with injected messages (uncapped).
//! - `MAX_FORCED_CONTINUATIONS` does NOT apply to `InputProvider` turns.
//! - `RunSettings::default()` (no provider) is byte-identical to original behaviour.
//!
//! Run with:
//! ```text
//! cargo test -p caliban-agent-core --features caliban-provider/mock interactive_input
//! ```

#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, InputProvider, Message, Role, RunSettings, StopCondition, TurnEvent,
};
use caliban_provider::{
    Capabilities, MockProvider, PromptCachingCapability, Provider, StopReason, StreamEvent,
    StreamingContentType, StreamingDelta, SystemPromptCapability, ToolUseCapability, Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Helpers — copied from integration.rs pattern
// ---------------------------------------------------------------------------

fn fake_caps() -> Capabilities {
    Capabilities {
        max_input_tokens: 100_000,
        max_output_tokens: 4096,
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

fn text_stream(stop: StopReason) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "msg_mock".to_owned(),
            model: "mock-model".to_owned(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text("ok".to_owned()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(stop),
            usage_delta: Some(Usage {
                input_tokens: 5,
                output_tokens: 2,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

fn provider_arc(mock: Arc<MockProvider>) -> Arc<dyn Provider + Send + Sync> {
    mock as Arc<dyn Provider + Send + Sync>
}

fn build_agent(mock: Arc<MockProvider>) -> Arc<Agent> {
    mock.set_capabilities(fake_caps());
    Arc::new(
        Agent::builder()
            .provider(provider_arc(mock))
            .model("mock-model")
            .max_tokens(256)
            .build()
            .expect("agent build"),
    )
}

// ---------------------------------------------------------------------------
// Test InputProvider implementations
// ---------------------------------------------------------------------------

/// Returns `None` immediately.
struct NoneProvider;

#[async_trait]
impl InputProvider for NoneProvider {
    async fn next_input(
        &self,
        _cancel: &tokio_util::sync::CancellationToken,
    ) -> Option<Vec<Message>> {
        None
    }
}

/// Returns `Some` for the first `resume_count` calls, then `None`.
struct CountingProvider {
    resume_count: u32,
    calls: AtomicU32,
}

impl CountingProvider {
    fn new(resume_count: u32) -> Self {
        Self {
            resume_count,
            calls: AtomicU32::new(0),
        }
    }
}

#[async_trait]
impl InputProvider for CountingProvider {
    async fn next_input(
        &self,
        _cancel: &tokio_util::sync::CancellationToken,
    ) -> Option<Vec<Message>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < self.resume_count {
            Some(vec![Message::user_text("continue")])
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: drain the stream and collect TurnStart/RunEnd events
// ---------------------------------------------------------------------------

struct RunSummary {
    turn_count: u32,
    stopped_for: StopCondition,
    final_messages: Vec<Message>,
}

async fn run_with_settings(agent: Arc<Agent>, settings: RunSettings) -> RunSummary {
    let cancel = CancellationToken::new();
    let mut stream =
        agent.stream_until_done_with_settings(vec![Message::user_text("hello")], cancel, settings);

    let mut turn_starts = 0u32;
    let mut result = None;

    while let Some(event) = stream.next().await {
        match event.expect("stream error") {
            TurnEvent::TurnStart { .. } => turn_starts += 1,
            TurnEvent::RunEnd {
                final_messages,
                turn_count,
                stopped_for,
                ..
            } => {
                result = Some(RunSummary {
                    turn_count,
                    stopped_for,
                    final_messages,
                });
            }
            _ => {}
        }
    }

    let mut summary = result.expect("stream must emit RunEnd");
    // Sanity: turn_starts matches RunEnd.turn_count
    assert_eq!(
        turn_starts, summary.turn_count,
        "TurnStart count should match RunEnd.turn_count"
    );
    summary.turn_count = turn_starts;
    summary
}

// ---------------------------------------------------------------------------
// Test 1: input_provider_none_ends_run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn input_provider_none_ends_run() {
    let mock = Arc::new(MockProvider::new());
    // One turn with EndTurn stop reason.
    mock.enqueue_stream(text_stream(StopReason::EndTurn));

    let agent = build_agent(Arc::clone(&mock));
    let settings = RunSettings {
        input_source: Some(Arc::new(NoneProvider)),
        ..RunSettings::default()
    };

    let summary = run_with_settings(agent, settings).await;

    assert_eq!(summary.turn_count, 1, "should run exactly 1 turn");
    assert!(
        matches!(summary.stopped_for, StopCondition::EndOfTurn),
        "expected EndOfTurn, got {:?}",
        summary.stopped_for
    );
}

// ---------------------------------------------------------------------------
// Test 2: input_provider_resumes_once_then_ends
// ---------------------------------------------------------------------------

#[tokio::test]
async fn input_provider_resumes_once_then_ends() {
    let mock = Arc::new(MockProvider::new());
    // Two provider turns: first ends naturally, second also ends naturally.
    mock.enqueue_stream(text_stream(StopReason::EndTurn));
    mock.enqueue_stream(text_stream(StopReason::EndTurn));

    let agent = build_agent(Arc::clone(&mock));
    // Provider returns Some once (injecting "continue"), then None.
    let settings = RunSettings {
        input_source: Some(Arc::new(CountingProvider::new(1))),
        ..RunSettings::default()
    };

    let summary = run_with_settings(agent, settings).await;

    assert_eq!(summary.turn_count, 2, "should run exactly 2 turns");
    assert!(
        matches!(summary.stopped_for, StopCondition::EndOfTurn),
        "expected EndOfTurn, got {:?}",
        summary.stopped_for
    );

    // The injected "continue" message should appear in final_messages.
    let injected = summary
        .final_messages
        .iter()
        .filter(|m| {
            m.role == Role::User
                && m.content.iter().any(|b| {
                    matches!(b, caliban_agent_core::ContentBlock::Text(t) if t.text == "continue")
                })
        })
        .count();
    assert_eq!(
        injected, 1,
        "injected 'continue' message should be in final_messages"
    );
}

// ---------------------------------------------------------------------------
// Test 3: input_provider_is_not_capped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn input_provider_is_not_capped() {
    use caliban_agent_core::stream::MAX_FORCED_CONTINUATIONS;

    let resume_count = u32::from(MAX_FORCED_CONTINUATIONS) + 2; // 5
    let mock = Arc::new(MockProvider::new());
    // Enqueue enough turns for resume_count + 1 final turn.
    for _ in 0..=(resume_count) {
        mock.enqueue_stream(text_stream(StopReason::EndTurn));
    }

    let agent = build_agent(Arc::clone(&mock));
    let settings = RunSettings {
        input_source: Some(Arc::new(CountingProvider::new(resume_count))),
        ..RunSettings::default()
    };

    let summary = run_with_settings(agent, settings).await;

    // Should have completed resume_count + 1 turns total, well above the cap.
    assert!(
        summary.turn_count > u32::from(MAX_FORCED_CONTINUATIONS),
        "human-driven turns must not be capped by MAX_FORCED_CONTINUATIONS ({MAX_FORCED_CONTINUATIONS}), got {} turns",
        summary.turn_count
    );
    assert!(
        matches!(summary.stopped_for, StopCondition::EndOfTurn),
        "expected EndOfTurn, got {:?}",
        summary.stopped_for
    );
}

// ---------------------------------------------------------------------------
// Test 4: no_input_source_matches_run_to_completion (regression guard)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_input_source_matches_run_to_completion() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(text_stream(StopReason::EndTurn));

    let agent = build_agent(Arc::clone(&mock));
    // Default settings: input_source is None.
    let settings = RunSettings::default();
    assert!(
        settings.input_source.is_none(),
        "RunSettings::default() must have input_source = None"
    );

    let summary = run_with_settings(agent, settings).await;

    assert_eq!(summary.turn_count, 1, "should run exactly 1 turn");
    assert!(
        matches!(summary.stopped_for, StopCondition::EndOfTurn),
        "expected EndOfTurn, got {:?}",
        summary.stopped_for
    );
}
