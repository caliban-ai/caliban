//! Fixture tests for [`AzureTransport`].
#![cfg(feature = "azure")]
#![allow(clippy::unreadable_literal)]

use std::collections::HashMap;
use std::time::Duration;

use caliban_provider::{CompletionRequest, Provider, StopReason};
use caliban_provider_openai::OpenAIProvider;
use caliban_provider_openai::config::AzureConfig;
use secrecy::SecretString;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn azure_complete_simple() {
    let server = MockServer::start().await;

    let resp_body = serde_json::json!({
        "id": "chatcmpl-XYZ",
        "object": "chat.completion",
        "created": 1700000000,
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "Hello!"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15}
    });

    Mock::given(method("POST"))
        .and(path("/openai/deployments/my-gpt-4o/chat/completions"))
        .and(query_param("api-version", "2024-10-21"))
        .and(header("api-key", "azure-key-xyz"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&resp_body))
        .mount(&server)
        .await;

    let mut deployments = HashMap::new();
    deployments.insert("gpt-4o".into(), "my-gpt-4o".into());

    let cfg = AzureConfig {
        api_key: SecretString::new("azure-key-xyz".into()),
        resource: String::new(), // unused when base_url is set
        api_version: "2024-10-21".into(),
        timeout: Duration::from_secs(10),
        deployments,
        base_url: Some(url::Url::parse(&server.uri()).unwrap()),
    };

    let provider = OpenAIProvider::azure(cfg).unwrap();

    let req = CompletionRequest::builder("gpt-4o")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();

    let resp = provider.complete(req).await.unwrap();
    assert_eq!(resp.id, "chatcmpl-XYZ");
    assert!(
        matches!(resp.stop_reason, StopReason::EndTurn),
        "expected EndTurn, got {:?}",
        resp.stop_reason
    );
    assert_eq!(resp.usage.input_tokens, 12);
    assert_eq!(resp.usage.output_tokens, 3);
}

#[tokio::test]
async fn azure_missing_deployment_returns_error() {
    let cfg = AzureConfig {
        api_key: SecretString::new("some-key".into()),
        resource: "my-resource".into(),
        api_version: "2024-10-21".into(),
        timeout: Duration::from_secs(10),
        deployments: HashMap::new(), // no mappings
        base_url: None,
    };

    let provider = OpenAIProvider::azure(cfg).unwrap();

    let req = CompletionRequest::builder("gpt-4o")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();

    let err = provider.complete(req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("gpt-4o") || msg.contains("deployment") || msg.contains("missing"),
        "unexpected error message: {msg}"
    );
}

#[tokio::test]
async fn azure_with_deployment_fluent_builder() {
    // Verify the fluent builder chains correctly and sets mappings.
    let cfg = AzureConfig {
        api_key: SecretString::new("key".into()),
        resource: "res".into(),
        api_version: "2024-10-21".into(),
        timeout: Duration::from_secs(10),
        deployments: HashMap::new(),
        base_url: None,
    }
    .with_deployment("gpt-4o", "my-gpt-4o-deploy")
    .with_deployment("gpt-4o-mini", "my-mini-deploy");

    assert_eq!(
        cfg.deployments.get("gpt-4o").map(String::as_str),
        Some("my-gpt-4o-deploy")
    );
    assert_eq!(
        cfg.deployments.get("gpt-4o-mini").map(String::as_str),
        Some("my-mini-deploy")
    );
}
