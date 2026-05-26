//! `WebSearch` tool — query a web search API and return top-K ranked results
//! as text blocks.
//!
//! Provider selection by env var `CALIBAN_WEBSEARCH_PROVIDER` ∈
//! `{"brave"|"tavily"|"exa"}`; default `brave`. Each provider reads its own
//! API key from env (`BRAVE_API_KEY` / `TAVILY_API_KEY` / `EXA_API_KEY`).
//!
//! Missing key → structured `ToolError::Execution` naming the env var so the
//! agent can try a different approach rather than failing the registry.
//!
//! See `docs/superpowers/specs/2026-05-24-builtin-tool-gaps-design.md`.

use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};

const DEFAULT_COUNT: u32 = 10;
const MAX_COUNT: u32 = 20;
const DEFAULT_TIMEOUT_SECS: u64 = 30;

const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";
const EXA_ENDPOINT: &str = "https://api.exa.ai/search";

/// One web search result hit.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Result title.
    pub title: String,
    /// Result URL.
    pub url: String,
    /// Snippet / description.
    pub snippet: String,
}

/// Selectable provider backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Brave Search.
    Brave,
    /// Tavily Search.
    Tavily,
    /// Exa Search.
    Exa,
}

impl Provider {
    /// Lowercase id string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Brave => "brave",
            Self::Tavily => "tavily",
            Self::Exa => "exa",
        }
    }

    /// Environment variable name holding the API key for this provider.
    #[must_use]
    pub fn env_var(self) -> &'static str {
        match self {
            Self::Brave => "BRAVE_API_KEY",
            Self::Tavily => "TAVILY_API_KEY",
            Self::Exa => "EXA_API_KEY",
        }
    }

    /// Parse a provider id (case-insensitive). Unrecognized → `None`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "brave" => Some(Self::Brave),
            "tavily" => Some(Self::Tavily),
            "exa" => Some(Self::Exa),
            _ => None,
        }
    }
}

/// `WebSearch` tool — issues a query against the configured provider and
/// returns ranked results.
pub struct WebSearchTool {
    client: reqwest::Client,
    /// Optional endpoint overrides (set by tests; production uses provider
    /// defaults).
    brave_endpoint: Option<String>,
    tavily_endpoint: Option<String>,
    exa_endpoint: Option<String>,
    schema: OnceLock<Value>,
}

impl std::fmt::Debug for WebSearchTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSearchTool")
            .field("client", &self.client)
            .field("brave_endpoint", &self.brave_endpoint)
            .field("tavily_endpoint", &self.tavily_endpoint)
            .field("exa_endpoint", &self.exa_endpoint)
            .finish_non_exhaustive()
    }
}

impl WebSearchTool {
    /// Build a tool using the given HTTP client.
    ///
    /// Production callers should pass a client built with
    /// [`caliban_common::http::default_client`] so the user-agent, TLS,
    /// HTTP/2, and timeout defaults are shared with provider transports.
    /// Tests inject their own client (typically a no-config
    /// `reqwest::Client::new()`) to bypass DNS / TLS for `wiremock`.
    #[must_use]
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            brave_endpoint: None,
            tavily_endpoint: None,
            exa_endpoint: None,
            schema: OnceLock::new(),
        }
    }

    /// Override the Brave endpoint URL (for tests).
    #[must_use]
    pub fn with_brave_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.brave_endpoint = Some(endpoint.into());
        self
    }

    /// Override the Tavily endpoint URL (for tests).
    #[must_use]
    pub fn with_tavily_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.tavily_endpoint = Some(endpoint.into());
        self
    }

    /// Override the Exa endpoint URL (for tests).
    #[must_use]
    pub fn with_exa_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.exa_endpoint = Some(endpoint.into());
        self
    }

    fn endpoint_for(&self, p: Provider) -> &str {
        match p {
            Provider::Brave => self.brave_endpoint.as_deref().unwrap_or(BRAVE_ENDPOINT),
            Provider::Tavily => self.tavily_endpoint.as_deref().unwrap_or(TAVILY_ENDPOINT),
            Provider::Exa => self.exa_endpoint.as_deref().unwrap_or(EXA_ENDPOINT),
        }
    }
}

