//! Hermetic tests for `VertexProvider` — no GCP credentials required.

#![allow(missing_docs)]
#![allow(clippy::duration_suboptimal_units)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use caliban_provider::{CompletionRequest, Provider};
use caliban_provider_vertex::{
    AuthRefresh, VertexConfig, VertexError, VertexProvider,
    models::{list_models_remote, strip_platform_suffix, vendored_vertex_models},
};
use gcp_auth::{Error as GcpError, Token, TokenProvider};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn env_map(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
    move |k: &str| map.get(k).cloned()
}

struct FixedTokenProvider {
    token: String,
    calls: Arc<AtomicU64>,
}

#[async_trait]
impl TokenProvider for FixedTokenProvider {
    async fn token(&self, _scopes: &[&str]) -> Result<Arc<Token>, GcpError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let payload = format!(
            r#"{{"access_token":"{}","expires_in":3600,"token_type":"Bearer"}}"#,
            self.token
        );
        let t: Token = serde_json::from_str(&payload).expect("token parse");
        Ok(Arc::new(t))
    }

    async fn project_id(&self) -> Result<Arc<str>, GcpError> {
        Ok(Arc::from("test-project"))
    }
}

fn fixed_token() -> (Arc<dyn TokenProvider>, Arc<AtomicU64>) {
    let calls = Arc::new(AtomicU64::new(0));
    let p: Arc<dyn TokenProvider> = Arc::new(FixedTokenProvider {
        token: "tok-xyz".into(),
        calls: calls.clone(),
    });
    (p, calls)
}

async fn make_provider() -> VertexProvider {
    let (provider, _) = fixed_token();
    let cfg = VertexConfig {
        project_id: "test-proj".into(),
        region: "us-east5".into(),
        service_account_key_path: None,
        auth_refresh: Duration::ZERO,
    };
    VertexProvider::from_parts(cfg, provider).await.unwrap()
}

// ---------------------------------------------------------------------------
// Config tests
// ---------------------------------------------------------------------------

#[test]
fn config_from_env_requires_project_and_region() {
    let getter = env_map(&[]);
    let err = VertexConfig::from_env_fn(&getter).unwrap_err();
    assert!(matches!(
        err,
        VertexError::MissingConfig("VERTEX_PROJECT_ID")
    ));
}

#[test]
fn config_from_env_reads_fields() {
    let getter = env_map(&[
        ("VERTEX_PROJECT_ID", "my-project"),
        ("VERTEX_REGION", "us-east5"),
        ("GOOGLE_APPLICATION_CREDENTIALS", "/etc/caliban/sa.json"),
        ("CALIBAN_GCP_AUTH_REFRESH", "10m"),
    ]);
    let cfg = VertexConfig::from_env_fn(&getter).expect("config");
    assert_eq!(cfg.project_id, "my-project");
    assert_eq!(cfg.region, "us-east5");
    assert_eq!(
        cfg.service_account_key_path.as_deref(),
        Some("/etc/caliban/sa.json")
    );
    assert_eq!(cfg.auth_refresh, Duration::from_secs(600));
}

#[test]
fn config_default_has_5min_refresh() {
    let cfg = VertexConfig::default();
    assert_eq!(cfg.auth_refresh, Duration::from_secs(300));
}

#[test]
fn config_falls_back_to_google_cloud_project() {
    let getter = env_map(&[
        ("GOOGLE_CLOUD_PROJECT", "fallback-proj"),
        ("VERTEX_REGION", "us-east5"),
    ]);
    let cfg = VertexConfig::from_env_fn(&getter).expect("config");
    assert_eq!(cfg.project_id, "fallback-proj");
}

// ---------------------------------------------------------------------------
// Auth refresh
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn auth_refresh_fetches_tokens_on_interval() {
    let (provider, calls) = fixed_token();
    let auth = AuthRefresh::spawn(provider, Duration::from_secs(60));
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_secs(125)).await;
    assert!(
        auth.refresh_count() >= 1,
        "expected at least one refresh, got {}",
        auth.refresh_count()
    );
    assert!(
        calls.load(Ordering::Relaxed) >= 1,
        "token provider should be called"
    );
}

// ---------------------------------------------------------------------------
// Provider behavior tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provider_name_returns_vertex() {
    let p = make_provider().await;
    assert_eq!(p.name(), "vertex");
}

