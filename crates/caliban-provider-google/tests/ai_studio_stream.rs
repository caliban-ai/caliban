#![allow(missing_docs)]

use caliban_provider::{CompletionRequest, ContentBlock, Provider, StopReason, collect_message};
use caliban_provider_google::{GoogleProvider, config::AIStudioConfig};
use secrecy::SecretString;
use url::Url;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn stream_simple_round_trip() {
    let server = MockServer::start().await;
    let sse_body = include_str!("fixtures/ai_studio/stream_simple.sse");

    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-2.0-flash:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .and(query_param("key", "key-xyz"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "text/event-stream")
                .set_body_raw(sse_body, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let cfg = AIStudioConfig {
        api_key: SecretString::new("key-xyz".into()),
        base_url: Url::parse(&server.uri()).unwrap(),
        api_version: "v1beta".to_string(),
        timeout: std::time::Duration::from_secs(10),
    };
    let provider = GoogleProvider::ai_studio(cfg).unwrap();
    let req = CompletionRequest::builder("gemini-2.0-flash")
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
