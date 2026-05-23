#![allow(missing_docs)]

use caliban_provider::{CompletionRequest, Provider, StopReason};
use caliban_provider_anthropic::{AnthropicProvider, config::DirectConfig};
use secrecy::SecretString;
use url::Url;
use wiremock::matchers::{body_json, header, header_exists, method, path};
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
        .and(path("/v1/messages"))
        .and(header("x-api-key", "key-xyz"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header_exists("content-type"))
        .and(body_json(&req_json))
        .respond_with(ResponseTemplate::new(200).set_body_json(&resp_json))
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
        .system("Be brief.")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();
    let resp = provider.complete(req).await.unwrap();
    assert_eq!(resp.id, "msg_01ABC");
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
    assert_eq!(resp.usage.input_tokens, 12);
}
