#![allow(missing_docs)]
//! Regression tests for three issues in the Ollama adapter:
//!
//! 1. `done_reason: "stop"` with a populated `tool_calls` array must be
//!    surfaced to the agent as `StopReason::ToolUse` so the agent loop
//!    continues. Ollama uses `"stop"` even on tool-calling turns.
//! 2. The `message.thinking` field (used by reasoning models such as
//!    qwen3.5) must be captured into a `Thinking` content block instead
//!    of being silently dropped.
//! 3. Upstream `tool_call.id` (e.g. `"call_xoh1i8k9"`) must be preserved
//!    on the `ToolUseBlock` rather than being replaced with a generated
//!    `tool_{idx}`.

use caliban_provider::{
    CompletionRequest, ContentBlock, Provider, StopReason, StreamEvent, StreamingContentType,
    StreamingDelta, collect_message,
};
use caliban_provider_ollama::transport::direct::DirectTransport;
use caliban_provider_ollama::{OllamaProvider, config::DirectConfig};
use futures::StreamExt;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn provider_for(server: &MockServer) -> OllamaProvider<DirectTransport> {
    let cfg = DirectConfig {
        base_url: Url::parse(&server.uri()).unwrap(),
        timeout: std::time::Duration::from_secs(10),
    };
    OllamaProvider::direct(cfg).unwrap()
}

fn build_request() -> CompletionRequest {
    CompletionRequest::builder("qwen3.5:9b")
        .user_text("go")
        .max_tokens(64)
        .build()
        .unwrap()
}

#[tokio::test]
async fn complete_tool_call_with_done_reason_stop_maps_to_tool_use() {
    // Ollama returns `done_reason: "stop"` on tool-calling turns (verified
    // against a live instance serving qwen3.5:9b). The IR must surface
    // `StopReason::ToolUse` whenever tool_calls is non-empty, regardless of
    // the textual done_reason, so the agent loop continues.
    let server = MockServer::start().await;
    let body: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/direct/complete_tool_call_response.json"
    ))
    .unwrap();

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;

    let resp = provider_for(&server)
        .complete(build_request())
        .await
        .unwrap();

    assert!(
        matches!(resp.stop_reason, StopReason::ToolUse),
        "expected ToolUse, got {:?}",
        resp.stop_reason
    );

    // The upstream tool_call.id must be preserved on the IR block.
    let tool_use = resp
        .message
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::ToolUse(t) => Some(t),
            _ => None,
        })
        .expect("expected ToolUse block");
    assert_eq!(tool_use.id, "call_xoh1i8k9");
    assert_eq!(tool_use.name, "Read");
}

#[tokio::test]
async fn complete_thinking_field_produces_thinking_block() {
    // Reasoning models (qwen3.5 via Ollama) populate `message.thinking`.
    // The previous schema silently dropped it; we now surface it as a
    // Thinking content block in the IR message.
    let server = MockServer::start().await;
    let body: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/direct/complete_thinking_response.json"
    ))
    .unwrap();

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;

    let resp = provider_for(&server)
        .complete(build_request())
        .await
        .unwrap();

    let thinking = resp
        .message
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::Thinking(t) => Some(t),
            _ => None,
        })
        .expect("expected Thinking block in content");
    assert_eq!(thinking.thinking, "The project is a Rust agent harness.");

    // Final text is still present.
    let text = resp
        .message
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::Text(t) => Some(&t.text),
            _ => None,
        })
        .expect("expected Text block");
    assert_eq!(text, "caliban is a Rust agent harness.");
}

#[tokio::test]
async fn stream_tool_call_with_done_reason_stop_maps_to_tool_use() {
    let server = MockServer::start().await;
    let body = include_str!("fixtures/direct/stream_tool_call_done_stop.ndjson");

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "application/x-ndjson")
                .set_body_raw(body, "application/x-ndjson"),
        )
        .mount(&server)
        .await;

    let provider = provider_for(&server);
    let stream = provider.stream(build_request()).await.unwrap();
    let (msg, stop, _usage) = collect_message(stream).await.unwrap();

    assert!(
        matches!(stop, StopReason::ToolUse),
        "expected ToolUse, got {stop:?}"
    );

    let tool_use = msg
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::ToolUse(t) => Some(t),
            _ => None,
        })
        .expect("expected ToolUse block");
    assert_eq!(tool_use.id, "call_xoh1i8k9");
    assert_eq!(tool_use.name, "Read");
}

#[tokio::test]
async fn stream_thinking_emits_thinking_block_then_text() {
    // The streaming parser must emit a Thinking ContentBlockStart + Delta(s)
    // when chunks carry `message.thinking`, then close it and open a Text
    // block once `message.content` deltas start arriving.
    let server = MockServer::start().await;
    let body = include_str!("fixtures/direct/stream_thinking_then_text.ndjson");

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "application/x-ndjson")
                .set_body_raw(body, "application/x-ndjson"),
        )
        .mount(&server)
        .await;

    let provider = provider_for(&server);
    let mut stream = provider.stream(build_request()).await.unwrap();

    let mut saw_thinking_start = false;
    let mut saw_thinking_delta = false;
    let mut saw_thinking_stop = false;
    let mut saw_text_start = false;
    let mut saw_text_delta = false;
    let mut thinking_index: Option<u32> = None;
    let mut text_index: Option<u32> = None;

    while let Some(evt) = stream.next().await {
        match evt.unwrap() {
            StreamEvent::ContentBlockStart {
                index,
                content_type: StreamingContentType::Thinking,
            } => {
                saw_thinking_start = true;
                thinking_index = Some(index);
            }
            StreamEvent::ContentBlockStart {
                index,
                content_type: StreamingContentType::Text,
            } => {
                saw_text_start = true;
                text_index = Some(index);
                // Thinking must close before text opens.
                assert!(
                    saw_thinking_stop,
                    "text block opened before thinking closed"
                );
            }
            StreamEvent::Delta {
                index,
                delta: StreamingDelta::Thinking(_),
            } => {
                saw_thinking_delta = true;
                assert_eq!(Some(index), thinking_index);
            }
            StreamEvent::Delta {
                index,
                delta: StreamingDelta::Text(_),
            } => {
                saw_text_delta = true;
                assert_eq!(Some(index), text_index);
            }
            StreamEvent::ContentBlockStop { index } if Some(index) == thinking_index => {
                saw_thinking_stop = true;
            }
            _ => {}
        }
    }

    assert!(saw_thinking_start, "no Thinking ContentBlockStart emitted");
    assert!(saw_thinking_delta, "no Thinking Delta emitted");
    assert!(saw_thinking_stop, "no Thinking ContentBlockStop emitted");
    assert!(saw_text_start, "no Text ContentBlockStart emitted");
    assert!(saw_text_delta, "no Text Delta emitted");
}
