//! `WebFetch` tool — GET an http(s):// URL, convert the body to text for the
//! agent, and optionally summarize via a secondary provider.
//!
//! See `docs/superpowers/specs/2026-05-23-web-fetch-design.md` for the full
//! design. Highlights:
//!
//! * GET only; http upgraded to https for domain-named hosts (IP literals and
//!   `localhost` are preserved so homelab and test setups work).
//! * Manual redirect handling: same-host (with optional `www.` strip) follows
//!   up to 10 hops; cross-host returns a notice for the model to act on.
//! * 10 MB body cap, 60s default timeout (1–300s configurable).
//! * HTML → Markdown via `htmd`; text/markdown/text/plain/json passthrough;
//!   binary content returns a notice with size + content-type.
//! * Optional summarizer wired via `with_summarizer`.

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{CompletionRequest, ContentBlock, Provider, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

const MAX_URL_LEN: usize = 2_000;
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 100_000;
const MAX_REDIRECTS: usize = 10;
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 300;
const SUMMARIZER_MAX_TOKENS: u32 = 1024;
const TRUNCATION_FOOTER: &str = "\n\n[content truncated at 100KB]";

/// `WebFetch` tool — fetches URLs and returns markdown/text for the agent.
pub struct WebFetchTool {
    client: reqwest::Client,
    summarizer: Option<Summarizer>,
    schema: OnceLock<Value>,
}

#[derive(Clone)]
struct Summarizer {
    provider: Arc<dyn Provider + Send + Sync>,
    model: String,
}

impl std::fmt::Debug for WebFetchTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `client` impls Debug; `summarizer` holds an `Arc<dyn Provider>` which
        // does not, so we report a boolean instead.
        f.debug_struct("WebFetchTool")
            .field("client", &self.client)
            .field("summarizer_wired", &self.summarizer.is_some())
            .finish_non_exhaustive()
    }
}

impl WebFetchTool {
    /// Build a [`WebFetchTool`] from a shared `reqwest::Client`.
    ///
    /// The client should be configured with `redirect(Policy::none())` —
    /// [`WebFetchTool`] follows redirects manually so it can apply same-host
    /// policy. If your client follows redirects automatically, redirect tests
    /// will pass but cross-host policy will be silently bypassed.
    #[must_use]
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            summarizer: None,
            schema: OnceLock::new(),
        }
    }

    /// Wire an optional secondary-model summarizer.
    ///
    /// When the model passes a `prompt` field to `WebFetch`, the fetched
    /// markdown is routed through this provider + model. Picking a small,
    /// fast model (e.g. Claude Haiku, Gemini Flash) is recommended because
    /// every `WebFetch` call with a `prompt` will route through it.
    #[must_use]
    pub fn with_summarizer(
        mut self,
        provider: Arc<dyn Provider + Send + Sync>,
        model: impl Into<String>,
    ) -> Self {
        self.summarizer = Some(Summarizer {
            provider,
            model: model.into(),
        });
        self
    }
}

// ---------------------------------------------------------------------------
// Helpers (pure functions, no I/O) — exercised by the test module directly.
// ---------------------------------------------------------------------------

/// Parse + validate a URL string for the `WebFetch` tool.
fn validate_url(input: &str) -> Result<Url, &'static str> {
    if input.len() > MAX_URL_LEN {
        return Err("URL is longer than 2000 characters");
    }
    let parsed = Url::parse(input).map_err(|_| "URL could not be parsed")?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("URL scheme must be http or https"),
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("URL must not contain a username or password");
    }
    let host = parsed.host_str().ok_or("URL has no host")?;
    if !host.contains('.') {
        return Err("URL host must contain a dot");
    }
    Ok(parsed)
}

