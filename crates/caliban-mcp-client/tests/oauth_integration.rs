//! Phase C OAuth integration tests — discovery + flow + persistence.
//!
//! Drives `caliban_mcp_client::oauth` against a mock authorization server
//! built with `wiremock`, exercising:
//!
//! * RFC 8414 / oauth-protected-resource discovery
//! * PKCE authorization-code flow with a loopback callback
//! * Token persistence + reuse via `MemoryStore` / `FileStore`
//! * Inline refresh near expiry
//! * Manual config skips discovery
//! * 401-on-use clears the cached token

#![allow(clippy::missing_panics_doc, clippy::pedantic)]

use std::time::Duration;

use caliban_mcp_client::config::OauthMode;
use caliban_mcp_client::oauth::{
    FileStore, ManualOauthConfig, MemoryStore, OauthAuthenticator, OauthEndpoints, OauthFlow,
    OauthFlowOptions, OauthTokens, TokenStore, discover_endpoints, endpoints_from_manual,
    refresh_tokens,
};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use url::Url;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Shared fixture helpers
// ---------------------------------------------------------------------------

async fn spawn_mock_oauth_server() -> MockServer {
    MockServer::start().await
}

async fn install_discovery_routes(server: &MockServer, audience: &str) {
    // The resource lives under `/mcp`, so RFC 9728 discovery hits the
    // PATH-PRESERVING well-known `/.well-known/oauth-protected-resource/mcp`.
    // The authorization server itself lives under `/login/oauth` (as GitHub's
    // does), so its RFC 8414 metadata is at the path-preserving
    // `/.well-known/oauth-authorization-server/login/oauth`. This exercises the
    // path-preserving `join_wellknown` fix end-to-end.
    let base = server.uri();
    let as_issuer = format!("{base}/login/oauth");
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resource": audience,
            "authorization_servers": [as_issuer],
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server/login/oauth"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authorization_endpoint": format!("{base}/oauth/authorize"),
            "token_endpoint":         format!("{base}/oauth/token"),
            "scopes_supported":       ["read", "write"],
        })))
        .mount(server)
        .await;
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build http client")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 1. Discovery happy path — protected-resource doc + RFC 8414 metadata.
#[tokio::test]
async fn discovery_returns_endpoints_via_well_known() {
    let server = spawn_mock_oauth_server().await;
    install_discovery_routes(&server, "https://api.example/mcp").await;
    let server_url = Url::parse(&format!("{}/mcp", server.uri())).unwrap();
    let endpoints = discover_endpoints("demo", &server_url, &http())
        .await
        .expect("discover");
    assert!(
        endpoints.auth_url.as_str().ends_with("/oauth/authorize"),
        "got: {}",
        endpoints.auth_url,
    );
    assert!(endpoints.token_url.as_str().ends_with("/oauth/token"));
    assert_eq!(
        endpoints.scopes,
        vec!["read".to_string(), "write".to_string()]
    );
    assert_eq!(endpoints.audience, "https://api.example/mcp");
}

/// 2. Manual config skips discovery entirely — we never hit the mock server.
#[tokio::test]
async fn manual_config_skips_discovery() {
    let cfg = ManualOauthConfig {
        client_id: Some("cid".to_string()),
        client_secret: None,
        auth_url: Some("https://manual/authorize".to_string()),
        token_url: Some("https://manual/token".to_string()),
        scopes: vec!["scope-a".to_string()],
        audience: Some("aud".to_string()),
    };
    let server_url = Url::parse("https://api.example/mcp").unwrap();
    let endpoints = endpoints_from_manual("s", &cfg, &server_url).expect("manual");
    assert_eq!(endpoints.auth_url.as_str(), "https://manual/authorize");
    assert_eq!(endpoints.token_url.as_str(), "https://manual/token");
    assert_eq!(endpoints.audience, "aud");
    assert_eq!(endpoints.scopes, vec!["scope-a".to_string()]);
}