#[derive(Debug, Deserialize)]
struct WebSearchInput {
    query: String,
    #[serde(default)]
    max_results: Option<u32>,
}

/// Resolve the currently selected provider from the environment. Defaults to
/// Brave when the env var is missing or unrecognized.
#[must_use]
pub fn selected_provider() -> Provider {
    std::env::var("CALIBAN_WEBSEARCH_PROVIDER")
        .ok()
        .and_then(|s| Provider::parse(&s))
        .unwrap_or(Provider::Brave)
}

// ---------------------------------------------------------------------------
// Provider response parsers
// ---------------------------------------------------------------------------

fn parse_brave_response(body: &str) -> Result<Vec<SearchHit>, ToolError> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        ToolError::execution(std::io::Error::other(format!(
            "brave: invalid JSON response: {e}"
        )))
    })?;
    let results = v
        .pointer("/web/results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let hits = results
        .into_iter()
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.to_string();
            let url = item.get("url")?.as_str()?.to_string();
            let snippet = item
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Some(SearchHit {
                title,
                url,
                snippet,
            })
        })
        .collect();
    Ok(hits)
}

fn parse_tavily_response(body: &str) -> Result<Vec<SearchHit>, ToolError> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        ToolError::execution(std::io::Error::other(format!(
            "tavily: invalid JSON response: {e}"
        )))
    })?;
    let results = v
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let hits = results
        .into_iter()
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.to_string();
            let url = item.get("url")?.as_str()?.to_string();
            let snippet = item
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Some(SearchHit {
                title,
                url,
                snippet,
            })
        })
        .collect();
    Ok(hits)
}

fn parse_exa_response(body: &str) -> Result<Vec<SearchHit>, ToolError> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        ToolError::execution(std::io::Error::other(format!(
            "exa: invalid JSON response: {e}"
        )))
    })?;
    let results = v
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let hits = results
        .into_iter()
        .filter_map(|item| {
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("(untitled)")
                .to_string();
            let url = item.get("url")?.as_str()?.to_string();
            let snippet = item
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| item.get("snippet").and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
            Some(SearchHit {
                title,
                url,
                snippet,
            })
        })
        .collect();
    Ok(hits)
}

// ---------------------------------------------------------------------------
// Tool impl
// ---------------------------------------------------------------------------