/// Upgrade `http://` to `https://` for non-private hosts.
///
/// IP literals and `localhost` are left as http to support homelab and local
/// testing setups (the canonical case is a mock HTTP server bound to
/// `127.0.0.1`). Domain-named hosts get upgraded to defend against accidental
/// cleartext on the public internet.
///
/// `Url::set_scheme` returns `Err` if the change crosses the special/non-
/// special boundary; http↔https is in-bounds, so the result is ignored.
fn upgrade_scheme(mut url: Url) -> Url {
    if url.scheme() != "http" {
        return url;
    }
    let preserve_http = match url.host() {
        Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => true,
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if !preserve_http {
        let _ = url.set_scheme("https");
    }
    url
}

/// True iff `a` and `b` share a host once a leading `www.` is stripped from
/// either side. Case-insensitive on the host portion.
fn same_host_ignoring_www(a: &Url, b: &Url) -> bool {
    fn strip(h: &str) -> &str {
        h.strip_prefix("www.").unwrap_or(h)
    }
    match (a.host_str(), b.host_str()) {
        (Some(ha), Some(hb)) => strip(ha).eq_ignore_ascii_case(strip(hb)),
        _ => false,
    }
}

/// Convert an HTML document to Markdown, stripping `<script>`, `<style>`,
/// `<noscript>`, and `<svg>` content before conversion.
///
/// The `htmd::HtmlToMarkdown` converter is cached in a `OnceLock` — building
/// it allocates a per-tag handler table that we'd otherwise rebuild on every
/// fetch. The converter is `Send + Sync` (it wraps stateless html5ever rules).
fn html_to_markdown(html: &str) -> Result<String, ToolError> {
    static CONVERTER: OnceLock<htmd::HtmlToMarkdown> = OnceLock::new();
    let converter = CONVERTER.get_or_init(|| {
        htmd::HtmlToMarkdown::builder()
            .skip_tags(vec!["script", "style", "noscript", "svg"])
            .build()
    });
    converter
        .convert(html)
        .map_err(|e| ToolError::execution(std::io::Error::other(format!("html→md: {e}"))))
}

/// Truncate text to `MAX_TEXT_CHARS` characters (not bytes), appending a
/// footer if truncation occurred.
fn truncate_text(s: String) -> String {
    if s.chars().count() <= MAX_TEXT_CHARS {
        return s;
    }
    let mut out: String = s.chars().take(MAX_TEXT_CHARS).collect();
    out.push_str(TRUNCATION_FOOTER);
    out
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct FetchInput {
    url: String,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

/// Classification of a Content-Type header for body-rendering purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyKind {
    /// HTML → run through htmd.
    Html,
    /// text/markdown, text/plain, text/*, application/json, application/*+json
    /// → UTF-8 decode lossy and pass through.
    Text,
    /// Anything else → emit a notice; do not include the body.
    Binary,
}

/// Outcome of a fetch loop: a fully fetched body, or a cross-host redirect.
#[derive(Debug)]
enum FetchOutcome {
    Body {
        final_url: Url,
        status: u16,
        reason: String,
        content_type: String,
        body: Vec<u8>,
    },
    CrossHostRedirect {
        from: Url,
        to: Url,
        status: u16,
        reason: String,
    },
}

/// Read a `reqwest::Response` body into a `Vec<u8>`, aborting if the total
/// exceeds [`MAX_BODY_BYTES`].
async fn read_body_capped(mut resp: reqwest::Response) -> Result<Vec<u8>, ToolError> {
    let mut acc: Vec<u8> = Vec::with_capacity(8 * 1024);
    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => return Err(ToolError::execution(e)),
        };
        if acc.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            return Err(ToolError::execution(std::io::Error::other(format!(
                "response body exceeds {MAX_BODY_BYTES}-byte cap"
            ))));
        }
        acc.extend_from_slice(&chunk);
    }
    Ok(acc)
}

/// Reason-phrase fallback for a status code when the server doesn't provide
/// one (HTTP/2 in particular drops reason phrases).
fn reason_for(status: reqwest::StatusCode) -> String {
    status.canonical_reason().unwrap_or("").to_string()
}

/// Perform a GET with manual redirect handling.
///
/// Returns `Body` on a 2xx (after upgrading scheme and following same-host
/// redirects), or `CrossHostRedirect` if a 3xx points to a different host.
/// Non-2xx, non-3xx statuses propagate the body up so callers can include it
/// in error messages.
async fn fetch_with_redirects(
    client: &reqwest::Client,
    initial: Url,
) -> Result<FetchOutcome, ToolError> {
    let mut current = upgrade_scheme(initial);
    for _ in 0..MAX_REDIRECTS {
        let resp = client
            .get(current.clone())
            .send()
            .await
            .map_err(ToolError::execution)?;
        let status = resp.status();
        let reason = reason_for(status);

        if status.is_redirection() {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    ToolError::execution(std::io::Error::other(format!(
                        "{status} redirect with no Location header"
                    )))
                })?;
            let next = current.join(loc).map_err(|e| {
                ToolError::execution(std::io::Error::other(format!(
                    "invalid Location header {loc:?}: {e}"
                )))
            })?;
            if same_host_ignoring_www(&current, &next) {
                current = upgrade_scheme(next);
                continue;
            }
            return Ok(FetchOutcome::CrossHostRedirect {
                from: current,
                to: next,
                status: status.as_u16(),
                reason,
            });
        }

        // Non-redirect: read body (bounded) and return.
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let final_url = current;
        let status_u16 = status.as_u16();
        let body = read_body_capped(resp).await?;
        return Ok(FetchOutcome::Body {
            final_url,
            status: status_u16,
            reason,
            content_type,
            body,
        });
    }
    Err(ToolError::execution(std::io::Error::other(format!(
        "exceeded {MAX_REDIRECTS}-hop redirect limit"
    ))))
}