/// 3. Full PKCE flow happy path — start the flow, drive the redirect to the
///    loopback URL with a fake `code`, observe the token endpoint POST,
///    receive the tokens.
#[tokio::test]
async fn pkce_flow_happy_path() {
    let server = spawn_mock_oauth_server().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "the-access-token",
            "refresh_token": "the-refresh-token",
            "expires_in": 3600,
            "scope": "read write",
        })))
        .mount(&server)
        .await;
    let endpoints = OauthEndpoints {
        auth_url: Url::parse(&format!("{}/oauth/authorize", server.uri())).unwrap(),
        token_url: Url::parse(&format!("{}/oauth/token", server.uri())).unwrap(),
        scopes: vec!["read".to_string()],
        audience: "aud".to_string(),
    };
    let opts = OauthFlowOptions::new("demo".to_string(), endpoints, "client-id".to_string());
    let flow = OauthFlow::start(opts).await.expect("start flow");
    let auth_url = flow.auth_url.clone();
    // Inspect the URL — it must include code_challenge + S256 + state.
    let qp: std::collections::HashMap<String, String> = auth_url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(qp.get("response_type").map(String::as_str), Some("code"));
    assert_eq!(
        qp.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert!(qp.contains_key("code_challenge"));
    assert!(qp.contains_key("state"));
    let redirect_uri = qp.get("redirect_uri").cloned().expect("redirect_uri");
    let state = qp.get("state").cloned().expect("state");
    // Spawn the "browser" GET to the redirect URL with code+state.
    let cb_client = reqwest::Client::new();
    tokio::spawn(async move {
        // Tiny delay so the await_callback path is set up.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let url = format!("{redirect_uri}?code=fake-auth-code&state={state}");
        let _ = cb_client.get(url).send().await;
    });
    let tokens = flow.await_callback(&http()).await.expect("await_callback");
    assert_eq!(tokens.access_token, "the-access-token");
    assert_eq!(tokens.refresh_token.as_deref(), Some("the-refresh-token"));
    assert!(tokens.expires_at.is_some());
}

/// 4. Cancelled flow — drop the OauthFlow before any callback arrives.
///    `await_callback` should time out promptly (we use a short cap) and
///    surface `OauthFlow { message: "callback timed out…" }`.
#[tokio::test]
async fn flow_times_out_when_user_never_completes() {
    let server = spawn_mock_oauth_server().await;
    let endpoints = OauthEndpoints {
        auth_url: Url::parse(&format!("{}/oauth/authorize", server.uri())).unwrap(),
        token_url: Url::parse(&format!("{}/oauth/token", server.uri())).unwrap(),
        scopes: vec![],
        audience: "aud".to_string(),
    };
    let mut opts = OauthFlowOptions::new("demo".to_string(), endpoints, "cid".to_string());
    opts.callback_timeout = Duration::from_millis(150);
    let flow = OauthFlow::start(opts).await.expect("start");
    let err = flow.await_callback(&http()).await.expect_err("should fail");
    let msg = err.to_string();
    assert!(msg.contains("timed out"), "got: {msg}");
}

/// 5. Token persist + reuse via the memory store.
#[tokio::test]
async fn token_store_persists_and_reuses() {
    let store = MemoryStore::default();
    let tokens = OauthTokens {
        access_token: "fresh".to_string(),
        refresh_token: Some("r".to_string()),
        expires_at: Some(Utc::now() + chrono::Duration::seconds(3600)),
        scopes: vec!["read".to_string()],
        client_id: None,
    };
    store.put("svc", "aud", &tokens).expect("put");
    let cached = store.get("svc", "aud").expect("get").expect("some");
    assert_eq!(cached.access_token, "fresh");
    assert!(!cached.needs_refresh(Utc::now()));
}

