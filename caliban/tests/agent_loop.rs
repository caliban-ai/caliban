#![allow(missing_docs)]

use std::sync::Arc;

use caliban_agent_core::{Agent, Message, ToolRegistry};
use caliban_provider::{
    ContentBlock, MockProvider, Provider, StopReason, StreamEvent, StreamingContentType,
    StreamingDelta, Usage,
};
use caliban_tools_builtin::{ReadTool, WorkspaceRoot};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn end_to_end_read_summarize() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("README.md");
    std::fs::write(&path, "# Caliban\n\nA Rust agent harness.\n").unwrap();

    let mock = Arc::new(MockProvider::new());

    // Turn 1: assistant emits a Read tool_use
    mock.enqueue_stream(vec![
        Ok(StreamEvent::MessageStart {
            id: "msg_1".into(),
            model: "mock".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::ToolUse {
                id: "tool_1".into(),
                name: "Read".into(),
            },
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::ToolUseInputJson(r#"{"path":"README.md"}"#.into()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            usage_delta: Some(Usage {
                input_tokens: 30,
                output_tokens: 10,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]);

    // Turn 2: assistant emits a text summary + EndTurn
    mock.enqueue_stream(vec![
        Ok(StreamEvent::MessageStart {
            id: "msg_2".into(),
            model: "mock".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text("It's a Rust agent harness.".into()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage {
                input_tokens: 50,
                output_tokens: 8,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]);

    let mut registry = ToolRegistry::new();
    let root = WorkspaceRoot::new(tmp.path());
    registry.register(Arc::new(ReadTool::new(root)));

    let provider: Arc<dyn Provider + Send + Sync> = mock;
    let agent = Arc::new(
        Agent::builder()
            .provider(provider)
            .tools(registry)
            .model("mock")
            .max_tokens(256)
            .build()
            .unwrap(),
    );

    let outcome = Arc::clone(&agent)
        .run_until_done(
            vec![Message::user_text("Read README.md")],
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let last = outcome.final_messages.last().unwrap();
    let text = match &last.content[0] {
        ContentBlock::Text(t) => &t.text,
        _ => panic!("expected text"),
    };
    assert!(
        text.contains("Rust"),
        "summary should mention Rust; got: {text}"
    );
}
