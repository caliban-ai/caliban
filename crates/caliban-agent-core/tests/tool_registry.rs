#![allow(missing_docs)]

use std::sync::Arc;

use async_trait::async_trait;
use caliban_agent_core::{ContentBlock, Tool, ToolContext, ToolError, ToolRegistry};
use serde_json::json;

struct EchoTool {
    schema: serde_json::Value,
}

impl EchoTool {
    fn new() -> Self {
        Self {
            schema: json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
        }
    }
}

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }
    fn description(&self) -> &'static str {
        "echo input back"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        input: serde_json::Value,
        _cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'text'"))?
            .to_string();
        Ok(vec![ContentBlock::Text(caliban_agent_core::TextBlock {
            text,
            cache_control: None,
        })])
    }
}

#[test]
fn register_and_lookup() {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(EchoTool::new()));
    assert!(reg.get("echo").is_some());
    assert!(reg.get("nope").is_none());
    assert_eq!(reg.names().collect::<Vec<_>>(), vec!["echo"]);
}

#[test]
fn duplicate_register_replaces() {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(EchoTool::new()));
    reg.register(Arc::new(EchoTool::new()));
    assert_eq!(reg.names().count(), 1);
}

#[test]
fn unregister_removes() {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(EchoTool::new()));
    assert!(reg.unregister("echo").is_some());
    assert!(reg.get("echo").is_none());
}

#[test]
fn to_caliban_tools_snapshot() {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(EchoTool::new()));
    let tools = reg.to_caliban_tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
}

#[tokio::test]
async fn invoke_returns_text_block() {
    let tool = EchoTool::new();
    let cx = ToolContext {
        tool_use_id: "toolu_1".into(),
        cancel: tokio_util::sync::CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    };
    let out = tool.invoke(json!({"text": "hi"}), cx).await.unwrap();
    assert_eq!(out.len(), 1);
}

#[tokio::test]
async fn invoke_invalid_input_errors() {
    let tool = EchoTool::new();
    let cx = ToolContext {
        tool_use_id: "toolu_1".into(),
        cancel: tokio_util::sync::CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    };
    let err = tool.invoke(json!({}), cx).await.unwrap_err();
    assert!(matches!(err, ToolError::InvalidInput(_)));
}
