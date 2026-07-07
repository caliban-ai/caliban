//! #391: bounded, display-ready tool result on the agent event stream.
//!
//! Drives one tool-call turn through a `MockProvider` + a registered tool and
//! asserts the resulting `TurnEvent::ToolCallEnd` carries `result_text`
//! (flattened, capped) + `truncated`, correlatable to the call via
//! `tool_use_id`. Hermetic — no network.

#![allow(missing_docs)]

use std::sync::Arc;

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, ContentBlock, Message, STREAM_RESULT_TEXT_CAP, TextBlock, Tool, ToolContext, ToolError,
    ToolRegistry, TurnEvent,
};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

/// A tool that returns a fixed text body of a chosen size (or an error).
struct FixedTool {
    name: String,
    schema: serde_json::Value,
    /// `Ok(text)` → success result with that text; `Err(msg)` → tool error.
    body: Result<String, String>,
}

impl FixedTool {
    fn ok(name: &str, text: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            schema: serde_json::json!({"type": "object"}),
            body: Ok(text.into()),
        }
    }
    fn err(name: &str, msg: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            schema: serde_json::json!({"type": "object"}),
            body: Err(msg.into()),
        }
    }
}

#[async_trait]
impl Tool for FixedTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &'static str {
        "fixed-body test tool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        match &self.body {
            Ok(text) => Ok(vec![ContentBlock::Text(TextBlock {
                text: text.clone(),
                cache_control: None,
            })]),
            Err(msg) => Err(ToolError::execution(std::io::Error::other(msg.clone()))),
        }
    }
}

/// An assistant turn that emits one `tool_use` block for `name`, then stops
/// with `ToolUse`.
fn tool_use_turn(id: &str, name: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "msg_tool".into(),
            model: "mock-model".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::ToolUse {
                id: id.to_owned(),
                name: name.to_owned(),
            },
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::ToolUseInputJson("{}".into()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

/// A turn that ends the run with `EndTurn` and no content.
fn end_turn_events() -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "msg_end".into(),
            model: "mock-model".into(),
        }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

/// Run one tool-call turn against `tool` and return the collected events.
async fn run_one_tool(tool: FixedTool, id: &str) -> Vec<TurnEvent> {
    let name = tool.name.clone();
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(tool_use_turn(id, &name));
    mock.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(tool));

    let agent = Arc::new(
        Agent::builder()
            .provider(mock as Arc<dyn Provider + Send + Sync>)
            .tools(registry)
            .model("mock-model")
            .max_tokens(64)
            .build()
            .expect("build agent"),
    );

    let mut events = Vec::new();
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("go")], CancellationToken::new());
    while let Some(ev) = stream.next().await {
        events.push(ev.expect("event should not error"));
    }
    events
}

fn tool_end(events: &[TurnEvent], id: &str) -> (String, bool, bool) {
    events
        .iter()
        .find_map(|e| match e {
            TurnEvent::ToolCallEnd {
                tool_use_id,
                result_text,
                truncated,
                is_error,
                ..
            } if tool_use_id == id => Some((result_text.clone(), *truncated, *is_error)),
            _ => None,
        })
        .expect("ToolCallEnd for the tool_use_id")
}

#[tokio::test]
async fn small_result_is_full_and_untruncated() {
    let events = run_one_tool(FixedTool::ok("small", "hello world"), "tu-small").await;
    let (result_text, truncated, is_error) = tool_end(&events, "tu-small");
    assert!(!is_error, "success tool should not be an error");
    assert!(!truncated, "small result must not be truncated");
    assert_eq!(result_text, "hello world", "full text preserved");
}

#[tokio::test]
async fn large_result_is_capped_and_flagged() {
    let full = "x".repeat(STREAM_RESULT_TEXT_CAP + 500);
    let events = run_one_tool(FixedTool::ok("big", full.clone()), "tu-big").await;
    let (result_text, truncated, _) = tool_end(&events, "tu-big");
    assert!(truncated, "over-cap result must be flagged truncated");
    assert_eq!(
        result_text.chars().count(),
        STREAM_RESULT_TEXT_CAP,
        "result_text is capped to STREAM_RESULT_TEXT_CAP chars",
    );
    assert!(
        full.starts_with(&result_text),
        "result_text is the head (prefix) of the full result",
    );
}

#[tokio::test]
async fn error_result_carries_error_text() {
    let events = run_one_tool(FixedTool::err("boom", "kaboom happened"), "tu-err").await;
    let (result_text, truncated, is_error) = tool_end(&events, "tu-err");
    assert!(is_error, "errored tool must set is_error");
    assert!(!truncated, "short error text must not be truncated");
    assert!(
        result_text.contains("kaboom"),
        "result_text should carry the error message, got: {result_text:?}",
    );
}
