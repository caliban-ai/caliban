#![allow(missing_docs)]

use caliban_provider::{CompletionRequest, Provider, StopReason};
use caliban_provider_anthropic::{AnthropicProvider, config::DirectConfig};
use secrecy::SecretString;
use url::Url;
use wiremock::matchers::{body_json, header, method, path};
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
        .and(header("content-type", "application/json"))
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

/// Anthropic reports `usage.input_tokens` as the uncached portion only,
/// with cached tokens reported separately under `cache_creation_input_tokens`
/// and `cache_read_input_tokens`. The IR converter normalizes to the
/// `OpenAI` convention where `input_tokens` is the **total prompt size
/// including any cached portion**. This test pins that behavior so the
/// TUI's usage display is consistent across providers.
#[tokio::test]
async fn input_tokens_normalized_to_include_cached_portions() {
    let server = MockServer::start().await;

    let resp_json = serde_json::json!({
        "id": "msg_cache",
        "model": "claude-3-5-sonnet-20241022",
        "role": "assistant",
        "type": "message",
        "content": [{"type": "text", "text": "ok"}],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": {
            "input_tokens": 50,
            "output_tokens": 5,
            "cache_creation_input_tokens": 100,
            "cache_read_input_tokens": 1000
        }
    });

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&resp_json))
        .mount(&server)
        .await;

    let cfg = DirectConfig {
        api_key: SecretString::new("k".into()),
        base_url: Url::parse(&server.uri()).unwrap(),
        anthropic_version: "2023-06-01".to_string(),
        timeout: std::time::Duration::from_secs(5),
    };
    let provider = AnthropicProvider::direct(cfg).unwrap();
    let req = CompletionRequest::builder("claude-3-5-sonnet-20241022")
        .system("sys")
        .user_text("hi")
        .max_tokens(64)
        .build()
        .unwrap();
    let resp = provider.complete(req).await.unwrap();

    // Normalized: 50 (uncached) + 100 (creation) + 1000 (read) = 1150.
    assert_eq!(
        resp.usage.input_tokens, 1150,
        "input_tokens should be normalized to total (50 + 100 + 1000)"
    );
    assert_eq!(resp.usage.cache_creation_input_tokens, Some(100));
    assert_eq!(resp.usage.cache_read_input_tokens, Some(1000));
}
