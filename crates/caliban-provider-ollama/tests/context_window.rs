#![allow(missing_docs)]
//! Runtime model discovery + context-window detection (#316, issue #60).
//!
//! Exercises the real `DirectTransport` against a mocked Ollama server so the
//! `/api/tags`, `/api/ps` and `/api/show` HTTP parsing and the `OllamaProvider`
//! discovery chain are covered end to end. Fixtures mirror bodies captured from
//! a live server (Ollama 0.30.6): `qwen3.6:27b-mlx` reports a 262144-token
//! window and `["completion","vision","thinking","tools"]` capabilities. There
//! is no static table — an undiscovered model gets the conservative bootstrap
//! window, not a wrong hardcoded value.

use caliban_provider::{CompletionRequest, Provider, ToolUseCapability};
use caliban_provider_ollama::discovery::BOOTSTRAP_CONTEXT;
use caliban_provider_ollama::transport::Transport;
use caliban_provider_ollama::transport::direct::DirectTransport;
use caliban_provider_ollama::{OllamaProvider, config::DirectConfig};
use serde_json::json;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "qwen3.6:27b-mlx";
const REAL_CTX: u32 = 262_144;

fn transport_for(server: &MockServer) -> DirectTransport {
    DirectTransport::new(DirectConfig {
        base_url: Url::parse(&server.uri()).unwrap(),
        timeout: std::time::Duration::from_secs(10),
        stream_total_timeout: None,
    })
    .unwrap()
}

fn provider_for(server: &MockServer) -> OllamaProvider<DirectTransport> {
    // `None` cache: never touch the real discovery cache dir from a test.
    OllamaProvider::from_transport_with_cache(transport_for(server), None)
}

async fn mount_tags(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
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
    json!({
        "model_info": {
            "general.architecture": "qwen3_5",
            "qwen3_5.context_length": REAL_CTX
        },
        "capabilities": ["completion", "vision", "thinking", "tools"]
    })
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
async fn ps_context_length_resolves_via_complete() {
    let server = MockServer::start().await;
    mount_chat(&server).await;
    mount_ps(&server, ps_loaded()).await;
    let provider = provider_for(&server);

    // Cold (no discovery, no cache): the honest bootstrap window — never a
    // wrong static per-model value.
    assert_eq!(
        provider.capabilities(MODEL).max_input_tokens,
        BOOTSTRAP_CONTEXT
    );

    provider.complete(request()).await.unwrap();

    // After a turn, the live /api/ps value is resolved.
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
async fn bootstrap_when_endpoints_absent() {
    // Neither probe endpoint is mounted → both error → the honest bootstrap
    // window stands (no static table). This is the "server unreachable, no
    // cache" degenerate case.
    let server = MockServer::start().await;
    mount_chat(&server).await;
    let provider = provider_for(&server);

    provider.complete(request()).await.unwrap();

    assert_eq!(
        provider.capabilities(MODEL).max_input_tokens,
        BOOTSTRAP_CONTEXT
    );
}

#[tokio::test]
async fn refresh_models_discovers_from_tags_with_caps_and_live_ctx() {
    // `refresh_models` lists `/api/tags`, enriches each via `/api/show`
    // (capabilities + context), and overlays the live `/api/ps` window. The
    // model is not in any static table — it's discovered entirely from the
    // server.
    let server = MockServer::start().await;
    mount_tags(
        &server,
        json!({ "models": [ { "name": MODEL, "details": { "family": "" } } ] }),
    )
    .await;
    mount_show(&server, show_body()).await;
    mount_ps(&server, ps_loaded()).await;
    let provider = provider_for(&server);

    let models = provider.refresh_models().await.expect("refresh");
    let m = models.iter().find(|m| m.id == MODEL).expect("discovered");
    // /api/ps live window wins; server capabilities are mapped.
    assert_eq!(m.capabilities.max_input_tokens, REAL_CTX);
    assert!(m.capabilities.vision);
    assert!(m.capabilities.thinking);
    assert_eq!(m.capabilities.tool_use, ToolUseCapability::ParallelCalls);

    // The sync readers reflect the discovery.
    assert_eq!(provider.capabilities(MODEL).max_input_tokens, REAL_CTX);
    assert_eq!(provider.list_models().len(), 1);
}
