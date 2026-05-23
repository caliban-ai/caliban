#![allow(missing_docs)]

use caliban_provider::{CompletionRequest, ContentBlock, Provider, StopReason, collect_message};
use caliban_provider_anthropic::{AnthropicProvider, config::DirectConfig};
use secrecy::SecretString;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn stream_simple_round_trip() {
    let server = MockServer::start().await;
    let sse_body = include_str!("fixtures/direct/stream_simple.sse");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "text/event-stream")
                .set_body_raw(sse_body, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let cfg = DirectConfig {
        api_key: SecretString::new("key-xyz".into()),
        base_url: Url::parse(&server.uri()).unwrap(),
        anthropic_version: "2023-06-01".to_string(),
        timeout: std::time::Duration::from_secs(10),
    };
    let provider = AnthropicProvider::direct(cfg).unwrap();
    let req = CompletionRequest::builder("claude-3-5-sonnet-20241022")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();
    let stream = provider.stream(req).await.unwrap();
    let (msg, stop, usage) = collect_message(stream).await.unwrap();
    let text = match &msg.content[0] {
        ContentBlock::Text(t) => &t.text,
        _ => panic!("expected text block"),
    };
    assert_eq!(text, "Hello!");
    assert!(matches!(stop, StopReason::EndTurn));
    assert_eq!(usage.output_tokens, 2);
}
