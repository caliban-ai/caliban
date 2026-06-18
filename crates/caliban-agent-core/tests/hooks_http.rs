//! Integration tests for `HttpHook` (ADR 0024) — uses `wiremock`.

use std::collections::BTreeMap;
use std::time::Duration;

use caliban_agent_core::{HookDecision, Hooks, HttpHook, ToolCtx};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ctx<'a>(name: &'a str, input: &'a serde_json::Value) -> ToolCtx<'a> {
    ToolCtx {
        turn_index: 0,
        tool_use_id: "t1",
        tool_name: name,
        input,
        is_read_only: false,
    }
}

#[tokio::test]
async fn http_200_with_deny_body_denies() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/preflight"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"http says no"}}"#,
        ))
        .mount(&server)
        .await;

    let url = format!("{}/preflight", server.uri());
    let hook = HttpHook {
        if_pattern: None,
        asynchronous: false,
        url: url.clone(),
        headers: BTreeMap::new(),
        timeout: Duration::from_secs(5),
        allowed_url_globs: vec!["*".into()],
        event_name: "PreToolUse".into(),
        matcher: "*".into(),
        client: reqwest::Client::new(),
    };
    let input = serde_json::json!({});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    match d {
        HookDecision::Deny(msg) => assert!(msg.contains("http says no")),
        d => panic!("unexpected: {d:?}"),
    }
}

#[tokio::test]
async fn http_url_not_allowlisted_skips() {
    let server = MockServer::start().await;
    // No mock registered — calling the URL would surface as failure; we expect
    // the hook to skip due to allowlist BEFORE making the call.
    let url = format!("{}/preflight", server.uri());
    let hook = HttpHook {
        if_pattern: None,
        asynchronous: false,
        url: url.clone(),
        headers: BTreeMap::new(),
        timeout: Duration::from_secs(5),
        // Glob restricts to *.example.com — server URL won't match.
        allowed_url_globs: vec!["*.example.com/*".into()],
        event_name: "PreToolUse".into(),
        matcher: "*".into(),
        client: reqwest::Client::new(),
    };
    let input = serde_json::json!({});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    assert!(matches!(d, HookDecision::Allow));
}

#[tokio::test]
async fn http_non_2xx_is_allow() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/preflight"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let url = format!("{}/preflight", server.uri());
    let hook = HttpHook {
        if_pattern: None,
        asynchronous: false,
        url: url.clone(),
        headers: BTreeMap::new(),
        timeout: Duration::from_secs(5),
        allowed_url_globs: vec!["*".into()],
        event_name: "PreToolUse".into(),
        matcher: "*".into(),
        client: reqwest::Client::new(),
    };
    let input = serde_json::json!({});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    assert!(matches!(d, HookDecision::Allow));
}

#[tokio::test]
async fn http_updated_input_parses() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/rewrite"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"hookSpecificOutput":{"updatedInput":{"command":"echo redacted"}}}"#,
        ))
        .mount(&server)
        .await;

    let url = format!("{}/rewrite", server.uri());
    let hook = HttpHook {
        if_pattern: None,
        asynchronous: false,
        url: url.clone(),
        headers: BTreeMap::new(),
        timeout: Duration::from_secs(5),
        allowed_url_globs: vec!["*".into()],
        event_name: "PreToolUse".into(),
        matcher: "*".into(),
        client: reqwest::Client::new(),
    };
    let input = serde_json::json!({"command": "rm -rf /"});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    match d {
        HookDecision::UpdatedInput(v) => assert_eq!(v["command"], "echo redacted"),
        d => panic!("unexpected: {d:?}"),
    }
}

#[tokio::test]
async fn http_matcher_skips_non_matching_tools() {
    let server = MockServer::start().await;
    // Mock returns Deny — but matcher should skip dispatch first.
    Mock::given(method("POST"))
        .and(path("/wf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"hookSpecificOutput":{"permissionDecision":"deny"}}"#),
        )
        .mount(&server)
        .await;
    let url = format!("{}/wf", server.uri());
    let hook = HttpHook {
        if_pattern: None,
        asynchronous: false,
        url: url.clone(),
        headers: BTreeMap::new(),
        timeout: Duration::from_secs(5),
        allowed_url_globs: vec!["*".into()],
        event_name: "PreToolUse".into(),
        matcher: "WebFetch".into(),
        client: reqwest::Client::new(),
    };
    let input = serde_json::json!({});
    // Bash doesn't match WebFetch → handler skipped.
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    assert!(matches!(d, HookDecision::Allow));
}