/// Run the optional secondary-model summarizer.
async fn run_summarizer(
    summarizer: &Summarizer,
    final_url: &Url,
    body_markdown: &str,
    prompt: &str,
) -> Result<String, ToolError> {
    let user_text = format!("URL: {final_url}\n\n---\n{body_markdown}\n---\n\nQuestion: {prompt}");
    let req = CompletionRequest::builder(summarizer.model.clone())
        .system("You answer questions about the provided web content. Be concise. Quote sparingly.")
        .user_text(user_text)
        .max_tokens(SUMMARIZER_MAX_TOKENS)
        .build()
        .map_err(|e| ToolError::execution(std::io::Error::other(format!("{e}"))))?;
    let resp = summarizer
        .provider
        .complete(req)
        .await
        .map_err(|e| ToolError::execution(std::io::Error::other(format!("{e}"))))?;
    let text = resp
        .message
        .content
        .into_iter()
        .find_map(|b| match b {
            ContentBlock::Text(t) => Some(t.text),
            _ => None,
        })
        .unwrap_or_default();
    Ok(text)
}

/// Render the response body to text based on its content type.
async fn render_body(content_type: &str, body: Vec<u8>) -> Result<String, ToolError> {
    match classify_content_type(content_type) {
        BodyKind::Html => {
            let html = String::from_utf8_lossy(&body).into_owned();
            // htmd is sync + html5ever-backed. Run in spawn_blocking so a
            // pathological page can't stall the tokio worker.
            let md = tokio::task::spawn_blocking(move || html_to_markdown(&html))
                .await
                .map_err(|e| ToolError::execution(std::io::Error::other(format!("{e}"))))??;
            Ok(truncate_text(md))
        }
        BodyKind::Text => {
            let text = String::from_utf8_lossy(&body).into_owned();
            Ok(truncate_text(text))
        }
        BodyKind::Binary => Ok(format!(
            "Binary content ({content_type}, {} bytes); not converted. \
             Use Bash + curl + a parser if you need its contents.",
            body.len(),
        )),
    }
}

