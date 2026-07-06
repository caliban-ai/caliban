//! #385: the run span must be the parent of the per-request `chat` spans and
//! the per-call `execute_tool` spans.
//!
//! `stream_until_done_with_settings` builds a `try_stream!` whose body runs
//! while the stream is *polled*, not while it is *constructed*. Making the run
//! span current only during construction (the old `#[instrument]` on a sync
//! fn) left the child spans mis-parented. This test drives one tool-call turn
//! plus a terminating turn through a `MockProvider` with an in-memory OTLP
//! exporter installed, then asserts every `chat` span and the `execute_tool`
//! span nest under the run span (`stream_until_done_with_settings`). Hermetic —
//! no network. Uses a current-thread runtime so the thread-local subscriber
//! set below stays active where the child spans are created.

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
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::export::trace::SpanData;
use opentelemetry_sdk::testing::trace::InMemorySpanExporterBuilder;
use opentelemetry_sdk::trace::TracerProvider;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt as _;

/// A trivial tool that returns a fixed text block.
struct EchoTool {
    name: String,
    schema: serde_json::Value,
}

impl EchoTool {
    fn new() -> Self {
        Self {
            name: "echo".to_owned(),
            schema: serde_json::json!({"type": "object"}),
        }
    }
}

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        &self.name
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

/// The run span (name = the fn name, per #385) parents every `chat` span and
/// the `execute_tool` span, so the exported trace tree nests correctly.
#[tokio::test]
async fn child_spans_nest_under_run_span() {
    let exporter = InMemorySpanExporterBuilder::new().build();
    let otel_provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = otel_provider.tracer("caliban-test");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let subscriber = Registry::default().with(otel_layer);
    let _guard = tracing::subscriber::set_default(subscriber);

    let mock = Arc::new(MockProvider::new());
    // Turn 1: model asks for a tool. Turn 2: model ends the run.
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
    // Drop closes the run span (owned by the SpanStream wrapper) so it exports.
    drop(stream);

    let spans = exporter.get_finished_spans().expect("in-memory spans");
    let names: Vec<&str> = spans.iter().map(|s| s.name.as_ref()).collect();

    let find = |pred: &dyn Fn(&SpanData) -> bool, what: &str| -> SpanData {
        spans
            .iter()
            .find(|s| pred(s))
            .unwrap_or_else(|| panic!("expected a `{what}` span, got: {names:?}"))
            .clone()
    };

    let run = find(
        &|s| s.name.as_ref() == "stream_until_done_with_settings",
        "stream_until_done_with_settings",
    );
    let run_id = run.span_context.span_id();

    // Every `chat` generation span (one per model request; there are two here)
    // must be a direct child of the run span.
    let chat_spans: Vec<&SpanData> = spans
        .iter()
        .filter(|s| s.name.starts_with("chat"))
        .collect();
    assert!(
        !chat_spans.is_empty(),
        "expected at least one `chat` span, got: {names:?}",
    );
    for chat in &chat_spans {
        assert_eq!(
            chat.parent_span_id, run_id,
            "chat span {:?} should nest under the run span (parent {:?}, got {:?})",
            chat.name, run_id, chat.parent_span_id,
        );
    }

    // The `execute_tool` span is a sibling of the chat spans, also under run.
    let tool = find(&|s| s.name.starts_with("execute_tool"), "execute_tool");
    assert_eq!(
        tool.parent_span_id, run_id,
        "execute_tool span should nest under the run span (parent {:?}, got {:?})",
        run_id, tool.parent_span_id,
    );
}