fn format_hits(query: &str, provider: Provider, hits: &[SearchHit], elapsed_ms: u128) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let plural = if hits.len() == 1 { "" } else { "s" };
    let provider_name = provider.as_str();
    let count = hits.len();
    // write! to a String never fails.
    let _ = writeln!(
        out,
        "Searched \"{query}\" via {provider_name} ({count} result{plural} in {elapsed_ms} ms)\n"
    );
    for (i, hit) in hits.iter().enumerate() {
        let n = i + 1;
        let _ = writeln!(out, "{n}. {} — {}", hit.title, hit.url);
        if !hit.snippet.is_empty() {
            let _ = writeln!(out, "   {}", hit.snippet);
        }
        out.push('\n');
    }
    out
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "WebSearch"
    }

    fn description(&self) -> &'static str {
        "Run a web search query and return the top-K ranked results (title, URL, snippet). Provider is selected by CALIBAN_WEBSEARCH_PROVIDER ∈ {brave|tavily|exa}; default is brave. Each provider reads its API key from its own env var (BRAVE_API_KEY / TAVILY_API_KEY / EXA_API_KEY). A missing key returns a structured error so the agent can fall back."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query." },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (1..=20, default 10).",
                        "minimum": 1,
                        "maximum": MAX_COUNT,
                    }
                },
                "required": ["query"]
            })
        })
    }

    async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: WebSearchInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;
        if parsed.query.trim().is_empty() {
            return Err(ToolError::invalid_input("query must be non-empty"));
        }
        let count = parsed
            .max_results
            .unwrap_or(DEFAULT_COUNT)
            .clamp(1, MAX_COUNT);

        let provider = selected_provider();
        let api_key = std::env::var(provider.env_var()).map_err(|_| {
            ToolError::execution(std::io::Error::other(format!(
                "WebSearch is not configured: missing {} for provider {}. \
                 Set CALIBAN_WEBSEARCH_PROVIDER + the matching key \
                 (BRAVE_API_KEY / TAVILY_API_KEY / EXA_API_KEY) or set the default key.",
                provider.env_var(),
                provider.as_str(),
            )))
        })?;

        let start = std::time::Instant::now();

        let request = match provider {
            Provider::Brave => self
                .client
                .get(self.endpoint_for(provider))
                .header("X-Subscription-Token", &api_key)
                .header("Accept", "application/json")
                .query(&[("q", parsed.query.as_str()), ("count", &count.to_string())]),
            Provider::Tavily => self
                .client
                .post(self.endpoint_for(provider))
                .header("Content-Type", "application/json")
                .json(&json!({
                    "api_key": api_key,
                    "query": parsed.query,
                    "max_results": count,
                })),
            Provider::Exa => self
                .client
                .post(self.endpoint_for(provider))
                .header("x-api-key", &api_key)
                .header("Content-Type", "application/json")
                .json(&json!({
                    "query": parsed.query,
                    "numResults": count,
                })),
        };

        let send_fut = request.send();
        let resp = tokio::select! {
            () = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
            () = tokio::time::sleep(Duration::from_secs(DEFAULT_TIMEOUT_SECS)) => {
                return Err(ToolError::execution(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("WebSearch timed out after {DEFAULT_TIMEOUT_SECS}s"),
                )));
            }
            r = send_fut => r.map_err(ToolError::execution)?,
        };

        let status = resp.status();
        let body = resp.text().await.map_err(ToolError::execution)?;
        if !status.is_success() {
            return Err(ToolError::execution(std::io::Error::other(format!(
                "{} returned status {} {}: {}",
                provider.as_str(),
                status.as_u16(),
                status.canonical_reason().unwrap_or(""),
                body,
            ))));
        }

        let hits = match provider {
            Provider::Brave => parse_brave_response(&body)?,
            Provider::Tavily => parse_tavily_response(&body)?,
            Provider::Exa => parse_exa_response(&body)?,
        };

        let elapsed_ms = start.elapsed().as_millis();
        let text = format_hits(&parsed.query, provider, &hits, elapsed_ms);
        Ok(vec![ContentBlock::Text(TextBlock {
            text,
            cache_control: None,
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use tokio_util::sync::CancellationToken;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx() -> ToolContext {
        ToolContext {
            tool_use_id: "t1".into(),
            cancel: CancellationToken::new(),
            hooks: None,
            turn_index: 0,
        }
    }

    /// Lock guarding env-var mutations across tests. Tests that touch env
    /// vars `CALIBAN_WEBSEARCH_PROVIDER` and `*_API_KEY` must acquire this
    /// to avoid races in the parallel test runner. `tokio::sync::Mutex` is
    /// async-aware so guards can be held across `.await` without tripping
    /// `clippy::await_holding_lock`.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    // SAFETY: env mutations are guarded by ENV_LOCK across all callers in
    // this test module. The std::env::set_var/remove_var APIs are unsafe
    // because POSIX setenv races with concurrent getenv on other threads;
    // serializing the mutations via the mutex avoids that.
    #[allow(unsafe_code)]
    fn set_env(provider: Option<&str>, api_key_env: Option<&str>, api_key_value: Option<&str>) {
        match provider {
            Some(p) => unsafe { std::env::set_var("CALIBAN_WEBSEARCH_PROVIDER", p) },
            None => unsafe { std::env::remove_var("CALIBAN_WEBSEARCH_PROVIDER") },
        }
        for v in ["BRAVE_API_KEY", "TAVILY_API_KEY", "EXA_API_KEY"] {
            unsafe { std::env::remove_var(v) };
        }
        if let (Some(k), Some(val)) = (api_key_env, api_key_value) {
            unsafe { std::env::set_var(k, val) };
        }
    }

    // ----------------------------------------------------------------------
    // Pure parser tests
    // ----------------------------------------------------------------------

    #[test]
    fn brave_parser_extracts_results() {
        let body = json!({
            "web": {
                "results": [
                    {
                        "title": "Rust homepage",
                        "url": "https://rust-lang.org/",
                        "description": "Empowering everyone to build reliable software"
                    },
                    {
                        "title": "docs.rs",
                        "url": "https://docs.rs/",
                        "description": "Docs"
                    }
                ]
            }
        })
        .to_string();
        let hits = parse_brave_response(&body).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust homepage");
        assert_eq!(hits[0].url, "https://rust-lang.org/");
        assert!(hits[0].snippet.contains("Empowering"));
    }

    #[test]
    fn tavily_parser_extracts_results() {
        let body = json!({
            "results": [
                {
                    "title": "Tavily Result",
                    "url": "https://example.com/t",
                    "content": "Hello from tavily"
                }
            ]
        })
        .to_string();
        let hits = parse_tavily_response(&body).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Tavily Result");
        assert_eq!(hits[0].url, "https://example.com/t");
        assert_eq!(hits[0].snippet, "Hello from tavily");
    }

    #[test]
    fn exa_parser_extracts_results() {
        let body = json!({
            "results": [
                {
                    "title": "Exa Result",
                    "url": "https://example.com/e",
                    "text": "Exa content"
                }
            ]
        })
        .to_string();
        let hits = parse_exa_response(&body).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.com/e");
        assert_eq!(hits[0].snippet, "Exa content");
    }

    #[test]
    fn selected_provider_defaults_to_brave_without_env() {
        let _g = ENV_LOCK.blocking_lock();
        set_env(None, None, None);
        assert_eq!(selected_provider(), Provider::Brave);
    }

    #[test]
    fn selected_provider_reads_env() {
        let _g = ENV_LOCK.blocking_lock();
        set_env(Some("tavily"), None, None);
        assert_eq!(selected_provider(), Provider::Tavily);
        set_env(Some("exa"), None, None);
        assert_eq!(selected_provider(), Provider::Exa);
        set_env(None, None, None);
    }

    // ----------------------------------------------------------------------
    // Invoke tests (wiremock-backed)
    // ----------------------------------------------------------------------

    #[tokio::test]
    async fn missing_api_key_returns_structured_error() {
        let _g = ENV_LOCK.lock().await;
        set_env(Some("brave"), None, None);
        let tool = WebSearchTool::new(reqwest::Client::new());
        let err = tool
            .invoke(json!({"query": "anything"}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)), "got: {err:?}");
        let msg = format!("{err}");
        assert!(msg.contains("BRAVE_API_KEY"), "msg: {msg}");
    }

    #[tokio::test]
    async fn brave_happy_path_returns_formatted_results() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/res/v1/web/search"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                json!({
                    "web": {
                        "results": [
                            { "title": "T1", "url": "https://t1.example/", "description": "snip1" },
                            { "title": "T2", "url": "https://t2.example/", "description": "snip2" }
                        ]
                    }
                })
                .to_string()
                .into_bytes(),
                "application/json",
            ))
            .mount(&server)
            .await;

        let _g = ENV_LOCK.lock().await;
        set_env(Some("brave"), Some("BRAVE_API_KEY"), Some("abc"));

        let endpoint = format!("{}/res/v1/web/search", server.uri());
        let tool = WebSearchTool::new(reqwest::Client::new()).with_brave_endpoint(endpoint);
        let out = tool
            .invoke(json!({"query": "rust async"}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text")
        };
        assert!(t.text.contains("T1"), "text: {}", t.text);
        assert!(t.text.contains("https://t1.example/"), "text: {}", t.text);
        assert!(t.text.contains("snip1"), "text: {}", t.text);
        assert!(t.text.contains("2 results"), "text: {}", t.text);
        set_env(None, None, None);
    }

    #[tokio::test]
    async fn tavily_happy_path_returns_formatted_results() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(
                    json!({
                        "results": [
                            { "title": "Tav", "url": "https://tav.example/", "content": "tav body" }
                        ]
                    })
                    .to_string()
                    .into_bytes(),
                    "application/json",
                ),
            )
            .mount(&server)
            .await;

        let _g = ENV_LOCK.lock().await;
        set_env(Some("tavily"), Some("TAVILY_API_KEY"), Some("xyz"));

        let endpoint = format!("{}/search", server.uri());
        let tool = WebSearchTool::new(reqwest::Client::new()).with_tavily_endpoint(endpoint);
        let out = tool.invoke(json!({"query": "hi"}), ctx()).await.unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text")
        };
        assert!(t.text.contains("Tav"), "text: {}", t.text);
        assert!(t.text.contains("via tavily"), "text: {}", t.text);
        set_env(None, None, None);
    }

    #[tokio::test]
    async fn exa_happy_path_returns_formatted_results() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(
                    json!({
                        "results": [
                            { "title": "ExaT", "url": "https://exa.example/", "text": "exa body" }
                        ]
                    })
                    .to_string()
                    .into_bytes(),
                    "application/json",
                ),
            )
            .mount(&server)
            .await;

        let _g = ENV_LOCK.lock().await;
        set_env(Some("exa"), Some("EXA_API_KEY"), Some("k"));

        let endpoint = format!("{}/search", server.uri());
        let tool = WebSearchTool::new(reqwest::Client::new()).with_exa_endpoint(endpoint);
        let out = tool.invoke(json!({"query": "hi"}), ctx()).await.unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text")
        };
        assert!(t.text.contains("ExaT"), "text: {}", t.text);
        assert!(t.text.contains("via exa"), "text: {}", t.text);
        set_env(None, None, None);
    }

    #[tokio::test]
    async fn http_error_status_returns_tool_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(401).set_body_raw(b"unauthorized".to_vec(), "text/plain"),
            )
            .mount(&server)
            .await;

        let _g = ENV_LOCK.lock().await;
        set_env(Some("brave"), Some("BRAVE_API_KEY"), Some("badkey"));

        let endpoint = format!("{}/res/v1/web/search", server.uri());
        let tool = WebSearchTool::new(reqwest::Client::new()).with_brave_endpoint(endpoint);
        let err = tool.invoke(json!({"query": "x"}), ctx()).await.unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
        let msg = format!("{err}");
        assert!(msg.contains("401"), "msg: {msg}");
        set_env(None, None, None);
    }

    #[tokio::test]
    async fn max_results_clamped_to_20() {
        // The clamp is internal; we just verify max_results=99 doesn't error
        // and that the tool succeeds.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                json!({ "web": { "results": [] } }).to_string().into_bytes(),
                "application/json",
            ))
            .mount(&server)
            .await;

        let _g = ENV_LOCK.lock().await;
        set_env(Some("brave"), Some("BRAVE_API_KEY"), Some("k"));

        let endpoint = format!("{}/res/v1/web/search", server.uri());
        let tool = WebSearchTool::new(reqwest::Client::new()).with_brave_endpoint(endpoint);
        let out = tool
            .invoke(json!({"query": "q", "max_results": 99}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text")
        };
        assert!(t.text.contains("0 results"), "text: {}", t.text);
        set_env(None, None, None);
    }

    #[tokio::test]
    async fn empty_query_rejected_as_invalid_input() {
        let _g = ENV_LOCK.lock().await;
        set_env(Some("brave"), Some("BRAVE_API_KEY"), Some("k"));
        let tool = WebSearchTool::new(reqwest::Client::new());
        let err = tool
            .invoke(json!({"query": "   "}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)), "got: {err:?}");
        set_env(None, None, None);
    }
}
