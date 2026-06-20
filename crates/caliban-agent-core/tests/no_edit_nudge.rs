//! Integration tests for the no-edit-progress nudge (#239).
//!
//! The agent loop tracks how many consecutive turns have completed without a
//! successful non-read-only ("edit-class") tool call. When that count crosses
//! the configurable `no_edit_nudge_threshold`, the loop injects exactly one
//! neutral user-message nudge and surfaces telemetry on `RunOutcome`
//! (`turns_without_edit`, `no_edit_nudge_emitted`).
//!
//! Run with:
//!
//! ```text
//! cargo test -p caliban-agent-core --features caliban-provider/mock no_edit
//! ```

#![allow(missing_docs)]

use std::sync::Arc;

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, AgentConfig, ContentBlock, Message, Role, TextBlock, Tool, ToolContext, ToolError,
    ToolRegistry,
};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Stream-event builders (mirrors integration.rs helpers, trimmed)
// ---------------------------------------------------------------------------

fn text_stream(
    msg_id: &str,
    text: &str,
    stop: StopReason,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
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
            stop_reason: Some(stop),
            usage_delta: Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

fn tool_use_stream(
    msg_id: &str,
    tool_use_id: &str,
    tool_name: &str,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.to_owned(),
            model: "mock-model".to_owned(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::ToolUse {
                id: tool_use_id.to_owned(),
                name: tool_name.to_owned(),
            },
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::ToolUseInputJson("{}".to_owned()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            usage_delta: Some(Usage {
                input_tokens: 10,
                output_tokens: 3,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

// ---------------------------------------------------------------------------
// Mock tools: one read-only, one edit-class (non-read-only, succeeds)
// ---------------------------------------------------------------------------

/// A read-only tool (e.g. like `Read`/`Grep`): `is_read_only()` returns true.
struct ReadOnlyTool;

#[async_trait]
impl Tool for ReadOnlyTool {
    fn name(&self) -> &'static str {
        "peek"
    }
    fn description(&self) -> &'static str {
        "A read-only mock tool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| serde_json::json!({"type": "object", "properties": {}}))
    }
    fn is_read_only(&self) -> bool {
        true
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> std::result::Result<Vec<ContentBlock>, ToolError> {
        Ok(vec![ContentBlock::Text(TextBlock {
            text: "peeked".to_owned(),
            cache_control: None,
        })])
    }
}

/// An edit-class tool (NOT read-only) that succeeds.
struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }
    fn description(&self) -> &'static str {
        "An edit-class mock tool (mutates state)"
    }
    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| serde_json::json!({"type": "object", "properties": {}}))
    }
    // is_read_only defaults to false → counts as an edit.
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> std::result::Result<Vec<ContentBlock>, ToolError> {
        Ok(vec![ContentBlock::Text(TextBlock {
            text: "edited".to_owned(),
            cache_control: None,
        })])
    }
}

fn provider_arc(mock: Arc<MockProvider>) -> Arc<dyn Provider + Send + Sync> {
    mock as Arc<dyn Provider + Send + Sync>
}

fn build_agent(mock: Arc<MockProvider>, registry: ToolRegistry, threshold: u32) -> Arc<Agent> {
    Arc::new(
        Agent::builder()
            .provider(provider_arc(mock))
            .tools(registry)
            .config(AgentConfig {
                model: "mock-model".to_owned(),
                max_tokens: 1024,
                max_turns: 50,
                no_edit_nudge_threshold: threshold,
                ..AgentConfig::default()
            })
            .build()
            .expect("agent should build"),
    )
}

const NUDGE_MARKER: &str = "without editing any files";

fn count_nudge_messages(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|m| m.role == Role::User)
        .filter(|m| {
            m.content.iter().any(|b| match b {
                ContentBlock::Text(t) => t.text.contains(NUDGE_MARKER),
                _ => false,
            })
        })
        .count()
}

// ---------------------------------------------------------------------------
// Test 1 — read-only-only run crosses the threshold → exactly one nudge.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nudge_fires_once_after_threshold_of_no_edit_turns() {
    let mock = Arc::new(MockProvider::new());
    // Four read-only tool turns (turns_since_last_edit climbs 1→4), then a
    // clean text turn to end. With threshold=3 the nudge fires exactly once
    // (after the third no-edit turn); the fourth no-edit turn is disarmed.
    for i in 0..4 {
        mock.enqueue_stream(tool_use_stream(
            &format!("msg{i}"),
            &format!("call{i}"),
            "peek",
        ));
    }
    mock.enqueue_stream(text_stream("msg_end", "all done", StopReason::EndTurn));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadOnlyTool));
    let agent = build_agent(Arc::clone(&mock), registry, 3);

    let outcome = agent
        .run_until_done(
            vec![Message::user_text("investigate")],
            CancellationToken::new(),
        )
        .await
        .expect("run should succeed");

    assert_eq!(
        count_nudge_messages(&outcome.final_messages),
        1,
        "exactly one no-edit nudge must appear in final_messages"
    );
    assert!(
        outcome.no_edit_nudge_emitted,
        "no_edit_nudge_emitted must be true after the nudge fires"
    );
    assert!(
        outcome.turns_without_edit >= 3,
        "high-water turns_without_edit should reach at least the threshold; got {}",
        outcome.turns_without_edit
    );
}