/// 6. Refresh inline near expiry — old refresh token swaps for a new bundle.
#[tokio::test]
async fn refresh_swaps_in_new_access_token() {
    let server = spawn_mock_oauth_server().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "refreshed-access",
            "expires_in": 7200,
            "scope": "read"
        })))
        .mount(&server)
        .await;
    let endpoints = OauthEndpoints {
        auth_url: Url::parse(&format!("{}/oauth/authorize", server.uri())).unwrap(),
        token_url: Url::parse(&format!("{}/oauth/token", server.uri())).unwrap(),
        scopes: vec!["read".to_string()],
        audience: "aud".to_string(),
    };
    let old = OauthTokens {
        access_token: "expiring".to_string(),
        refresh_token: Some("rtok".to_string()),
        // Mark as expiring inside the refresh margin.
        expires_at: Some(Utc::now() + chrono::Duration::seconds(10)),
        scopes: vec![],
        client_id: None,
    };
    assert!(old.needs_refresh(Utc::now()));
    let new = refresh_tokens(&http(), "svc", &endpoints, "cid", None, &old)
        .await
        .expect("refresh");
    assert_eq!(new.access_token, "refreshed-access");
    // The auth server omitted refresh_token in its response, so we
    // preserve the previous one for future refreshes.
    assert_eq!(new.refresh_token.as_deref(), Some("rtok"));
}

/// 7. Token endpoint 401 → `OauthExchange` error surfaced.
#[tokio::test]
async fn token_endpoint_401_surfaces_exchange_error() {
    let server = spawn_mock_oauth_server().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(401).set_body_string("expired refresh token"))
        .mount(&server)
        .await;
    let endpoints = OauthEndpoints {
        auth_url: Url::parse(&format!("{}/oauth/authorize", server.uri())).unwrap(),
        token_url: Url::parse(&format!("{}/oauth/token", server.uri())).unwrap(),
        scopes: vec![],
        audience: "aud".to_string(),
    };
    let old = OauthTokens {
        access_token: "x".to_string(),
        refresh_token: Some("r".to_string()),
        expires_at: None,
        scopes: vec![],
        client_id: None,
    };
    let err = refresh_tokens(&http(), "svc", &endpoints, "cid", None, &old)
        .await
        .expect_err("should fail");
    let s = err.to_string();
    assert!(
        s.contains("401") || s.contains("token endpoint"),
        "got: {s}"
    );
}

/// 8. 401-on-use semantics — the store's `clear` removes the cached
///    entry, simulating what the manager does when it observes a 401
///    from the resource server during a tool call.
#[tokio::test]
async fn clear_on_401_removes_cached_token() {
    let store = MemoryStore::default();
    store
        .put(
            "svc",
            "aud",
            &OauthTokens {
                access_token: "stale".to_string(),
                refresh_token: None,
                expires_at: None,
                scopes: vec![],
                client_id: None,
            },
        )
        .expect("put");
    assert!(store.get("svc", "aud").expect("get").is_some());
    store.clear("svc", "aud").expect("clear");
    assert!(store.get("svc", "aud").expect("get").is_none());
}

/// 9. Keyring fallback to file when keychain probe fails — drive the
///    fallback path manually via `FileStore` (we don't have a portable
///    way to make `keyring` fail at runtime in a test).
#[tokio::test]
async fn file_store_serves_as_keyring_fallback() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("tokens.json");
    let store = FileStore::new(path.clone());
    let tokens = OauthTokens {
        access_token: "via-file".to_string(),
        refresh_token: None,
        expires_at: None,
        scopes: vec![],
        client_id: None,
    };
    store.put("svc", "aud", &tokens).expect("put");
    let reread = FileStore::new(path);
    let got = reread.get("svc", "aud").expect("get").expect("some");
    assert_eq!(got.access_token, "via-file");
}

/// 10. Token-endpoint response that returns *no* refresh_token preserves
///     the existing one across refreshes.
#[tokio::test]
async fn refresh_preserves_existing_refresh_token() {
    let server = spawn_mock_oauth_server().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "new-access",
            "expires_in": 60
        })))
        .mount(&server)
        .await;
    let endpoints = OauthEndpoints {
        auth_url: Url::parse(&format!("{}/oauth/authorize", server.uri())).unwrap(),
        token_url: Url::parse(&format!("{}/oauth/token", server.uri())).unwrap(),
        scopes: vec![],
        audience: "aud".to_string(),
    };
    let old = OauthTokens {
        access_token: "old".to_string(),
        refresh_token: Some("preserved".to_string()),
        expires_at: None,
        scopes: vec![],
        client_id: None,
    };
    let new = refresh_tokens(&http(), "svc", &endpoints, "cid", None, &old)
        .await
        .expect("refresh");
    assert_eq!(new.refresh_token.as_deref(), Some("preserved"));
}

