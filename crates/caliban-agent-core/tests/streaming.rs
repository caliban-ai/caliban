//! Streaming-specific integration tests (Scenario 11 from the spec).
//!
//! These tests drive `Agent::stream_until_done` directly and verify the
//! sequence and content of `TurnEvent`s.

#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, ContentBlock, StopCondition, TextBlock, Tool, ToolContext, ToolError, ToolRegistry,
    TurnEvent,
};
use caliban_provider::{
    Message, MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta,
    Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn provider_arc(mock: Arc<MockProvider>) -> Arc<dyn Provider + Send + Sync> {
    mock as Arc<dyn Provider + Send + Sync>
}

fn text_stream_events(
    msg_id: &str,
    model: &str,
    text: &str,
    stop: StopReason,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.to_owned(),
            model: model.to_owned(),
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
                input_tokens: 8,
                output_tokens: 4,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

fn tool_use_stream_events(
    msg_id: &str,
    model: &str,
    tool_use_id: &str,
    tool_name: &str,
    input_json: &str,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.to_owned(),
            model: model.to_owned(),
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
            delta: StreamingDelta::ToolUseInputJson(input_json.to_owned()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            usage_delta: Some(Usage {
                input_tokens: 8,
                output_tokens: 3,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

struct EchoTool {
    count: Arc<AtomicU32>,
}

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }
    fn description(&self) -> &'static str {
        "Echoes back"
    }
    fn input_schema(&self) -> &serde_json::Value {
        static S: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        S.get_or_init(|| serde_json::json!({"type": "object"}))
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> std::result::Result<Vec<ContentBlock>, ToolError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(vec![ContentBlock::Text(TextBlock {
            text: "echoed".to_owned(),
            cache_control: None,
        })])
    }
}

// ---------------------------------------------------------------------------
// Scenario 11a — text-only stream emits expected event sequence
// ---------------------------------------------------------------------------

/// Verify the event sequence for a single-turn text-only response:
/// `TurnStart` → `AssistantTextDelta` → `TurnEnd` → `RunEnd`
#[tokio::test]
async fn stream_text_only_emits_all_events() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(text_stream_events(
        "msg1",
        "mock-model",
        "Hello from stream!",
        StopReason::EndTurn,
    ));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider_arc(Arc::clone(&mock)))
            .model("mock-model")
            .max_tokens(1024)
            .build()
            .expect("build"),
    );

    let mut events: Vec<TurnEvent> = Vec::new();
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    while let Some(ev) = stream.next().await {
        events.push(ev.expect("event should not error"));
    }

    // Verify ordering: TurnStart comes before AssistantTextDelta.
    let turn_start_idx = events
        .iter()
        .position(|e| matches!(e, TurnEvent::TurnStart { .. }))
        .expect("TurnStart missing");
    let text_delta_idx = events
        .iter()
        .position(|e| matches!(e, TurnEvent::AssistantTextDelta { .. }))
        .expect("AssistantTextDelta missing");
    let turn_end_idx = events
        .iter()
        .position(|e| matches!(e, TurnEvent::TurnEnd { .. }))
        .expect("TurnEnd missing");
    let run_end_idx = events
        .iter()
        .position(|e| matches!(e, TurnEvent::RunEnd { .. }))
        .expect("RunEnd missing");

    assert!(
        turn_start_idx < text_delta_idx,
        "TurnStart before AssistantTextDelta"
    );
    assert!(
        text_delta_idx < turn_end_idx,
        "AssistantTextDelta before TurnEnd"
    );
    assert!(turn_end_idx < run_end_idx, "TurnEnd before RunEnd");

    // Verify TurnStart carries model name.
    if let TurnEvent::TurnStart { model, .. } = &events[turn_start_idx] {
        assert_eq!(model, "mock-model");
    }

    // Verify AssistantTextDelta carries the text.
    if let TurnEvent::AssistantTextDelta { text, .. } = &events[text_delta_idx] {
        assert_eq!(text, "Hello from stream!");
    }

    // Verify TurnEnd stop reason.
    if let TurnEvent::TurnEnd { stop_reason, .. } = &events[turn_end_idx] {
        assert_eq!(*stop_reason, StopReason::EndTurn);
    }

    // Verify RunEnd carry correct stopped_for.
    if let TurnEvent::RunEnd { stopped_for, .. } = &events[run_end_idx] {
        assert!(
            matches!(stopped_for, StopCondition::EndOfTurn),
            "expected EndOfTurn, got {stopped_for:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Scenario 11b — tool-use stream emits ToolCallStart / ToolCallInputDelta / ToolCallEnd
// ---------------------------------------------------------------------------

/// Verify the event sequence for a tool-use turn:
/// `TurnStart` → `ToolCallStart` → `ToolCallInputDelta` → `ToolCallEnd` → `TurnEnd`
/// → (turn 2) `TurnStart` → `AssistantTextDelta` → `TurnEnd` → `RunEnd`
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn stream_tool_use_emits_tool_events() {
    let mock = Arc::new(MockProvider::new());
    // Turn 1: tool use.
    mock.enqueue_stream(tool_use_stream_events(
        "msg1",
        "mock-model",
        "tu_stream_1",
        "echo",
        r#"{"x":1}"#,
    ));
    // Turn 2: text end.
    mock.enqueue_stream(text_stream_events(
        "msg2",
        "mock-model",
        "Done!",
        StopReason::EndTurn,
    ));

    let invocations = Arc::new(AtomicU32::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool {
        count: Arc::clone(&invocations),
    }));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider_arc(Arc::clone(&mock)))
            .tools(registry)
            .model("mock-model")
            .max_tokens(1024)
            .build()
            .expect("build"),
    );

    let mut events: Vec<TurnEvent> = Vec::new();
    let mut stream = agent.stream_until_done(
        vec![Message::user_text("call echo")],
        CancellationToken::new(),
    );
    while let Some(ev) = stream.next().await {
        events.push(ev.expect("event should not error"));
    }

    // All required event types must appear.
    let has = |variant: fn(&TurnEvent) -> bool| events.iter().any(variant);

    assert!(
        has(|e| matches!(e, TurnEvent::TurnStart { .. })),
        "TurnStart missing"
    );
    assert!(
        has(|e| matches!(e, TurnEvent::ToolCallStart { .. })),
        "ToolCallStart missing"
    );
    assert!(
        has(|e| matches!(e, TurnEvent::ToolCallInputDelta { .. })),
        "ToolCallInputDelta missing"
    );
    assert!(
        has(|e| matches!(e, TurnEvent::ToolCallEnd { .. })),
        "ToolCallEnd missing"
    );
    assert!(
        has(|e| matches!(e, TurnEvent::TurnEnd { .. })),
        "TurnEnd missing"
    );
    assert!(
        has(|e| matches!(e, TurnEvent::AssistantTextDelta { .. })),
        "AssistantTextDelta missing"
    );
    assert!(
        has(|e| matches!(e, TurnEvent::RunEnd { .. })),
        "RunEnd missing"
    );

    // Verify ToolCallStart carries the tool name.
    let tool_start = events
        .iter()
        .find(|e| matches!(e, TurnEvent::ToolCallStart { .. }))
        .unwrap();
    if let TurnEvent::ToolCallStart {
        name, tool_use_id, ..
    } = tool_start
    {
        assert_eq!(name, "echo");
        assert_eq!(tool_use_id, "tu_stream_1");
    }

    // Verify ToolCallInputDelta carries the JSON fragment.
    let tool_delta = events
        .iter()
        .find(|e| matches!(e, TurnEvent::ToolCallInputDelta { .. }))
        .unwrap();
    if let TurnEvent::ToolCallInputDelta { partial_json, .. } = tool_delta {
        assert!(partial_json.contains('x') || partial_json.contains('1'));
    }

    // ToolCallEnd should have is_error: false (echo succeeds).
    let tool_end = events
        .iter()
        .find(|e| matches!(e, TurnEvent::ToolCallEnd { .. }))
        .unwrap();
    if let TurnEvent::ToolCallEnd {
        is_error,
        tool_use_id,
        ..
    } = tool_end
    {
        assert!(!is_error, "echo tool should not produce an error");
        assert_eq!(tool_use_id, "tu_stream_1");
    }

    // RunEnd should report EndOfTurn and 2 turns.
    let run_end = events
        .iter()
        .find(|e| matches!(e, TurnEvent::RunEnd { .. }))
        .unwrap();
    if let TurnEvent::RunEnd {
        stopped_for,
        turn_count,
        ..
    } = run_end
    {
        assert!(matches!(stopped_for, StopCondition::EndOfTurn));
        assert_eq!(*turn_count, 2);
    }

    assert_eq!(invocations.load(Ordering::SeqCst), 1, "echo called once");
}