/// Classify a Content-Type header value.
fn classify_content_type(ct: &str) -> BodyKind {
    let ct = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if ct.starts_with("text/html") {
        BodyKind::Html
    } else if ct.starts_with("text/")
        || ct == "application/json"
        || (ct.starts_with("application/") && ct.ends_with("+json"))
    {
        BodyKind::Text
    } else {
        BodyKind::Binary
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "WebFetch"
    }

    fn description(&self) -> &'static str {
        "Fetch an http(s):// URL via GET and return its body as text. HTML is converted to Markdown; text/plain and JSON pass through; binary content returns a short notice. Manual redirect handling: same-host redirects (≤10 hops) are followed; cross-host redirects are surfaced to the caller. Bounded at 10MB body and 60s default timeout (1–300s configurable). Authenticated services are not supported — use a dedicated MCP tool. If an optional `prompt` is set and a summarizer is wired, the body is summarized against that prompt; otherwise the body is returned verbatim. NOTE: requests to localhost and private IPs are allowed for homelab use; the operator's host network is reachable."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute http(s):// URL to fetch"
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional. If set AND a summarizer is wired, the fetched content is summarized against this prompt; otherwise it is ignored."
                },
                "timeout_seconds": {
                    "type": "integer",
                    "description": "Per-request timeout (default 60).",
                    "minimum": 1,
                    "maximum": 300
                }
            },
            "required": ["url"]
        }))
    }

    async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: FetchInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;
        let initial = validate_url(&parsed.url).map_err(ToolError::invalid_input)?;
        let timeout = Duration::from_secs(
            parsed
                .timeout_seconds
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .clamp(1, MAX_TIMEOUT_SECS),
        );

        let start = std::time::Instant::now();

        let fetch_fut = fetch_with_redirects(&self.client, initial);
        let outcome = tokio::select! {
            () = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
            () = tokio::time::sleep(timeout) => {
                return Err(ToolError::execution(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("WebFetch timed out after {}s", timeout.as_secs()),
                )));
            }
            r = fetch_fut => r?,
        };

        match outcome {
            FetchOutcome::CrossHostRedirect {
                from,
                to,
                status,
                reason,
            } => {
                let ms = start.elapsed().as_millis();
                let header = format!(
                    "→ WebFetch {from} (0 bytes, n/a, {ms} ms)\n→ Status: {status} {reason}\n\n"
                );
                let notice = format!(
                    "REDIRECT DETECTED: {from} → {to} ({status} {reason})\n\
                     The redirect crosses hosts. To proceed, call WebFetch again with url=\"{to}\"."
                );
                Ok(vec![ContentBlock::Text(TextBlock {
                    text: header + &notice,
                    cache_control: None,
                })])
            }
            FetchOutcome::Body {
                final_url,
                status,
                reason,
                content_type,
                body,
            } => {
                let bytes_len = body.len();

                // 4xx / 5xx → error, but keep the rendered body in the message
                // so the model can see what failed.
                let is_success = (200..300).contains(&status);
                let body_text = render_body(&content_type, body).await?;

                // Optional summarization: only on 2xx with a prompt + summarizer
                // (don't try to summarize an error page).
                let final_body = if is_success {
                    self.maybe_summarize_or_note(
                        &final_url,
                        &body_text,
                        parsed.prompt.as_deref(),
                        &cx,
                    )
                    .await
                } else {
                    body_text
                };

                let ms = start.elapsed().as_millis();
                let ct_for_header = if content_type.is_empty() {
                    "(none)".to_string()
                } else {
                    content_type.clone()
                };
                let text = format!(
                    "→ WebFetch {final_url} ({bytes_len} bytes, {ct_for_header}, {ms} ms)\n\
                     → Status: {status} {reason}\n\n{final_body}"
                );

                if !is_success {
                    return Err(ToolError::execution(std::io::Error::other(text)));
                }
                Ok(vec![ContentBlock::Text(TextBlock {
                    text,
                    cache_control: None,
                })])
            }
        }
    }
}