/// 11. State-mismatch on callback → `OauthFlow` error.
#[tokio::test]
async fn state_mismatch_in_callback_surfaces_error() {
    let server = spawn_mock_oauth_server().await;
    let endpoints = OauthEndpoints {
        auth_url: Url::parse(&format!("{}/oauth/authorize", server.uri())).unwrap(),
        token_url: Url::parse(&format!("{}/oauth/token", server.uri())).unwrap(),
        scopes: vec![],
        audience: "aud".to_string(),
    };
    let mut opts = OauthFlowOptions::new("demo".to_string(), endpoints, "cid".to_string());
    opts.callback_timeout = Duration::from_secs(2);
    let flow = OauthFlow::start(opts).await.expect("start");
    let redirect_uri = flow
        .auth_url
        .query_pairs()
        .find(|(k, _)| k == "redirect_uri")
        .map(|(_, v)| v.into_owned())
        .expect("redirect_uri");
    let cb_client = reqwest::Client::new();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        // Bogus state.
        let url = format!("{redirect_uri}?code=abc&state=wrong-state");
        let _ = cb_client.get(url).send().await;
    });
    let err = flow.await_callback(&http()).await.expect_err("fail");
    let s = err.to_string();
    assert!(s.contains("state mismatch"), "got: {s}");
}

/// 12. Discovery POSTs include the body shape the token endpoint expects.
///     This is a sanity check on the form encoder.
#[tokio::test]
async fn refresh_request_body_includes_required_fields() {
    let server = spawn_mock_oauth_server().await;
    // Capture the request body via a permissive match.
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "x",
            "expires_in": 60
        })))
        .mount(&server)
        .await;
    let endpoints = OauthEndpoints {
        auth_url: Url::parse(&format!("{}/oauth/authorize", server.uri())).unwrap(),
        token_url: Url::parse(&format!("{}/oauth/token", server.uri())).unwrap(),
        scopes: vec!["read".to_string(), "write".to_string()],
        audience: "aud".to_string(),
    };
    let old = OauthTokens {
        access_token: "a".to_string(),
        refresh_token: Some("r".to_string()),
        expires_at: None,
        scopes: vec![],
        client_id: None,
    };
    let _ = refresh_tokens(
        &http(),
        "svc",
        &endpoints,
        "client-id",
        Some("client-secret"),
        &old,
    )
    .await
    .expect("refresh");
    let received = server.received_requests().await.unwrap();
    let post = received
        .iter()
        .find(|r| r.url.path() == "/oauth/token")
        .expect("post received");
    let body_str = std::str::from_utf8(&post.body).expect("utf8");
    assert!(body_str.contains("grant_type=refresh_token"), "{body_str}");
    assert!(body_str.contains("refresh_token=r"), "{body_str}");
    assert!(body_str.contains("client_id=client-id"), "{body_str}");
    assert!(
        body_str.contains("client_secret=client-secret"),
        "{body_str}"
    );
    // Whitespace in `scope=read write` is form-urlencoded as `+` or %20.
    assert!(
        body_str.contains("scope=read+write") || body_str.contains("scope=read%20write"),
        "{body_str}",
    );
}

// ---------------------------------------------------------------------------
// OauthAuthenticator — connect-path orchestration (the wiring under test in
// #300). These prove the reuse / refresh / headless-no-hang / no-client-id
// decisions without ever opening a browser.
// ---------------------------------------------------------------------------

fn manual_cfg(server: &MockServer) -> ManualOauthConfig {
    ManualOauthConfig {
        client_id: Some("cid".to_string()),
        client_secret: None,
        auth_url: Some(format!("{}/oauth/authorize", server.uri())),
        token_url: Some(format!("{}/oauth/token", server.uri())),
        scopes: vec!["read".to_string()],
        audience: Some("aud".to_string()),
    }
}

