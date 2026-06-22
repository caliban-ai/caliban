//! Integration tests for #245: retrying a provider stream that is interrupted
//! *before any content is emitted*.
//!
//! `with_retry` only covers stream *establishment*. A transient
//! `StreamInterrupted`/timeout that lands while draining the response used to
//! be terminal. The fix re-issues the turn (bounded) when the interruption
//! happens before any text/thinking/tool content has streamed; once content has
//! been emitted it stays terminal (replaying would double-emit).
//!
//! Run with:
//!
//! ```text
//! cargo test -p caliban-agent-core --features caliban-provider/mock midstream
//! ```

#![allow(missing_docs)]

use std::sync::Arc;

use caliban_agent_core::{Agent, AgentConfig, ContentBlock, Message, Role, StopCondition};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use tokio_util::sync::CancellationToken;

type Events = Vec<caliban_provider::error::Result<StreamEvent>>;

fn usage() -> Usage {
    Usage {
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    }
}

/// A clean text turn that ends the run.
fn text_stream(msg_id: &str, text: &str) -> Events {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.to_owned(),
            model: "mock-model".to_owned(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text(text.to_owned()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(usage()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

/// Stream that opens OK (`MessageStart`) then is interrupted **before** any
/// content block — the safe-to-retry case.
fn interrupted_before_content(msg_id: &str) -> Events {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.to_owned(),
            model: "mock-model".to_owned(),
        }),
        Err(caliban_provider::Error::StreamInterrupted(
            "boom".to_owned(),
        )),
    ]
}

/// Stream that emits a text delta (content!) and *then* is interrupted — must
/// NOT be retried (replay would double-emit).
fn interrupted_after_content(msg_id: &str) -> Events {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.to_owned(),
            model: "mock-model".to_owned(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text("partial answer".to_owned()),
        }),
        Err(caliban_provider::Error::StreamInterrupted(
            "boom".to_owned(),
        )),
    ]
}

fn build_agent(mock: Arc<MockProvider>) -> Arc<Agent> {
    Arc::new(
        Agent::builder()
            .provider(mock as Arc<dyn Provider + Send + Sync>)
            .tools(caliban_agent_core::ToolRegistry::new())
            .config(AgentConfig {
                model: "mock-model".to_owned(),
                max_tokens: 1024,
                max_turns: 50,
                // Irrelevant here; disable so it can't interfere.
                no_edit_nudge_threshold: 0,
                ..AgentConfig::default()
            })
            .build()
            .expect("agent should build"),
    )
}

fn assistant_text(messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<String>()
}

// ---------------------------------------------------------------------------
// Test 1 — interruption before any content → the turn is re-issued and the
// next (good) stream completes the run.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn midstream_interruption_before_content_is_retried() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(interrupted_before_content("m0"));
    mock.enqueue_stream(text_stream("m1", "recovered answer"));

    let outcome = build_agent(Arc::clone(&mock))
        .run_until_done(vec![Message::user_text("go")], CancellationToken::new())
        .await
        .expect("run should complete (Ok)");

    assert!(
        matches!(outcome.stopped_for, StopCondition::EndOfTurn),
        "a pre-content interruption must be retried and the run end cleanly; got {:?}",
        outcome.stopped_for
    );
    assert!(
        assistant_text(&outcome.final_messages).contains("recovered answer"),
        "the retried (second) stream's content must be the final answer"
    );
    assert_eq!(
        outcome.turn_count, 1,
        "the re-issue must not consume an extra turn slot; got {}",
        outcome.turn_count
    );
}

// ---------------------------------------------------------------------------
// Test 2 — interruption AFTER content was emitted → terminal (no retry), so
// the original interruption surfaces.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn midstream_interruption_after_content_is_terminal() {
    let mock = Arc::new(MockProvider::new());
    // Only the failing stream is enqueued. If the fix wrongly retried, it would
    // pull a (nonexistent) next stream and the error would differ; asserting the
    // original "boom" payload proves it did NOT retry.
    mock.enqueue_stream(interrupted_after_content("m0"));

    let outcome = build_agent(Arc::clone(&mock))
        .run_until_done(vec![Message::user_text("go")], CancellationToken::new())
        .await
        .expect("run should return Ok with an error stop condition");

    match outcome.stopped_for {
        StopCondition::ProviderError(msg) => assert!(
            msg.contains("boom"),
            "must surface the original mid-stream interruption, not a retry artifact; got {msg}"
        ),
        other => panic!("expected ProviderError after post-content interruption, got {other:?}"),
    }
}
