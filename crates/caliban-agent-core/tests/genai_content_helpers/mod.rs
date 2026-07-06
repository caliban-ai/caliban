//! Shared harness for the #380 `gen_ai.input.messages` / `gen_ai.output.messages`
//! content tests.
//!
//! The content gate is a process-global `LazyLock` over `OTEL_LOG_USER_PROMPTS`,
//! read once on first use. To exercise both the gated-on and gated-off paths
//! without a caching footgun, the two behaviours live in **separate integration
//! test binaries** (`genai_content_on.rs` / `genai_content_off.rs`) — each is its
//! own process, so each reads the env in its intended state exactly once. This
//! module holds the OTLP-exporter + `MockProvider` scaffolding both share.

#![allow(dead_code)]

use std::sync::Arc;

use caliban_agent_core::Agent;
use caliban_provider::{
    Message, MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta,
    Usage,
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

/// Read a span attribute by key.
pub(crate) fn attr<'a>(span: &'a SpanData, key: &str) -> Option<&'a Value> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| &kv.value)
}

/// Drive one model turn (`user` prompt → `assistant` reply) through a
/// `MockProvider` with an in-memory OTLP exporter installed, and return the
/// exported `chat` generation span.
///
/// Hermetic — no network. The subscriber guard is held only for the duration of
/// the run; the returned `SpanData` is a clone, so callers can inspect it after
/// the exporter/subscriber are gone.
pub(crate) async fn run_and_capture_chat_span(user: &str, assistant: &str) -> SpanData {
    let exporter = InMemorySpanExporterBuilder::new().build();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("caliban-test");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let subscriber = Registry::default().with(otel_layer);
    // Held on the current (single) test thread; the current-thread tokio runtime
    // polls the stream on this same thread, so the subscriber stays active for
    // the whole run.
    let _guard = tracing::subscriber::set_default(subscriber);

    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(text_stream_events(
        "msg1",
        "resp-model",
        assistant,
        StopReason::EndTurn,
    ));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider_arc(Arc::clone(&mock)))
            .model("req-model")
            .max_tokens(1024)
            .build()
            .expect("build agent"),
    );

    let mut stream =
        agent.stream_until_done(vec![Message::user_text(user)], CancellationToken::new());
    while let Some(ev) = stream.next().await {
        ev.expect("event should not error");
    }
    drop(stream);

    let spans = exporter.get_finished_spans().expect("in-memory spans");
    spans
        .iter()
        .find(|s| s.name.starts_with("chat"))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "expected a `chat` generation span, got: {:?}",
                spans.iter().map(|s| &s.name).collect::<Vec<_>>()
            )
        })
}
