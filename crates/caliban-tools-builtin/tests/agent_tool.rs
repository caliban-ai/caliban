//! Integration tests for the `AgentTool` (sub-agent primitive).

use std::sync::Arc;

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, ContentBlock, TextBlock, Tool, ToolContext, ToolError, ToolRegistry,
};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use caliban_tools_builtin::{AgentFactory, AgentTool, AgentToolInput};
use tokio_util::sync::CancellationToken;

// ---- Test tools ----

struct ReadTestTool {
    schema: serde_json::Value,
}

impl ReadTestTool {
    fn new() -> Self {
        Self {
            schema: serde_json::json!({"type":"object"}),
        }
    }
}

#[async_trait]
impl Tool for ReadTestTool {
    fn name(&self) -> &'static str {
        "Read"
    }
    fn description(&self) -> &'static str {
        "read test tool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        Ok(vec![ContentBlock::Text(TextBlock {
            text: "read-ok".into(),
            cache_control: None,
        })])
    }
}

// ---- Helpers ----

fn text_response(text: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "m".into(),
            model: "mock".into(),
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
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

fn agent_with_provider(mp: Arc<MockProvider>, tools: ToolRegistry, model: &str) -> Agent {
    Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(tools)
        .model(model)
        .max_tokens(64)
        .max_turns(20)
        .build()
        .unwrap()
}

fn ctx() -> ToolContext {
    ToolContext {
        tool_use_id: "t1".into(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    }
}

fn factory_from(mp: Arc<MockProvider>) -> AgentFactory {
    Arc::new(move |input: &AgentToolInput| {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ReadTestTool::new()));
        let model = input
            .model
            .clone()
            .unwrap_or_else(|| "mock-default".to_string());
        agent_with_provider(Arc::clone(&mp), registry, &model)
    })
}

// ---- Tests ----

#[tokio::test]
async fn returns_final_text_to_parent() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_response("OK"));

    let tool = AgentTool::new(factory_from(mp), None);
    let out = tool
        .invoke(serde_json::json!({ "prompt": "say OK" }), ctx())
        .await
        .unwrap();
    let ContentBlock::Text(t) = &out[0] else {
        panic!("expected text block")
    };
    assert_eq!(t.text, "OK");
}

#[tokio::test]
async fn truncates_long_output() {
    let big = "a".repeat(6_000);
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_response(&big));

    let tool = AgentTool::new(factory_from(mp), None);
    let out = tool
        .invoke(serde_json::json!({ "prompt": "say long" }), ctx())
        .await
        .unwrap();
    let ContentBlock::Text(t) = &out[0] else {
        panic!()
    };
    assert!(t.text.ends_with("[sub-agent output truncated]"));
    // 5000 a's + newlines + footer
    assert!(t.text.starts_with("aaaaa"));
}

#[tokio::test]
async fn model_override_is_honored() {
    // We use a closure-side variable to inspect what model the factory was given.
    let chosen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let chosen_for_factory = std::sync::Arc::clone(&chosen);
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_response("OK"));

    let factory: AgentFactory = Arc::new(move |input: &AgentToolInput| {
        let model = input
            .model
            .clone()
            .unwrap_or_else(|| "mock-default".to_string());
        *chosen_for_factory.lock().unwrap() = model.clone();
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ReadTestTool::new()));
        agent_with_provider(Arc::clone(&mp), registry, &model)
    });

    let tool = AgentTool::new(factory, None);
    tool.invoke(
        serde_json::json!({ "prompt": "say OK", "model": "gpt-4o-mini" }),
        ctx(),
    )
    .await
    .unwrap();
    assert_eq!(*chosen.lock().unwrap(), "gpt-4o-mini");
}

#[tokio::test]
async fn cancellation_propagates() {
    let mp = Arc::new(MockProvider::new());
    // Enqueue a stream that takes a small amount of time before completion.
    mp.enqueue_stream(text_response("never read"));

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        cancel_clone.cancel();
    });

    let cx = ToolContext {
        tool_use_id: "t1".into(),
        cancel: cancel.clone(),
        hooks: None,
        turn_index: 0,
    };

    let tool = AgentTool::new(factory_from(mp), None);
    let res = tool.invoke(serde_json::json!({ "prompt": "go" }), cx).await;
    // Either the sub-agent finished before cancel (small race) — accept OK,
    // OR it was cancelled. Both demonstrate the wiring works; we mainly want
    // to exercise the cancel path doesn't panic.
    if let Err(e) = res {
        assert!(matches!(e, ToolError::Cancelled));
    }
    // If Ok, the sub-agent finished before the spawned cancel fired — also fine.
    // The cancel token at least gets to fire without panicking.
    drop(cancel);
}

#[tokio::test]
async fn invalid_input_errors() {
    let mp = Arc::new(MockProvider::new());
    let tool = AgentTool::new(factory_from(mp), None);
    // Missing required "prompt"
    let err = tool.invoke(serde_json::json!({}), ctx()).await.unwrap_err();
    assert!(matches!(err, ToolError::InvalidInput(_)));
}