#[tokio::test]
async fn provider_capabilities_match_anthropic() {
    let p = make_provider().await;
    // Vertex's capabilities() should strip an `@<date>` suffix and look up
    // the canonical model in the Anthropic table.
    let vertex_caps = p.capabilities("claude-sonnet-4-6@20260101");
    let anthropic_caps = caliban_provider_anthropic::models::capabilities_for("claude-sonnet-4-6");
    assert_eq!(vertex_caps, anthropic_caps);
    assert!(vertex_caps.vision);
}

#[tokio::test]
async fn provider_list_models_filters_to_anthropic() {
    let p = make_provider().await;
    let models = p.list_models();
    assert!(!models.is_empty());
    // The vendored list comes straight from the Anthropic table; with
    // current dateless Claude 4.x IDs, the wire form has no `@`. We
    // just assert the list is non-empty and surfaces a known canonical
    // ID so the table → vertex mapping stays wired.
    assert!(models.iter().any(|m| m.id == "claude-sonnet-4-6"));
}

#[test]
fn vendored_vertex_models_include_known_ids() {
    let models = vendored_vertex_models();
    assert!(models.iter().any(|m| m.id == "claude-opus-4-7"));
    assert!(models.iter().any(|m| m.id == "claude-sonnet-4-6"));
    assert!(models.iter().any(|m| m.id == "claude-haiku-4-5"));
}

#[test]
fn strip_platform_suffix_drops_at_date() {
    assert_eq!(
        strip_platform_suffix("claude-sonnet-4-6@20260101"),
        "claude-sonnet-4-6"
    );
    assert_eq!(strip_platform_suffix("custom-model"), "custom-model");
}

// ---------------------------------------------------------------------------
// HTTP-mock tests against a wiremock server.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_models_remote_parses_publishers_response() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "models": [
            {
                "name": "publishers/anthropic/models/claude-sonnet-4-6@20260101",
                "display_name": "Claude Sonnet 4.6"
            },
            {
                "name": "publishers/anthropic/models/claude-haiku-4-5@20251001",
                "display_name": "Claude Haiku 4.5"
            }
        ]
    });
    Mock::given(method("GET"))
        .and(path("/v1/publishers/anthropic/models"))
        .and(header("Authorization", "Bearer tok-xyz"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    // Exercise the live-discovery code path directly (the same fetch+parse
    // `refresh_models` runs), pointed at wiremock via the public
    // `list_models_remote` against a caller-supplied base URL.
    let (token_provider, _) = fixed_token();
    let client = caliban_common::http::default_client();
    let models = list_models_remote(&client, &token_provider, &server.uri())
        .await
        .expect("list");
    assert_eq!(models.len(), 2);
    assert!(models.iter().any(|m| m.id == "claude-sonnet-4-6"));
}

#[tokio::test]
async fn list_models_remote_surfaces_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/publishers/anthropic/models"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let (token_provider, _) = fixed_token();
    let client = caliban_common::http::default_client();
    let err = list_models_remote(&client, &token_provider, &server.uri())
        .await
        .expect_err("should error");
    assert!(matches!(err, VertexError::InvalidConfig(_)));
}

#[tokio::test]
async fn complete_call_construction_exercises_transport() {
    // We can't easily redirect the Vertex transport at a wiremock server
    // (it hard-codes the `{region}-aiplatform.googleapis.com` host). This
    // test just exercises the provider/transport construction + request
    // shape; the actual network call is expected to fail because Vertex
    // is unreachable from the test host (no real auth, fake region).
    let p = make_provider().await;
    let req = CompletionRequest::builder("claude-sonnet-4-6")
        .system("sys")
        .user_text("hello")
        .max_tokens(16)
        .build()
        .unwrap();
    let res = p.complete(req).await;
    // Either an auth or network error — never a panic / type error.
    assert!(res.is_err());
}

/// Live Vertex test. Only runs if `CALIBAN_LIVE_VERTEX=1`.
#[tokio::test]
#[ignore = "requires real GCP credentials and CALIBAN_LIVE_VERTEX=1"]
async fn live_vertex_smoke() {
    if std::env::var("CALIBAN_LIVE_VERTEX").ok().as_deref() != Some("1") {
        return;
    }
    let cfg = VertexConfig::from_env().expect("env");
    let _provider = VertexProvider::from_config(cfg).await.expect("provider");
}
