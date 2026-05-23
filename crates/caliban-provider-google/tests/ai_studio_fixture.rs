#![allow(missing_docs)]

use caliban_provider::{CompletionRequest, Provider, StopReason};
use caliban_provider_google::{GoogleProvider, config::AIStudioConfig};
use secrecy::SecretString;
use url::Url;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn complete_simple_round_trip() {
    let server = MockServer::start().await;
    let req_json: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/ai_studio/complete_simple_request.json"
    ))
    .unwrap();
    let resp_json: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/ai_studio/complete_simple_response.json"
    ))
    .unwrap();

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.0-flash:generateContent"))
        .and(query_param("key", "key-xyz"))
        .and(body_json(&req_json))
        .respond_with(ResponseTemplate::new(200).set_body_json(&resp_json))
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
        .system("Be brief.")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();
    let resp = provider.complete(req).await.unwrap();
    assert_eq!(resp.model, "gemini-2.0-flash-001");
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
    assert_eq!(resp.usage.input_tokens, 12);
    assert_eq!(resp.usage.output_tokens, 3);
}