// ---------------------------------------------------------------------------
// Scenario 11c — RunEnd is always the last event
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_end_is_always_last_event() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(text_stream_events(
        "m1",
        "mock-model",
        "hi",
        StopReason::EndTurn,
    ));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider_arc(Arc::clone(&mock)))
            .model("mock-model")
            .max_tokens(1024)
            .build()
            .expect("build"),
    );

    let mut events: Vec<TurnEvent> = Vec::new();
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("hello")], CancellationToken::new());
    while let Some(ev) = stream.next().await {
        events.push(ev.expect("no error expected"));
    }

    assert!(!events.is_empty(), "should have at least one event");
    assert!(
        matches!(events.last().unwrap(), TurnEvent::RunEnd { .. }),
        "last event must be RunEnd"
    );
}

// ---------------------------------------------------------------------------
// Scenario 11d — usage accumulates across turns in RunEnd
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_end_accumulates_usage() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(tool_use_stream_events(
        "m1",
        "mock-model",
        "tu1",
        "echo",
        "{}",
    ));
    mock.enqueue_stream(text_stream_events(
        "m2",
        "mock-model",
        "ok",
        StopReason::EndTurn,
    ));

    let invocations = Arc::new(AtomicU32::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool {
        count: Arc::clone(&invocations),
    }));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider_arc(Arc::clone(&mock)))
            .tools(registry)
            .model("mock-model")
            .max_tokens(1024)
            .build()
            .expect("build"),
    );

    let mut total_from_turn_ends = Usage::default();
    let mut run_end_usage = Usage::default();

    let mut stream =
        agent.stream_until_done(vec![Message::user_text("go")], CancellationToken::new());
    while let Some(ev) = stream.next().await {
        let ev = ev.expect("no error");
        match &ev {
            TurnEvent::TurnEnd { usage, .. } => {
                total_from_turn_ends.merge(*usage);
            }
            TurnEvent::RunEnd { total_usage, .. } => {
                run_end_usage = *total_usage;
            }
            _ => {}
        }
    }

    // Usage in RunEnd should equal the sum of TurnEnd usages.
    assert_eq!(
        run_end_usage.input_tokens, total_from_turn_ends.input_tokens,
        "RunEnd usage should sum TurnEnd usages"
    );
    assert_eq!(
        run_end_usage.output_tokens,
        total_from_turn_ends.output_tokens
    );
}
