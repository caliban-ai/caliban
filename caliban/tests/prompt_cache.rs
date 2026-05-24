//! Integration test: confirm `prompt_cache=true` marks the outgoing Anthropic
//! wire JSON with `cache_control: {"type": "ephemeral"}` on the last system
//! text block and the last tool definition. Confirm `prompt_cache=false`
//! omits the markers entirely.

use std::sync::Arc;

use caliban_agent_core::{Agent, ToolContext, ToolError, ToolRegistry};
use caliban_provider::Provider;
use caliban_provider_anthropic::{AnthropicProvider, config::DirectConfig};
use secrecy::SecretString;
use serde_json::{Value, json};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Minimal tool stub. We register two of them so the test can prove the
/// LAST one is marked and the earlier one is not.
struct DummyTool {
    name: String,
    schema: serde_json::Value,
}

impl DummyTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            schema: json!({"type": "object"}),
        }
    }
}

#[async_trait::async_trait]
impl caliban_agent_core::Tool for DummyTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &'static str {
        "dummy"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<Vec<caliban_provider::ContentBlock>, ToolError> {
        Ok(Vec::new())
    }
}

/// Build a `DirectConfig` pointing at a mock server.
fn cfg_for(server: &MockServer) -> DirectConfig {
    DirectConfig {
        api_key: SecretString::new("test-key".to_string().into()),
        base_url: Url::parse(&server.uri()).expect("server URI parses"),
        anthropic_version: "2023-06-01".into(),
        timeout: std::time::Duration::from_secs(5),
    }
}

/// Standard "ok" Anthropic response — single text block, `end_turn`.
fn ok_response() -> Value {
    json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "model": "claude-test",
        "content": [{"type": "text", "text": "ok"}],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": {"input_tokens": 5, "output_tokens": 1}
    })
}

#[tokio::test]
async fn prompt_cache_on_emits_cache_control_in_wire_json() {
    let server = MockServer::start().await;

    let captured: Arc<std::sync::Mutex<Option<Value>>> = Arc::new(std::sync::Mutex::new(None));
    let captured_clone = Arc::clone(&captured);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(move |req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).expect("body is JSON");
            *captured_clone.lock().expect("mutex poisoned") = Some(body);
            ResponseTemplate::new(200).set_body_json(ok_response())
        })
        .mount(&server)
        .await;

    let provider: Arc<dyn Provider + Send + Sync> =
        Arc::new(AnthropicProvider::direct(cfg_for(&server)).expect("provider"));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(DummyTool::new("a")));
    registry.register(Arc::new(DummyTool::new("b")));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider)
            .tools(registry)
            .model("claude-test")
            .max_tokens(64)
            .prompt_cache(true)
            .build()
            .expect("agent"),
    );

    let mut session = caliban_agent_core::Session::new(agent);
    session.system("system prompt").user_text("hi");
    let _ = session.run().await.expect("run");

    let body = captured
        .lock()
        .expect("mutex poisoned")
        .clone()
        .expect("captured body");

    // System message: with cache_control on a block, ir_convert serializes
    // system as an array of blocks (not as a single string).
    let system = &body["system"];
    let blocks = system
        .as_array()
        .unwrap_or_else(|| panic!("expected array-form system, got: {system}"));
    let last_block = blocks.last().expect("blocks empty");
    assert_eq!(
        last_block["cache_control"]["type"], "ephemeral",
        "last system block should be marked, got: {last_block}"
    );

    // Tools: last has cache_control, earlier do not.
    let tools = body["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 2, "expected 2 tools, got: {tools:?}");
    let last_tool = tools.last().unwrap();
    assert_eq!(
        last_tool["cache_control"]["type"], "ephemeral",
        "last tool should be marked, got: {last_tool}"
    );
    assert!(
        tools[0].get("cache_control").is_none(),
        "earlier tool should not be marked, got: {}",
        tools[0]
    );
}

#[tokio::test]
async fn prompt_cache_off_omits_cache_control() {
    let server = MockServer::start().await;

    let captured: Arc<std::sync::Mutex<Option<Value>>> = Arc::new(std::sync::Mutex::new(None));
    let captured_clone = Arc::clone(&captured);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(move |req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).expect("body is JSON");
            *captured_clone.lock().expect("mutex poisoned") = Some(body);
            ResponseTemplate::new(200).set_body_json(ok_response())
        })
        .mount(&server)
        .await;

    let provider: Arc<dyn Provider + Send + Sync> =
        Arc::new(AnthropicProvider::direct(cfg_for(&server)).expect("provider"));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(DummyTool::new("a")));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider)
            .tools(registry)
            .model("claude-test")
            .max_tokens(64)
            .prompt_cache(false)
            .build()
            .expect("agent"),
    );

    let mut session = caliban_agent_core::Session::new(agent);
    session.system("system prompt").user_text("hi");
    let _ = session.run().await.expect("run");

    let body = captured
        .lock()
        .expect("mutex poisoned")
        .clone()
        .expect("captured body");
    let serialized = body.to_string();
    assert!(
        !serialized.contains("cache_control"),
        "no cache_control expected when prompt_cache=false; got: {serialized}"
    );
}