// ---------------------------------------------------------------------------
// Test 2 — a successful edit before the threshold → no nudge.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_nudge_when_edit_resets_the_counter() {
    let mock = Arc::new(MockProvider::new());
    // peek, peek, EDIT (resets), peek, then end. With threshold=3 neither the
    // pre-edit streak (peek, peek = 2) nor the post-edit streak (peek, then the
    // final no-tool turn = 2) reaches 3, because the edit resets midstream.
    mock.enqueue_stream(tool_use_stream("m0", "c0", "peek"));
    mock.enqueue_stream(tool_use_stream("m1", "c1", "peek"));
    mock.enqueue_stream(tool_use_stream("m2", "c2", "edit"));
    mock.enqueue_stream(tool_use_stream("m3", "c3", "peek"));
    mock.enqueue_stream(text_stream("m_end", "done", StopReason::EndTurn));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadOnlyTool));
    registry.register(Arc::new(EditTool));
    let agent = build_agent(Arc::clone(&mock), registry, 3);

    let outcome = agent
        .run_until_done(vec![Message::user_text("fix it")], CancellationToken::new())
        .await
        .expect("run should succeed");

    assert_eq!(
        count_nudge_messages(&outcome.final_messages),
        0,
        "no nudge must appear when an edit resets the counter before the threshold"
    );
    assert!(
        !outcome.no_edit_nudge_emitted,
        "no_edit_nudge_emitted must be false when no nudge fires"
    );
    assert!(
        outcome.turns_without_edit < 3,
        "the edit must keep the high-water below the threshold; got {}",
        outcome.turns_without_edit
    );
}

// ---------------------------------------------------------------------------
// Test 3 — after a nudge fires and disarms, a successful edit re-arms it so
// a SECOND nudge can fire on a later no-edit streak.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nudge_rearms_after_edit_following_a_nudge() {
    // Sequence (threshold = 2):
    //   turn 0: peek → turns_since_last_edit = 1
    //   turn 1: peek → turns_since_last_edit = 2 → nudge #1 fires, disarms
    //   turn 2: edit → resets to 0, re-arms
    //   turn 3: peek → turns_since_last_edit = 1
    //   turn 4: peek → turns_since_last_edit = 2 → nudge #2 fires, disarms
    //   turn 5: text (end_turn)
    //
    // max_turns must cover all 6 provider turns plus two extra forced by nudge
    // injection (break 'inner advances turn_index). Set it to 20 to be safe.
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(tool_use_stream("m0", "c0", "peek"));
    mock.enqueue_stream(tool_use_stream("m1", "c1", "peek"));
    mock.enqueue_stream(tool_use_stream("m2", "c2", "edit"));
    mock.enqueue_stream(tool_use_stream("m3", "c3", "peek"));
    mock.enqueue_stream(tool_use_stream("m4", "c4", "peek"));
    mock.enqueue_stream(text_stream("m_end", "done", StopReason::EndTurn));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadOnlyTool));
    registry.register(Arc::new(EditTool));
    let agent = build_agent(Arc::clone(&mock), registry, 2);

    let outcome = agent
        .run_until_done(
            vec![Message::user_text("investigate and fix")],
            CancellationToken::new(),
        )
        .await
        .expect("run should succeed");

    assert_eq!(
        count_nudge_messages(&outcome.final_messages),
        2,
        "exactly two no-edit nudges must appear: one before the edit, one after the post-edit no-edit streak; got {}",
        count_nudge_messages(&outcome.final_messages),
    );
    assert!(
        outcome.no_edit_nudge_emitted,
        "no_edit_nudge_emitted must be true when at least one nudge fires"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — threshold = 0 disables the nudge entirely.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn threshold_zero_never_nudges() {
    let mock = Arc::new(MockProvider::new());
    for i in 0..6 {
        mock.enqueue_stream(tool_use_stream(&format!("m{i}"), &format!("c{i}"), "peek"));
    }
    mock.enqueue_stream(text_stream("m_end", "done", StopReason::EndTurn));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadOnlyTool));
    let agent = build_agent(Arc::clone(&mock), registry, 0);

    let outcome = agent
        .run_until_done(
            vec![Message::user_text("just look")],
            CancellationToken::new(),
        )
        .await
        .expect("run should succeed");

    assert_eq!(
        count_nudge_messages(&outcome.final_messages),
        0,
        "threshold=0 must never inject a nudge"
    );
    assert!(
        !outcome.no_edit_nudge_emitted,
        "no_edit_nudge_emitted must stay false when disabled"
    );
}
