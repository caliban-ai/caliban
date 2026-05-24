//! Verify that token usage counters (`input_tokens`, `output_tokens`,
//! `cache_creation_input_tokens`, `cache_read_input_tokens`) aggregate
//! correctly across multiple turns via `Usage::merge`.

use std::sync::Arc;

use async_trait::async_trait;
use caliban_agent_core::{Agent, ToolContext, ToolError, ToolRegistry};
use caliban_provider::{
    ContentBlock, Message, MockProvider, Provider, StopReason, StreamEvent, StreamingContentType,
    StreamingDelta, Usage,
};
use tokio_util::sync::CancellationToken;

/// A tool that returns an empty result block.
struct NoOpTool {
    name: String,
    schema: serde_json::Value,
}

impl NoOpTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            schema: serde_json::json!({"type": "object"}),
        }
    }
}

#[async_trait]
impl caliban_agent_core::Tool for NoOpTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &'static str {
        "noop"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        Ok(Vec::new())
    }
}

/// Stream events for one turn that calls a tool, with explicit Usage.
fn tool_turn_events(
    msg_id: &str,
    tool_use_id: &str,
    usage: Usage,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.into(),
            model: "mock-model".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::ToolUse {
                id: tool_use_id.into(),
                name: "noop".into(),
            },
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::ToolUseInputJson("{}".into()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            usage_delta: Some(usage),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

/// Stream events for one terminal turn (text response), with explicit Usage.
fn text_turn_events(
    msg_id: &str,
    text: &str,
    usage: Usage,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.into(),
            model: "mock-model".into(),
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
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(usage),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

/// All four token counters must sum across two turns.
///
/// Turn 1 usage: `input=10, output=3, cache_creation=5`
/// Turn 2 usage: `input=7,  output=4, cache_read=5`
/// Expected total: `input=17, output=7, cache_creation=5, cache_read=5`.
#[tokio::test]
async fn token_counters_aggregate_across_turns() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(tool_turn_events(
        "m1",
        "tu_1",
        Usage {
            input_tokens: 10,
            output_tokens: 3,
            cache_creation_input_tokens: Some(5),
            cache_read_input_tokens: None,
        },
    ));
    mock.enqueue_stream(text_turn_events(
        "m2",
        "done",
        Usage {
            input_tokens: 7,
            output_tokens: 4,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: Some(5),
        },
    ));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(NoOpTool::new("noop")));

    let agent = Arc::new(
        Agent::builder()
            .provider(mock as Arc<dyn Provider + Send + Sync>)
            .tools(registry)
            .model("mock-model")
            .max_tokens(1024)
            .build()
            .expect("agent"),
    );

    let outcome = Arc::clone(&agent)
        .run_until_done(vec![Message::user_text("go")], CancellationToken::new())
        .await
        .expect("run");

    assert_eq!(outcome.total_usage.input_tokens, 17);
    assert_eq!(outcome.total_usage.output_tokens, 7);
    assert_eq!(outcome.total_usage.cache_creation_input_tokens, Some(5));
    assert_eq!(outcome.total_usage.cache_read_input_tokens, Some(5));
}

/// When one turn provides cache fields and another doesn't, the merge keeps
/// the populated value rather than dropping to None.
#[tokio::test]
async fn cache_fields_survive_mixed_turns() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(tool_turn_events(
        "m1",
        "tu_1",
        Usage {
            input_tokens: 5,
            output_tokens: 1,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
    ));
    mock.enqueue_stream(text_turn_events(
        "m2",
        "done",
        Usage {
            input_tokens: 3,
            output_tokens: 2,
            cache_creation_input_tokens: Some(42),
            cache_read_input_tokens: None,
        },
    ));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(NoOpTool::new("noop")));

    let agent = Arc::new(
        Agent::builder()
            .provider(mock as Arc<dyn Provider + Send + Sync>)
            .tools(registry)
            .model("mock-model")
            .max_tokens(1024)
            .build()
            .expect("agent"),
    );

    let outcome = Arc::clone(&agent)
        .run_until_done(vec![Message::user_text("go")], CancellationToken::new())
        .await
        .expect("run");

    assert_eq!(outcome.total_usage.input_tokens, 8);
    assert_eq!(outcome.total_usage.output_tokens, 3);
    assert_eq!(
        outcome.total_usage.cache_creation_input_tokens,
        Some(42),
        "cache_creation present in turn 2 should survive merge with turn 1's None"
    );
    assert_eq!(outcome.total_usage.cache_read_input_tokens, None);
}