/// A fresh (non-expiring) cached token is reused verbatim — no browser, no
/// network. Manual mode so no discovery is needed either.
#[tokio::test]
async fn authenticator_reuses_cached_token() {
    let server = spawn_mock_oauth_server().await;
    let store: Arc<dyn TokenStore> = Arc::new(MemoryStore::default());
    store
        .put(
            "github",
            "aud",
            &OauthTokens {
                access_token: "cached-access".to_string(),
                refresh_token: Some("r".to_string()),
                expires_at: Some(Utc::now() + chrono::Duration::seconds(3600)),
                scopes: vec![],
                client_id: Some("cid".to_string()),
            },
        )
        .expect("put");
    let auth = OauthAuthenticator::new(http(), Arc::clone(&store), /* interactive */ true);
    let url = Url::parse("https://api.example/mcp").unwrap();
    let token = auth
        .bearer_for("github", OauthMode::Manual, &url, &manual_cfg(&server))
        .await
        .expect("bearer");
    assert_eq!(token.as_deref(), Some("cached-access"));
}

/// A cold cache in headless mode (interactive = false) fails with an
/// actionable error instead of hanging on a loopback callback.
#[tokio::test]
async fn authenticator_headless_cold_cache_errors() {
    let server = spawn_mock_oauth_server().await;
    let store: Arc<dyn TokenStore> = Arc::new(MemoryStore::default());
    let auth = OauthAuthenticator::new(http(), store, /* interactive */ false);
    let url = Url::parse("https://api.example/mcp").unwrap();
    let err = auth
        .bearer_for("github", OauthMode::Manual, &url, &manual_cfg(&server))
        .await
        .expect_err("headless cold cache must error, not hang");
    let s = err.to_string();
    assert!(s.contains("interactive"), "got: {s}");
}

/// `auto` mode with a successful discovery but no configured client_id (and no
/// dynamic registration) fails with the no-client-id error — checked before
/// interactivity, so it fires even when interactive.
#[tokio::test]
async fn authenticator_auto_without_client_id_errors() {
    let server = spawn_mock_oauth_server().await;
    install_discovery_routes(&server, "aud").await;
    let store: Arc<dyn TokenStore> = Arc::new(MemoryStore::default());
    let auth = OauthAuthenticator::new(http(), store, /* interactive */ true);
    let server_url = Url::parse(&format!("{}/mcp", server.uri())).unwrap();
    let err = auth
        .bearer_for(
            "github",
            OauthMode::Auto,
            &server_url,
            &ManualOauthConfig::default(),
        )
        .await
        .expect_err("no client_id + no DCR must error");
    let s = err.to_string();
    assert!(
        s.contains("dynamic client registration") || s.contains("client_id"),
        "got: {s}",
    );
}

/// A near-expiry cached token is silently refreshed (no browser), the refreshed
/// token is returned and persisted. Manual mode → the token endpoint is the
/// mock's `/oauth/token`.
#[tokio::test]
async fn authenticator_refreshes_expiring_token() {
    let server = spawn_mock_oauth_server().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "refreshed-access",
            "expires_in": 3600
        })))
        .mount(&server)
        .await;
    let store: Arc<dyn TokenStore> = Arc::new(MemoryStore::default());
    store
        .put(
            "github",
            "aud",
            &OauthTokens {
                access_token: "old-access".to_string(),
                refresh_token: Some("rtok".to_string()),
                expires_at: Some(Utc::now() + chrono::Duration::seconds(10)),
                scopes: vec![],
                client_id: Some("cid".to_string()),
            },
        )
        .expect("put");
    let auth = OauthAuthenticator::new(http(), Arc::clone(&store), /* interactive */ false);
    let url = Url::parse("https://api.example/mcp").unwrap();
    let token = auth
        .bearer_for("github", OauthMode::Manual, &url, &manual_cfg(&server))
        .await
        .expect("bearer");
    assert_eq!(token.as_deref(), Some("refreshed-access"));
    // Persisted for next time.
    let cached = store.get("github", "aud").expect("get").expect("some");
    assert_eq!(cached.access_token, "refreshed-access");
    assert_eq!(cached.client_id.as_deref(), Some("cid"));
}
