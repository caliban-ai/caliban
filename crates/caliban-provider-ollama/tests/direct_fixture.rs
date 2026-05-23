#![allow(missing_docs)]

use caliban_provider::{CompletionRequest, Provider, StopReason};
use caliban_provider_ollama::{OllamaProvider, config::DirectConfig};
use url::Url;
use wiremock::matchers::{body_json, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn complete_simple_round_trip() {
    let server = MockServer::start().await;
    let req_json: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/direct/complete_simple_request.json")).unwrap();
    let resp_json: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/direct/complete_simple_response.json"
    ))
    .unwrap();

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(header_exists("content-type"))
        .and(body_json(&req_json))
        .respond_with(ResponseTemplate::new(200).set_body_json(&resp_json))
        .mount(&server)
        .await;

    let cfg = DirectConfig {
        base_url: Url::parse(&server.uri()).unwrap(),
        timeout: std::time::Duration::from_secs(10),
    };
    let provider = OllamaProvider::direct(cfg).unwrap();
    let req = CompletionRequest::builder("llama3.1")
        .system("Be brief.")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();
    let resp = provider.complete(req).await.unwrap();
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
    assert_eq!(resp.usage.input_tokens, 12);
    assert_eq!(resp.usage.output_tokens, 2);
    assert_eq!(resp.model, "llama3.1");
    let text = match &resp.message.content[0] {
        caliban_provider::ContentBlock::Text(t) => &t.text,
        _ => panic!("expected text block"),
    };
    assert_eq!(text, "Hello!");
}
