//! #379: OTel GenAI semconv `execute_tool` span per tool call.
//!
//! Drives one tool-call turn through a `MockProvider` + a registered tool with
//! an in-memory OTLP span exporter installed, then asserts the exported span
//! carries the semconv-only `gen_ai.tool.*` attributes (ADR 0053). Hermetic —
//! no network. Uses a current-thread runtime so the thread-local subscriber set
//! below stays active where the `dispatch_tool` span is created.

#![allow(missing_docs)]

use std::sync::Arc;

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, ContentBlock, Message, TextBlock, Tool, ToolContext, ToolError, ToolRegistry,
};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use opentelemetry::Value;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::export::trace::SpanData;
use opentelemetry_sdk::testing::trace::InMemorySpanExporterBuilder;
use opentelemetry_sdk::trace::TracerProvider;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt as _;

/// A trivial tool that returns a fixed text block.
struct EchoTool {
    schema: serde_json::Value,
}

impl EchoTool {
    fn new() -> Self {
        Self {
            schema: serde_json::json!({"type": "object"}),
        }
    }
}

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &'static str {
        "echo test tool"
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
            text: "ok".to_owned(),
            cache_control: None,
        })])
    }
}

/// An assistant turn that emits one `tool_use` block, then stops with `ToolUse`.
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

fn attr<'a>(span: &'a SpanData, key: &str) -> Option<&'a Value> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| &kv.value)
}

/// A tool call emits an `execute_tool …` span carrying the semconv `gen_ai.tool.*`
/// attributes.
#[tokio::test]
async fn tool_call_emits_execute_tool_span() {
    let exporter = InMemorySpanExporterBuilder::new().build();
    let otel_provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = otel_provider.tracer("caliban-test");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let subscriber = Registry::default().with(otel_layer);
    let _guard = tracing::subscriber::set_default(subscriber);

    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(tool_use_turn("call-1", "echo"));
    mock.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(EchoTool::new()));

    let agent = Arc::new(
        Agent::builder()
            .provider(mock as Arc<dyn Provider + Send + Sync>)
            .tools(registry)
            .model("mock-model")
            .max_tokens(64)
            .build()
            .expect("build agent"),
    );

    let mut stream =
        agent.stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    while let Some(ev) = stream.next().await {
        ev.expect("event should not error");
    }
    drop(stream);

    let spans = exporter.get_finished_spans().expect("in-memory spans");
    let tool = spans
        .iter()
        .find(|s| s.name.starts_with("execute_tool"))
        .unwrap_or_else(|| {
            panic!(
                "expected an `execute_tool` span, got: {:?}",
                spans.iter().map(|s| &s.name).collect::<Vec<_>>()
            )
        });

    // otel.name sets the exported span name to "execute_tool {tool}".
    assert_eq!(tool.name.as_ref(), "execute_tool echo");
    assert!(
        matches!(attr(tool, "gen_ai.operation.name"), Some(Value::String(s)) if s.as_str() == "execute_tool"),
        "gen_ai.operation.name should be \"execute_tool\"",
    );
    assert!(
        matches!(attr(tool, "gen_ai.tool.name"), Some(Value::String(s)) if s.as_str() == "echo"),
        "gen_ai.tool.name should be the tool name",
    );
    assert!(
        matches!(attr(tool, "gen_ai.tool.call.id"), Some(Value::String(s)) if s.as_str() == "call-1"),
        "gen_ai.tool.call.id should be the tool-use id",
    );
}
