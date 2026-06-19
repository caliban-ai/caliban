#![allow(missing_docs)]
//! Runtime context-window detection (issue #60).
//!
//! Exercises the real `DirectTransport` against a mocked Ollama server so the
//! `/api/ps` and `/api/show` HTTP parsing and the `OllamaProvider` resolution
//! chain are covered end to end. Fixtures mirror bodies captured from a live
//! server (Ollama 0.30.6): `qwen3.6:27b-mlx` reports a 262144-token window,
//! 8× the 32768 the static table would guess.

use caliban_provider::{CompletionRequest, Provider};
use caliban_provider_ollama::transport::Transport;
use caliban_provider_ollama::transport::direct::DirectTransport;
use caliban_provider_ollama::{OllamaProvider, config::DirectConfig};
use serde_json::json;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "qwen3.6:27b-mlx";
const REAL_CTX: u32 = 262_144;
const STATIC_FALLBACK: u32 = 32_768;

fn transport_for(server: &MockServer) -> DirectTransport {
    DirectTransport::new(DirectConfig {
        base_url: Url::parse(&server.uri()).unwrap(),
        timeout: std::time::Duration::from_secs(10),
    })
    .unwrap()
}

fn provider_for(server: &MockServer) -> OllamaProvider<DirectTransport> {
    OllamaProvider::from_transport(transport_for(server))
}

async fn mount_ps(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/api/ps"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn mount_show(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/api/show"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

fn ps_loaded() -> serde_json::Value {
    json!({ "models": [
        { "name": MODEL, "model": MODEL, "context_length": REAL_CTX }
    ] })
}

fn show_body() -> serde_json::Value {
    json!({ "model_info": {
        "general.architecture": "qwen3_5",
        "qwen3_5.context_length": REAL_CTX
    } })
}

// ---- transport layer ---------------------------------------------------

#[tokio::test]
async fn direct_running_models_parses_ps() {
    let server = MockServer::start().await;
    mount_ps(&server, ps_loaded()).await;

    let running = transport_for(&server).running_models().await.unwrap();
    assert_eq!(running.len(), 1);
    assert!(running[0].matches(MODEL));
    assert_eq!(running[0].context_length, Some(REAL_CTX));
}

#[tokio::test]
async fn direct_show_model_parses_show() {
    let server = MockServer::start().await;
    mount_show(&server, show_body()).await;

    let show = transport_for(&server)
        .show_model(MODEL)
        .await
        .unwrap()
        .expect("show body present");
    assert_eq!(show.context_length(), Some(REAL_CTX));
}

#[tokio::test]
async fn direct_running_models_empty_when_nothing_loaded() {
    let server = MockServer::start().await;
    mount_ps(&server, json!({ "models": [] })).await;
    assert!(
        transport_for(&server)
            .running_models()
            .await
            .unwrap()
            .is_empty()
    );
}

// ---- provider resolution chain ----------------------------------------

fn chat_ok() -> serde_json::Value {
    json!({
        "model": MODEL,
        "created_at": "2026-06-07T00:00:00Z",
        "message": { "role": "assistant", "content": "hi" },
        "done": true,
        "done_reason": "stop"
    })
}

async fn mount_chat(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_ok()))
        .mount(server)
        .await;
}

fn request() -> CompletionRequest {
    CompletionRequest::builder(MODEL)
        .user_text("go")
        .max_tokens(16)
        .build()
        .unwrap()
}

#[tokio::test]
async fn ps_context_length_overrides_static_table() {
    let server = MockServer::start().await;
    mount_chat(&server).await;
    mount_ps(&server, ps_loaded()).await;
    let provider = provider_for(&server);

    // Before any turn, capabilities() reflects only the static table.
    assert_eq!(
        provider.capabilities(MODEL).max_input_tokens,
        STATIC_FALLBACK
    );

    provider.complete(request()).await.unwrap();

    // After a turn, the live /api/ps value wins.
    assert_eq!(provider.capabilities(MODEL).max_input_tokens, REAL_CTX);
}

#[tokio::test]
async fn show_resolves_when_model_not_loaded() {
    // The gemma3:1b scenario: /api/ps lists nothing (GGUF runner can't load
    // it), but /api/show still reports the model's max context.
    let server = MockServer::start().await;
    mount_chat(&server).await;
    mount_ps(&server, json!({ "models": [] })).await;
    mount_show(&server, show_body()).await;
    let provider = provider_for(&server);

    provider.complete(request()).await.unwrap();

    assert_eq!(provider.capabilities(MODEL).max_input_tokens, REAL_CTX);
}

#[tokio::test]
async fn static_fallback_when_endpoints_absent() {
    // Neither probe endpoint is mounted → both error → static table stands.
    let server = MockServer::start().await;
    mount_chat(&server).await;
    let provider = provider_for(&server);

    provider.complete(request()).await.unwrap();

    assert_eq!(
        provider.capabilities(MODEL).max_input_tokens,
        STATIC_FALLBACK
    );
}

#[tokio::test]
async fn refresh_models_overlays_live_context_window() {
    // `refresh_models` is the trait's live-discovery hook (#161): it probes
    // `/api/ps` and overlays each loaded model's real window onto the static
    // catalog, instead of returning the static table verbatim. `qwen3.5` is a
    // catalog entry; report a live, non-default window for it.
    const LIVE_CTX: u32 = 200_000;
    let server = MockServer::start().await;
    mount_ps(
        &server,
        json!({ "models": [
            { "name": "qwen3.5", "model": "qwen3.5", "context_length": LIVE_CTX }
        ] }),
    )
    .await;
    let provider = provider_for(&server);

    let models = provider.refresh_models().await.expect("refresh");
    let q = models
        .iter()
        .find(|m| m.id == "qwen3.5")
        .expect("qwen3.5 present in catalog");
    assert_eq!(q.capabilities.max_input_tokens, LIVE_CTX);

    // The probe also seeds the cache the sync `capabilities` reader overlays.
    assert_eq!(provider.capabilities("qwen3.5").max_input_tokens, LIVE_CTX);
}
