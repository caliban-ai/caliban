//! #378: per-model-request OpenTelemetry `GenAI` "chat" generation span.
//!
//! Drives one model turn through a `MockProvider` with an in-memory OTLP span
//! exporter installed, then asserts the exported span carries the semconv-only
//! `gen_ai.*` attributes (ADR 0053). Hermetic — no network.

#![allow(missing_docs)]

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

fn attr<'a>(span: &'a SpanData, key: &str) -> Option<&'a Value> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| &kv.value)
}

/// One model turn emits a `chat …` generation span carrying the required
/// semconv-only `gen_ai.*` attributes.
#[tokio::test]
async fn one_turn_emits_gen_ai_chat_span() {
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
    // Requested model ("req-model") differs from the responded model
    // ("resp-model") so request/response attributes are distinguishable.
    mock.enqueue_stream(text_stream_events(
        "msg1",
        "resp-model",
        "hi there",
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
        agent.stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    while let Some(ev) = stream.next().await {
        ev.expect("event should not error");
    }
    drop(stream);

    let spans = exporter.get_finished_spans().expect("in-memory spans");
    let chat = spans
        .iter()
        .find(|s| s.name.starts_with("chat"))
        .unwrap_or_else(|| {
            panic!(
                "expected a `chat` generation span, got: {:?}",
                spans.iter().map(|s| &s.name).collect::<Vec<_>>()
            )
        });

    // otel.name sets the exported span name to "chat {model}".
    assert_eq!(chat.name.as_ref(), "chat req-model");

    // Operation + models + provider.
    assert!(
        matches!(attr(chat, "gen_ai.operation.name"), Some(Value::String(s)) if s.as_str() == "chat"),
        "gen_ai.operation.name should be \"chat\"",
    );
    assert!(
        matches!(attr(chat, "gen_ai.request.model"), Some(Value::String(s)) if s.as_str() == "req-model"),
        "gen_ai.request.model should be the requested model",
    );
    assert!(
        matches!(attr(chat, "gen_ai.response.model"), Some(Value::String(s)) if s.as_str() == "resp-model"),
        "gen_ai.response.model should be the responded model",
    );
    assert!(
        matches!(attr(chat, "gen_ai.provider.name"), Some(Value::String(s)) if s.as_str() == "mock"),
        "gen_ai.provider.name should be the provider name",
    );

    // Finish reason mapped from StopReason::EndTurn.
    assert!(
        matches!(attr(chat, "gen_ai.response.finish_reasons"), Some(Value::String(s)) if s.as_str() == "stop"),
        "gen_ai.response.finish_reasons should map EndTurn -> stop",
    );

    // Token usage (folded input; no cache tokens here so input == 8).
    assert_eq!(
        attr(chat, "gen_ai.usage.input_tokens"),
        Some(&Value::I64(8)),
        "gen_ai.usage.input_tokens",
    );
    assert_eq!(
        attr(chat, "gen_ai.usage.output_tokens"),
        Some(&Value::I64(4)),
        "gen_ai.usage.output_tokens",
    );

    // max_tokens recorded; temperature/top_p unset -> omitted.
    assert_eq!(
        attr(chat, "gen_ai.request.max_tokens"),
        Some(&Value::I64(1024)),
        "gen_ai.request.max_tokens",
    );
    assert!(
        attr(chat, "gen_ai.request.temperature").is_none(),
        "unset temperature should be omitted",
    );
    assert!(
        attr(chat, "gen_ai.request.top_p").is_none(),
        "unset top_p should be omitted",
    );
}
