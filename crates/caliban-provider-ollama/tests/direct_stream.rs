#![allow(missing_docs)]

use caliban_provider::{CompletionRequest, Provider, StopReason, collect_message};
use caliban_provider_ollama::{OllamaProvider, config::DirectConfig};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn stream_simple_round_trip() {
    let server = MockServer::start().await;
    let ndjson_body = include_str!("fixtures/direct/stream_simple.ndjson");

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "application/x-ndjson")
                .set_body_raw(ndjson_body, "application/x-ndjson"),
        )
        .mount(&server)
        .await;

    let cfg = DirectConfig {
        base_url: Url::parse(&server.uri()).unwrap(),
        timeout: std::time::Duration::from_secs(10),
        stream_total_timeout: None,
    };
    let provider = OllamaProvider::direct(cfg).unwrap();
    let req = CompletionRequest::builder("llama3.1")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();
    let stream = provider.stream(req).await.unwrap();
    let (msg, stop, usage) = collect_message(stream).await.unwrap();
    let text = match &msg.content[0] {
        caliban_provider::ContentBlock::Text(t) => &t.text,
        _ => panic!("expected text block"),
    };
    assert_eq!(text, "Hello!");
    assert!(matches!(stop, StopReason::EndTurn));
    assert_eq!(usage.output_tokens, 2);
}

#[tokio::test]
async fn stream_without_done_marker_is_error_not_clean_endturn() {
    // #424: a response cut short (no `done:true`) must surface as an error so
    // the agent loop can retry — not be presented as a clean EndTurn with
    // truncated content.
    let server = MockServer::start().await;
    // One non-final content chunk, then the stream simply ends. No done:true.
    let body = concat!(
        r#"{"model":"llama3.1","created_at":"t","message":{"role":"assistant","content":"partial"},"done":false}"#,
        "\n"
    );
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "application/x-ndjson")
                .set_body_raw(body, "application/x-ndjson"),
        )
        .mount(&server)
        .await;

    let cfg = DirectConfig {
        base_url: Url::parse(&server.uri()).unwrap(),
        timeout: std::time::Duration::from_secs(10),
        stream_total_timeout: None,
    };
    let provider = OllamaProvider::direct(cfg).unwrap();
    let req = CompletionRequest::builder("llama3.1")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();
    let stream = provider.stream(req).await.unwrap();
    let result = collect_message(stream).await;
    assert!(
        result.is_err(),
        "a truncated stream (no done marker) must be an error, got {result:?}"
    );
}
