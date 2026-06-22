//! Integration tests for the empty/degenerate-turn guard (#249).
//!
//! Some Ollama reasoning models (gemma-family, via the native `/api/chat`
//! endpoint) intermittently end a turn after emitting only a `thinking` block —
//! no tool call, no final text — while still consuming output tokens. Without a
//! guard the agent loop treats that natural `EndTurn` as a clean completion and
//! ends the run as a silent "success" with no work done (the bug behind #249).
//!
//! The guard detects a degenerate turn (output tokens > 0, but no tool call and
//! no actionable text) and injects a single neutral nudge, taking another turn
//! instead of ending. It is bounded by `empty_turn_nudge_max` consecutive
//! nudges so a model that keeps stalling cannot loop forever; the streak resets
//! the moment a productive turn occurs.
//!
//! Run with:
//!
//! ```text
//! cargo test -p caliban-agent-core --features caliban-provider/mock empty_turn
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
// Stream-event builders
// ---------------------------------------------------------------------------

/// A degenerate turn: a single `thinking` block, no text, no tool call, ending
/// naturally with `EndTurn` but having consumed output tokens. Mirrors the
/// gemma "reasoned, then stopped without acting" wire shape from #249.
fn thinking_only_stream(
    msg_id: &str,
    thinking: &str,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.to_owned(),
            model: "mock-model".to_owned(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Thinking,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Thinking(thinking.to_owned()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage {
                input_tokens: 10,
                // Non-zero output: the model *did* generate (reasoning), which
                // is what makes the empty turn a degenerate stall rather than a
                // legitimately empty completion.
                output_tokens: 7,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

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
// A read-only tool so the post-nudge turn has something to call.
// ---------------------------------------------------------------------------

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

fn provider_arc(mock: Arc<MockProvider>) -> Arc<dyn Provider + Send + Sync> {
    mock as Arc<dyn Provider + Send + Sync>
}

fn build_agent(
    mock: Arc<MockProvider>,
    registry: ToolRegistry,
    empty_turn_nudge_max: u32,
) -> Arc<Agent> {
    Arc::new(
        Agent::builder()
            .provider(provider_arc(mock))
            .tools(registry)
            .config(AgentConfig {
                model: "mock-model".to_owned(),
                max_tokens: 1024,
                max_turns: 50,
                // Disable the no-edit nudge so it can't confound these tests.
                no_edit_nudge_threshold: 0,
                empty_turn_nudge_max,
                ..AgentConfig::default()
            })
            .build()
            .expect("agent should build"),
    )
}

const EMPTY_TURN_NUDGE_MARKER: &str = "no tool call";

fn count_empty_turn_nudges(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|m| m.role == Role::User)
        .filter(|m| {
            m.content.iter().any(|b| match b {
                ContentBlock::Text(t) => t.text.contains(EMPTY_TURN_NUDGE_MARKER),
                _ => false,
            })
        })
        .count()
}

// ---------------------------------------------------------------------------
// Test 1 — a thinking-only turn is nudged and the run continues past it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn thinking_only_turn_is_nudged_and_run_continues() {
    let mock = Arc::new(MockProvider::new());
    // Turn 0: degenerate (thinking only, EndTurn) — the gemma stall.
    mock.enqueue_stream(thinking_only_stream(
        "m0",
        "I should look at the file first.",
    ));
    // After the nudge the model acts (turn 1) then ends cleanly (turn 2).
    mock.enqueue_stream(tool_use_stream("m1", "c1", "peek"));
    mock.enqueue_stream(text_stream("m_end", "all done", StopReason::EndTurn));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadOnlyTool));
    let agent = build_agent(Arc::clone(&mock), registry, 2);

    let outcome = agent
        .run_until_done(vec![Message::user_text("fix it")], CancellationToken::new())
        .await
        .expect("run should succeed");

    assert_eq!(
        count_empty_turn_nudges(&outcome.final_messages),
        1,
        "exactly one empty-turn nudge must be injected after the degenerate turn"
    );
    assert!(
        outcome.turn_count >= 3,
        "the run must continue past the degenerate turn (consume the later tool + text turns); got turn_count={}",
        outcome.turn_count
    );
}

// ---------------------------------------------------------------------------
// Test 2 — the guard is bounded: at most `empty_turn_nudge_max` consecutive
// nudges, then the run ends rather than looping forever.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_turn_nudge_is_bounded() {
    let mock = Arc::new(MockProvider::new());
    // Three back-to-back degenerate turns. With max=2: turn0→nudge#1,
    // turn1→nudge#2, turn2→budget exhausted → run ends.
    mock.enqueue_stream(thinking_only_stream("m0", "thinking..."));
    mock.enqueue_stream(thinking_only_stream("m1", "still thinking..."));
    mock.enqueue_stream(thinking_only_stream("m2", "more thinking..."));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadOnlyTool));
    let agent = build_agent(Arc::clone(&mock), registry, 2);

    let outcome = agent
        .run_until_done(vec![Message::user_text("fix it")], CancellationToken::new())
        .await
        .expect("run should succeed");

    assert_eq!(
        count_empty_turn_nudges(&outcome.final_messages),
        2,
        "the guard must inject at most empty_turn_nudge_max (=2) nudges, then stop"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — a normal text completion is NOT degenerate and must never be nudged
// (guards against regressing well-behaved models, e.g. qwen).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn normal_text_completion_is_not_nudged() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(text_stream(
        "m_end",
        "here is the answer",
        StopReason::EndTurn,
    ));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadOnlyTool));
    let agent = build_agent(Arc::clone(&mock), registry, 2);

    let outcome = agent
        .run_until_done(
            vec![Message::user_text("answer me")],
            CancellationToken::new(),
        )
        .await
        .expect("run should succeed");

    assert_eq!(
        count_empty_turn_nudges(&outcome.final_messages),
        0,
        "a turn with real text content is a legitimate completion, not a degenerate turn"
    );
    assert_eq!(
        outcome.turn_count, 1,
        "a clean text completion must end the run in a single turn"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — `empty_turn_nudge_max = 0` disables the guard entirely (a degenerate
// turn ends the run, preserving the legacy behavior for callers that opt out).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_turn_nudge_max_zero_disables_guard() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(thinking_only_stream("m0", "thinking, then stopping"));
    // A second turn is enqueued but must NOT be consumed when the guard is off.
    mock.enqueue_stream(tool_use_stream("m1", "c1", "peek"));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadOnlyTool));
    let agent = build_agent(Arc::clone(&mock), registry, 0);

    let outcome = agent
        .run_until_done(vec![Message::user_text("fix it")], CancellationToken::new())
        .await
        .expect("run should succeed");

    assert_eq!(
        count_empty_turn_nudges(&outcome.final_messages),
        0,
        "empty_turn_nudge_max=0 must never inject a nudge"
    );
    assert_eq!(
        outcome.turn_count, 1,
        "with the guard disabled, a degenerate turn ends the run in one turn"
    );
}
