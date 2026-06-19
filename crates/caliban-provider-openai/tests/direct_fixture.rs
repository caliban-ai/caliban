#![allow(missing_docs)]

use caliban_provider::{CompletionRequest, Provider, StopReason};
use caliban_provider_openai::{OpenAIProvider, config::DirectConfig};
use secrecy::SecretString;
use url::Url;
use wiremock::matchers::{body_json, body_partial_json, header, header_exists, method, path};
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
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer key-xyz"))
        .and(header_exists("content-type"))
        .and(body_json(&req_json))
        .respond_with(ResponseTemplate::new(200).set_body_json(&resp_json))
        .mount(&server)
        .await;

    let cfg = DirectConfig {
        api_key: SecretString::new("key-xyz".into()),
        base_url: Url::parse(&format!("{}/v1", server.uri())).unwrap(),
        organization: None,
        project: None,
        timeout: std::time::Duration::from_secs(10),
    };
    let provider = OpenAIProvider::direct(cfg).unwrap();
    let req = CompletionRequest::builder("gpt-4o")
        .system("Be brief.")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();
    let resp = provider.complete(req).await.unwrap();
    assert_eq!(resp.id, "chatcmpl-XYZ");
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
    assert_eq!(resp.usage.input_tokens, 12);
    assert_eq!(resp.usage.output_tokens, 3);
}

#[tokio::test]
async fn o1_uses_developer_role() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(serde_json::json!({
            "messages": [
                {"role": "developer", "content": "Be brief."},
                {"role": "user", "content": "Hi!"}
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-O1",
            "object": "chat.completion",
            "created": 1_700_000_000_u64,
            "model": "o1",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "OK", "refusal": null, "tool_calls": []},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 5,
                "completion_tokens": 1,
                "total_tokens": 6
            }
        })))
        .mount(&server)
        .await;

    let cfg = DirectConfig {
        api_key: SecretString::new("k".into()),
        base_url: Url::parse(&format!("{}/v1", server.uri())).unwrap(),
        organization: None,
        project: None,
        timeout: std::time::Duration::from_secs(10),
    };
    let provider = OpenAIProvider::direct(cfg).unwrap();
    let req = CompletionRequest::builder("o1")
        .system("Be brief.")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();
    let _ = provider.complete(req).await.unwrap();
}

#[tokio::test]
async fn rate_limit_429_captures_retry_after_header() {
    // A 429 carrying a delta-seconds Retry-After must surface on RateLimit so
    // the agent-core retry loop honors the server's requested backoff. This
    // exercises the shared `check_status` path end-to-end: header parse off a
    // real reqwest response → BadStatus → From → RateLimit.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "42"))
        .mount(&server)
        .await;

    let cfg = DirectConfig {
        api_key: SecretString::new("k".into()),
        base_url: Url::parse(&format!("{}/v1", server.uri())).unwrap(),
        organization: None,
        project: None,
        timeout: std::time::Duration::from_secs(10),
    };
    let provider = OpenAIProvider::direct(cfg).unwrap();
    let req = CompletionRequest::builder("gpt-4o")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();

    let err = provider.complete(req).await.unwrap_err();
    assert!(
        matches!(
            err,
            caliban_provider::Error::RateLimit { retry_after: Some(d) }
                if d == std::time::Duration::from_secs(42)
        ),
        "expected RateLimit with retry_after=42s, got {err:?}",
    );
}