impl WebFetchTool {
    /// Run the configured summarizer (if any) or return the raw body. When a
    /// `prompt` is set but no summarizer is wired, the raw body is returned
    /// with a footer note. On summarizer error the raw body is returned with
    /// a `[summarization failed: …]` footer — the model still has the content.
    async fn maybe_summarize_or_note(
        &self,
        final_url: &Url,
        body_text: &str,
        prompt: Option<&str>,
        cx: &ToolContext,
    ) -> String {
        let Some(prompt) = prompt else {
            return body_text.to_string();
        };
        let Some(summarizer) = &self.summarizer else {
            return format!(
                "{body_text}\n\n[note: prompt provided but no summarizer configured; ignoring]"
            );
        };
        let summarize = run_summarizer(summarizer, final_url, body_text, prompt);
        tokio::select! {
            () = cx.cancel.cancelled() => format!(
                "{body_text}\n\n[summarization cancelled]"
            ),
            res = summarize => match res {
                Ok(s) => s,
                Err(e) => format!("{body_text}\n\n[summarization failed: {e}]"),
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    // ----------------------------------------------------------------------
    // validate_url
    // ----------------------------------------------------------------------

    #[test]
    fn validate_accepts_https() {
        assert!(validate_url("https://example.com/").is_ok());
    }

    #[test]
    fn validate_accepts_http() {
        assert!(validate_url("http://example.com/path?q=1").is_ok());
    }

    #[test]
    fn validate_rejects_file_scheme() {
        assert!(validate_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn validate_rejects_ftp_scheme() {
        assert!(validate_url("ftp://example.com/x").is_err());
    }

    #[test]
    fn validate_rejects_userinfo() {
        assert!(validate_url("https://user:pass@example.com/").is_err());
        assert!(validate_url("https://user@example.com/").is_err());
    }

    #[test]
    fn validate_rejects_bare_hostname() {
        assert!(validate_url("http://intranet").is_err());
    }

    #[test]
    fn validate_accepts_dotted_internal_host() {
        // Homelab use case: dotted internal hostname is allowed.
        assert!(validate_url("http://nas.lan/").is_ok());
    }

    #[test]
    fn validate_rejects_oversized_url() {
        let big = format!("https://example.com/{}", "a".repeat(2100));
        assert!(big.len() > MAX_URL_LEN);
        assert!(validate_url(&big).is_err());
    }

    #[test]
    fn validate_rejects_garbage() {
        assert!(validate_url("not a url").is_err());
    }

    // ----------------------------------------------------------------------
    // upgrade_scheme
    // ----------------------------------------------------------------------

    #[test]
    fn upgrade_changes_http_to_https() {
        let u = upgrade_scheme(Url::parse("http://example.com/x").unwrap());
        assert_eq!(u.scheme(), "https");
        assert_eq!(u.host_str(), Some("example.com"));
        assert_eq!(u.path(), "/x");
    }

    #[test]
    fn upgrade_leaves_https_unchanged() {
        let u = upgrade_scheme(Url::parse("https://example.com/").unwrap());
        assert_eq!(u.scheme(), "https");
    }

    #[test]
    fn upgrade_preserves_http_for_ipv4_literal() {
        let u = upgrade_scheme(Url::parse("http://127.0.0.1:8080/x").unwrap());
        assert_eq!(u.scheme(), "http");
    }

    #[test]
    fn upgrade_preserves_http_for_ipv6_literal() {
        let u = upgrade_scheme(Url::parse("http://[::1]:8080/x").unwrap());
        assert_eq!(u.scheme(), "http");
    }

    #[test]
    fn upgrade_preserves_http_for_localhost_hostname() {
        let u = upgrade_scheme(Url::parse("http://localhost:8080/x").unwrap());
        assert_eq!(u.scheme(), "http");
    }

    // ----------------------------------------------------------------------
    // same_host_ignoring_www
    // ----------------------------------------------------------------------

    #[test]
    fn same_host_strips_www_on_either_side() {
        let a = Url::parse("https://example.com/").unwrap();
        let b = Url::parse("https://www.example.com/").unwrap();
        assert!(same_host_ignoring_www(&a, &b));
        assert!(same_host_ignoring_www(&b, &a));
    }

    #[test]
    fn same_host_is_case_insensitive() {
        let a = Url::parse("https://Example.COM/").unwrap();
        let b = Url::parse("https://example.com/").unwrap();
        assert!(same_host_ignoring_www(&a, &b));
    }

    #[test]
    fn different_hosts_are_not_same() {
        let a = Url::parse("https://example.com/").unwrap();
        let b = Url::parse("https://other.com/").unwrap();
        assert!(!same_host_ignoring_www(&a, &b));
    }

    // ----------------------------------------------------------------------
    // truncate_text
    // ----------------------------------------------------------------------

    #[test]
    fn truncate_leaves_short_text_alone() {
        let s = "hello world".to_string();
        assert_eq!(truncate_text(s.clone()), s);
    }

    #[test]
    fn truncate_cuts_long_text_and_appends_footer() {
        let s = "x".repeat(MAX_TEXT_CHARS + 10);
        let out = truncate_text(s);
        assert!(out.ends_with(TRUNCATION_FOOTER));
        // The body before the footer is exactly MAX_TEXT_CHARS characters.
        let body = out.strip_suffix(TRUNCATION_FOOTER).unwrap();
        assert_eq!(body.chars().count(), MAX_TEXT_CHARS);
    }

    // ----------------------------------------------------------------------
    // html_to_markdown
    // ----------------------------------------------------------------------

    #[test]
    fn html_converts_h1_to_hash() {
        let md = html_to_markdown("<h1>Hello</h1>").unwrap();
        assert!(md.contains("# Hello"), "got: {md:?}");
    }

    #[test]
    fn html_strips_script_content() {
        let md =
            html_to_markdown("<html><body><p>visible</p><script>alert(1)</script></body></html>")
                .unwrap();
        assert!(md.contains("visible"), "got: {md:?}");
        assert!(!md.contains("alert(1)"), "script body leaked: {md:?}");
    }

    #[test]
    fn html_strips_style_content() {
        let md = html_to_markdown(
            "<html><head><style>.x{color:red}</style></head><body><p>hi</p></body></html>",
        )
        .unwrap();
        assert!(md.contains("hi"), "got: {md:?}");
        assert!(!md.contains("color:red"), "style body leaked: {md:?}");
    }

    #[test]
    fn html_preserves_paragraphs() {
        let md = html_to_markdown("<p>one</p><p>two</p>").unwrap();
        assert!(md.contains("one"));
        assert!(md.contains("two"));
    }

    // ----------------------------------------------------------------------
    // classify_content_type
    // ----------------------------------------------------------------------

    #[test]
    fn classify_html() {
        assert_eq!(classify_content_type("text/html"), BodyKind::Html);
        assert_eq!(
            classify_content_type("text/html; charset=utf-8"),
            BodyKind::Html
        );
    }

    #[test]
    fn classify_text_and_json_as_passthrough() {
        assert_eq!(classify_content_type("text/plain"), BodyKind::Text);
        assert_eq!(classify_content_type("text/markdown"), BodyKind::Text);
        assert_eq!(classify_content_type("application/json"), BodyKind::Text);
        assert_eq!(
            classify_content_type("application/vnd.api+json"),
            BodyKind::Text
        );
    }

    #[test]
    fn classify_binary() {
        assert_eq!(classify_content_type("application/pdf"), BodyKind::Binary);
        assert_eq!(classify_content_type("image/png"), BodyKind::Binary);
        assert_eq!(
            classify_content_type("application/octet-stream"),
            BodyKind::Binary
        );
        assert_eq!(classify_content_type(""), BodyKind::Binary);
    }

    // ----------------------------------------------------------------------
    // Tool::invoke integration tests (wiremock-backed)
    // ----------------------------------------------------------------------

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

    /// Build a tool with a `reqwest::Client` configured for manual redirects.
    fn tool_with_client() -> WebFetchTool {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client builds");
        WebFetchTool::new(client)
    }

    fn url_of(m: &MockServer, p: &str) -> String {
        format!("{}{}", m.uri(), p)
    }

    fn text_of(blocks: &[ContentBlock]) -> &str {
        match blocks.first().expect("at least one block") {
            ContentBlock::Text(t) => t.text.as_str(),
            _ => panic!("expected text block"),
        }
    }

    /// Mount a single GET / handler that returns `body` with content-type `ct`.
    async fn mock_body(ct: &str, body: impl Into<Vec<u8>>) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body.into(), ct))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn fetches_text_html_and_converts_to_markdown() {
        let server = mock_body("text/html; charset=utf-8", "<h1>Hello</h1><p>World</p>").await;
        let out = tool_with_client()
            .invoke(json!({"url": url_of(&server, "/")}), ctx())
            .await
            .unwrap();
        let body = text_of(&out);
        assert!(body.contains("# Hello"), "body: {body}");
        assert!(body.contains("World"), "body: {body}");
        assert!(body.contains("Status: 200"), "body: {body}");
    }

    #[tokio::test]
    async fn fetches_text_plain_passthrough() {
        let server = mock_body("text/plain", "raw text body").await;
        let out = tool_with_client()
            .invoke(json!({"url": url_of(&server, "/")}), ctx())
            .await
            .unwrap();
        assert!(text_of(&out).contains("raw text body"));
    }

    #[tokio::test]
    async fn fetches_json_passthrough() {
        let server = mock_body("application/json", r#"{"hello":"world"}"#).await;
        let out = tool_with_client()
            .invoke(json!({"url": url_of(&server, "/")}), ctx())
            .await
            .unwrap();
        assert!(text_of(&out).contains(r#"{"hello":"world"}"#));
    }

    #[tokio::test]
    async fn binary_content_returns_notice() {
        let server = mock_body("application/pdf", vec![0x25, 0x50, 0x44, 0x46]).await;
        let out = tool_with_client()
            .invoke(json!({"url": url_of(&server, "/")}), ctx())
            .await
            .unwrap();
        let body = text_of(&out);
        assert!(body.contains("Binary content"), "body: {body}");
        assert!(body.contains("application/pdf"), "body: {body}");
        assert!(body.contains("Status: 200"), "body: {body}");
    }

    #[tokio::test]
    async fn non_2xx_returns_tool_error_with_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(404).set_body_raw(b"page missing".to_vec(), "text/plain"),
            )
            .mount(&server)
            .await;

        let err = tool_with_client()
            .invoke(json!({"url": url_of(&server, "/")}), ctx())
            .await
            .unwrap_err();
        let s = format!("{err}");
        assert!(
            matches!(err, ToolError::Execution(_)),
            "wrong variant: {err:?}"
        );
        assert!(s.contains("Status: 404"), "msg: {s}");
    }

    // ----------------------------------------------------------------------
    // Edge cases: redirects, limits, cancel
    // ----------------------------------------------------------------------

    #[tokio::test]
    async fn truncates_oversized_markdown() {
        // 200KB of plain text → after pass-through, must be truncated to 100KB
        // plus the footer.
        let big = "a".repeat(200_000);
        let server = mock_body("text/plain", big).await;
        let out = tool_with_client()
            .invoke(json!({"url": url_of(&server, "/")}), ctx())
            .await
            .unwrap();
        let body = text_of(&out);
        assert!(
            body.ends_with(TRUNCATION_FOOTER),
            "tail: {:?}",
            &body[body.len().saturating_sub(60)..]
        );
    }

    #[tokio::test]
    async fn rejects_oversized_response_body() {
        // 11MB body must exceed the 10MB cap. We can't realistically generate
        // 11MB here without it being slow, but it's fine: 10MB + 1 byte is
        // enough.
        let oversize = vec![b'x'; MAX_BODY_BYTES + 1];
        let server = mock_body("text/plain", oversize).await;
        let err = tool_with_client()
            .invoke(json!({"url": url_of(&server, "/")}), ctx())
            .await
            .unwrap_err();
        let s = format!("{err}");
        assert!(matches!(err, ToolError::Execution(_)));
        assert!(
            s.to_lowercase().contains("byte cap") || s.contains("10485760"),
            "msg: {s}"
        );
    }

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let err = tool_with_client()
            .invoke(json!({"url": "file:///etc/passwd"}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn rejects_url_with_userinfo_invoke() {
        let err = tool_with_client()
            .invoke(json!({"url": "https://user:pass@example.com/"}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn rejects_bare_hostname_invoke() {
        let err = tool_with_client()
            .invoke(json!({"url": "http://intranet"}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn rejects_oversized_url_invoke() {
        let big = format!("https://example.com/{}", "a".repeat(2100));
        let err = tool_with_client()
            .invoke(json!({"url": big}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn follows_same_host_redirect() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/old"))
            .respond_with(ResponseTemplate::new(301).insert_header("location", "/new"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/new"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(b"<h1>landed</h1>".to_vec(), "text/html"),
            )
            .mount(&server)
            .await;

        let out = tool_with_client()
            .invoke(json!({"url": url_of(&server, "/old")}), ctx())
            .await
            .unwrap();
        let body = text_of(&out);
        assert!(body.contains("# landed"), "body: {body}");
        // Final URL in the header should be /new.
        assert!(body.contains("/new"), "body: {body}");
    }

    #[tokio::test]
    async fn surfaces_cross_host_redirect() {
        // Redirect to a host that *won't* match — we use a different port on
        // 127.0.0.1, but importantly the host string is "other.example" which
        // resolution-wise won't be hit because we never follow it.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(301)
                    .insert_header("location", "https://other.example/elsewhere"),
            )
            .mount(&server)
            .await;
        let out = tool_with_client()
            .invoke(json!({"url": url_of(&server, "/")}), ctx())
            .await
            .unwrap();
        let body = text_of(&out);
        assert!(body.contains("REDIRECT DETECTED"), "body: {body}");
        assert!(body.contains("other.example/elsewhere"), "body: {body}");
        // We did not follow it, so no "landed" body should appear.
        assert!(!body.contains("landed"), "body: {body}");
    }

    #[tokio::test]
    async fn caps_redirect_hops() {
        // Each /hop/N redirects to /hop/(N+1) on the same host, forever.
        let server = MockServer::start().await;
        for i in 0..(MAX_REDIRECTS + 2) {
            let from = format!("/hop/{i}");
            let to = format!("/hop/{}", i + 1);
            Mock::given(method("GET"))
                .and(path(from))
                .respond_with(ResponseTemplate::new(301).insert_header("location", to.as_str()))
                .mount(&server)
                .await;
        }
        let err = tool_with_client()
            .invoke(json!({"url": url_of(&server, "/hop/0")}), ctx())
            .await
            .unwrap_err();
        let s = format!("{err}");
        assert!(matches!(err, ToolError::Execution(_)));
        assert!(
            s.contains("redirect limit") || s.contains("hop"),
            "msg: {s}"
        );
    }

    #[tokio::test]
    async fn timeout_aborts_request() {
        use std::time::Duration as StdDur;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(StdDur::from_secs(5))
                    .set_body_raw(b"slow".to_vec(), "text/plain"),
            )
            .mount(&server)
            .await;

        let start = std::time::Instant::now();
        let err = tool_with_client()
            .invoke(
                json!({"url": url_of(&server, "/"), "timeout_seconds": 1}),
                ctx(),
            )
            .await
            .unwrap_err();
        assert!(
            start.elapsed().as_secs() < 3,
            "took too long: {:?}",
            start.elapsed()
        );
        let s = format!("{err}").to_lowercase();
        assert!(s.contains("timed out") || s.contains("timeout"), "msg: {s}");
    }

    #[tokio::test]
    async fn cancellation_aborts_request() {
        use std::time::Duration as StdDur;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(StdDur::from_secs(30))
                    .set_body_raw(b"slow".to_vec(), "text/plain"),
            )
            .mount(&server)
            .await;

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(StdDur::from_millis(100)).await;
            cancel_clone.cancel();
        });
        let cx = ToolContext {
            tool_use_id: "t1".into(),
            cancel,
            hooks: None,
            turn_index: 0,
        };
        let start = std::time::Instant::now();
        let err = tool_with_client()
            .invoke(json!({"url": url_of(&server, "/")}), cx)
            .await
            .unwrap_err();
        assert!(
            start.elapsed().as_millis() < 2000,
            "cancellation took too long: {:?}",
            start.elapsed()
        );
        assert!(matches!(err, ToolError::Cancelled), "got: {err:?}");
    }

    // ----------------------------------------------------------------------
    // Summarizer wiring
    // ----------------------------------------------------------------------

    use caliban_provider::{CompletionResponse, Message, Role, StopReason, Usage};

    fn mk_summarizer_text(text: &str) -> CompletionResponse {
        CompletionResponse {
            id: "test".into(),
            model: "test-model".into(),
            message: Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text(TextBlock {
                    text: text.into(),
                    cache_control: None,
                })],
            },
            stop_reason: StopReason::EndTurn,
            stop_sequence: None,
            usage: Usage::default(),
        }
    }

    #[tokio::test]
    async fn summarizer_used_when_wired_and_prompt_set() {
        let server = mock_body("text/html", "<p>body</p>").await;
        let mock = Arc::new(caliban_provider::mock::MockProvider::new());
        mock.enqueue_complete(Ok(mk_summarizer_text("SUMMARY_ANSWER")));
        let tool = tool_with_client().with_summarizer(mock, "test-model");
        let out = tool
            .invoke(
                json!({"url": url_of(&server, "/"), "prompt": "what does it say?"}),
                ctx(),
            )
            .await
            .unwrap();
        let body = text_of(&out);
        assert!(body.contains("SUMMARY_ANSWER"), "body: {body}");
        // Raw body should NOT appear when summarization succeeds.
        assert!(!body.contains("body\n\n[note"), "body: {body}");
    }

    #[tokio::test]
    async fn summarizer_failure_falls_back_to_raw() {
        let server = mock_body("text/plain", "raw body content").await;
        let mock = Arc::new(caliban_provider::mock::MockProvider::new());
        mock.enqueue_complete(Err(caliban_provider::error::Error::InvalidRequest(
            "boom".to_string(),
        )));
        let tool = tool_with_client().with_summarizer(mock, "test-model");
        let out = tool
            .invoke(
                json!({"url": url_of(&server, "/"), "prompt": "summarize"}),
                ctx(),
            )
            .await
            .unwrap();
        let body = text_of(&out);
        assert!(body.contains("raw body content"), "body: {body}");
        assert!(body.contains("summarization failed"), "body: {body}");
    }

    #[tokio::test]
    async fn prompt_without_summarizer_returns_raw_with_note() {
        let server = mock_body("text/plain", "raw body content").await;
        let out = tool_with_client()
            .invoke(
                json!({"url": url_of(&server, "/"), "prompt": "summarize"}),
                ctx(),
            )
            .await
            .unwrap();
        let body = text_of(&out);
        assert!(body.contains("raw body content"), "body: {body}");
        assert!(body.contains("no summarizer configured"), "body: {body}");
    }
}
