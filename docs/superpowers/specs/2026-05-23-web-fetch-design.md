# WebFetchTool — Design

**Date:** 2026-05-23
**Status:** Approved
**Target branch:** `jf/feat/web-fetch`
**Sub-project of:** caliban Rust agent harness
**Depends on:** `caliban-tools-builtin`, `caliban-provider`

## Goal

Add a `WebFetchTool` to `caliban-tools-builtin` that lets the agent fetch a URL, convert the response to text (HTML→markdown when applicable), and return it as a `ContentBlock::Text`. Optional secondary-model summarization is wired at tool-construction time and triggered when the model passes a `prompt` field.

WebSearchTool is intentionally out of scope. The agent receives URLs from the user (or from upstream tools) and reaches the open web through this single tool.

## Non-goals

- Web search of any kind (deferred — would require a pluggable `WebSearchBackend` trait and external backend choices).
- Built-in domain allow/deny lists. Per-host permissioning is the `Hooks` layer's job (matches how `BashTool` defers to hooks).
- Auth / cookies / POST / PUT / DELETE — read-only GET only. Authenticated services should be reached through MCP tools.
- Persistent caching to disk. In-memory caching is deferred to v2.
- Sandbox isolation / aggressive SSRF mitigation beyond simple URL hygiene. The operator's host has network access; this matches the existing trust model of `BashTool` (which can already curl anything).
- Auto-persistence of binary content to disk (the TS reference's PDF-save behavior). Defer — too coupled to a not-yet-built artifact store.

## API surface

### Input schema

```json
{
  "type": "object",
  "properties": {
    "url": {
      "type": "string",
      "description": "Absolute http(s):// URL to fetch"
    },
    "prompt": {
      "type": "string",
      "description": "Optional. If set AND a summarizer is wired, the fetched content is summarized against this prompt; otherwise it is ignored and the raw markdown is returned with a footer note."
    },
    "timeout_seconds": {
      "type": "integer",
      "minimum": 1,
      "maximum": 300,
      "description": "Per-request timeout (default 60)."
    }
  },
  "required": ["url"]
}
```

### Construction (Rust)

```rust
pub struct WebFetchTool { /* ... */ }

impl WebFetchTool {
    /// Build with a shared reqwest::Client and no summarizer.
    pub fn new(client: reqwest::Client) -> Self;

    /// Builder method: wire an optional secondary-model summarizer.
    /// `summarizer` is any provider; `model` is the model id the
    /// summarizer should use. Picking a small, fast model is recommended
    /// because every WebFetch with a `prompt` will route through it.
    pub fn with_summarizer(
        self,
        summarizer: Arc<dyn Provider + Send + Sync>,
        model: impl Into<String>,
    ) -> Self;
}
```

A shared `reqwest::Client` is injected so callers can reuse one client across the tool registry (matches the pattern in `caliban-provider-anthropic`). Tool name: `"WebFetch"`.

## HTTP behavior

| Aspect              | Choice                                                                                                                                          | Rationale                                                                            |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| Method              | `GET` only                                                                                                                                      | Read-only tool                                                                       |
| URL parsing         | `url::Url`                                                                                                                                      | Already a workspace dep                                                              |
| Scheme              | http / https only; http upgraded to https for domain-named hosts (IP literals and `localhost` preserved so homelab and tests work)               | Matches reference; protects against accidental cleartext on the public internet      |
| Userinfo            | Reject `user:pass@host` URLs                                                                                                                    | No place for creds in a URL handed to an LLM                                         |
| Hostname            | Domain hosts must contain a `.` (reject bare hostnames like `intranet`); IP literals (IPv4/IPv6) and `localhost` allowed                        | Cheap hygiene; intentional intranet fetches use a dotted internal domain; carve-out matches scheme/upgrade logic so homelab + local test rigs work |
| URL length          | Reject if > 2000 chars                                                                                                                          | Matches reference                                                                    |
| Redirects           | `RedirectPolicy::none()` on the client; we follow manually                                                                                      | Need per-hop policy control                                                          |
| Redirect policy     | Up to 10 hops; follow only when hostname matches (with optional `www.` strip)                                                                   | Mirrors reference                                                                    |
| Cross-host redirect | Return a `REDIRECT DETECTED ...` notice and let the model re-call with the new URL                                                              | Mirrors reference; avoids silent host switch                                         |
| Max content size    | 10 MB hard cap (streaming read, abort early)                                                                                                    | Bounds memory; matches reference                                                     |
| Per-request timeout | Default 60 s; model may override 1–300 s via `timeout_seconds`                                                                                  | Bounded latency                                                                      |
| Cancellation        | `tokio::select!` on `cx.cancel.cancelled()` — abort the request                                                                                 | Consistent with `BashTool`                                                           |
| User-Agent          | `caliban/<crate_version> (+https://github.com/johnford2002/caliban)`                                                                            | Polite; future-friendly when the repo is set                                         |
| Auth                | None                                                                                                                                            | Use MCP tools for authenticated services                                             |
| Localhost / private IPs | **Allowed**                                                                                                                                 | Homelab use case is in-scope; documented in tool description as an operator concern  |

## Content handling

1. Stream the response body into a bounded `Vec<u8>` (10 MB cap; if exceeded, abort with a size-limit error rather than letting reqwest buffer indefinitely).
2. Inspect `Content-Type`:
   - `text/html*` → strip `<script>`, `<style>`, `<noscript>`, `<svg>` content, then convert via `htmd`.
   - `text/markdown`, `text/plain`, `application/json`, `application/*+json`, `text/*` (fallback) → UTF-8 decode lossy, return as-is.
   - Anything else (binary) → return a short notice describing content-type and size; do not include the body.
3. Truncate the textual result to 100 000 chars, appending `\n\n[content truncated at 100KB]` if cut.

`htmd` is chosen over `html2md` and `mdka`:

- `html2md` (Kanedias) is GPL-3.0-or-later with a `jni` dep and has documented "100× output blowup" cases on real-world pages.
- `mdka` pulls `tikv-jemallocator` + `rayon` unconditionally.
- `htmd` (letmutex) is Apache-2.0, 442 stars, actively maintained (last release 2026-04-04), html5ever-backed, turndown.js-inspired. Has a `HtmlToMarkdown` builder we can configure to drop tags.

`htmd` is sync; the agent loop runs in tokio. The conversion call is wrapped in `tokio::task::spawn_blocking` so it cannot stall the runtime on pathological pages.

## Summarizer flow

If the input contains `prompt` AND a summarizer was wired via `with_summarizer(...)`:

1. Build a `CompletionRequest` with:
   - `model` = configured summarizer model id
   - `max_tokens` = 1024
   - `system` = `"You answer questions about the provided web content. Be concise. Quote sparingly."`
   - one user message with text `"URL: <final-url>\n\n---\n<markdown body, truncated to 100K>\n---\n\nQuestion: <prompt>"`
2. Call `summarizer.complete(req).await` inside the same `tokio::select!` as the fetch so cancellation kills it too.
3. On success, return the assistant's text as the tool result body (preceded by the standard `→ WebFetch ...` header).
4. On summarizer error, fall back to the raw markdown body with a footer `[summarization failed: <error>]`. Do not fail the tool — the model has the content it needs to retry itself.

If `prompt` is set but no summarizer is wired, return raw markdown with a footer `[note: prompt provided but no summarizer configured; ignoring]`. (Don't error.)

If `prompt` is absent, the summarizer is never called even when configured.

## Output format

Header line mirrors the existing tools' `→ Header` style:

```
→ WebFetch <final-url-after-redirects> (<bytes> bytes, <content-type>, <duration_ms> ms)
→ Status: <code> <reason-phrase>

<markdown body OR summarizer answer OR redirect notice>
```

Cross-host redirect example:

```
→ WebFetch https://example.com/old (0 bytes, n/a, 132 ms)
→ Status: 301 Moved Permanently

REDIRECT DETECTED: https://example.com/old → https://other.com/new (301 Moved Permanently)
The redirect crosses hosts. To proceed, call WebFetch again with url="https://other.com/new".
```

Binary-content example:

```
→ WebFetch https://example.com/report.pdf (2415293 bytes, application/pdf, 412 ms)
→ Status: 200 OK

Binary content (application/pdf, 2415293 bytes); not converted. Use Bash + curl + a parser if you need its contents.
```

## Crate changes

`crates/caliban-tools-builtin/Cargo.toml`:

```toml
[dependencies]
# … existing deps …
reqwest = { workspace = true }
url     = { workspace = true }
bytes   = { workspace = true }
futures = { workspace = true }
htmd    = "0.5"

[dev-dependencies]
# … existing dev-deps …
wiremock = { workspace = true }
```

All deps except `htmd` are already in the workspace.

`crates/caliban-tools-builtin/src/lib.rs` adds `pub mod web_fetch; pub use web_fetch::WebFetchTool;`.

`crates/caliban-tools-builtin/src/web_fetch.rs` is the new file (~400 LOC including tests).

## `caliban` binary integration

In `caliban/src/main.rs` (or wherever the registry is built), construct one shared `reqwest::Client` and register a `WebFetchTool::new(client.clone())` alongside the other tools. The summarizer wiring is left out for v1 (operator can opt in later via a future CLI flag); the design ensures it's a one-line `.with_summarizer(...)` change when that flag arrives.

README's tool list gets one sentence noting `WebFetch` is available and that authenticated services are not supported.

## Testing strategy

`wiremock`-backed integration tests in `web_fetch.rs`:

1. `fetches_text_html_and_converts_to_markdown` — `<h1>Hello</h1>` → output contains `# Hello`.
2. `strips_script_and_style_before_conversion` — page with `<script>alert(1)</script>` → output does not contain `alert(1)`.
3. `fetches_text_plain_passthrough` — `text/plain` body → output is body verbatim.
4. `fetches_json_passthrough` — `application/json` → output is body verbatim.
5. `truncates_oversized_markdown` — 200 KB HTML body → output ends with truncation notice and is ≤ 100 KB body.
6. `rejects_oversized_response_body` — 11 MB body → `ToolError::Execution` with size-limit message; no OOM.
7. `rejects_non_http_scheme` — `file:///etc/passwd` → `ToolError::InvalidInput`.
8. `rejects_url_with_userinfo` — `https://user:pass@example.com` → `ToolError::InvalidInput`.
9. `rejects_bare_hostname` — `http://intranet` → `ToolError::InvalidInput`.
10. `rejects_oversized_url` — 2001-char URL → `ToolError::InvalidInput`.
11. `follows_same_host_redirect` — 301 → same host, different path → returns final body.
12. `surfaces_cross_host_redirect` — 301 → different host → output is REDIRECT DETECTED notice, not the body.
13. `caps_redirect_hops` — 11 consecutive same-host redirects → `ToolError::Execution` mentioning hop limit.
14. `binary_content_returns_notice` — `application/pdf` → output is the binary notice.
15. `non_2xx_returns_tool_error_with_status` — 404 → `ToolError::Execution` containing `Status: 404`.
16. `timeout_aborts_request` — wiremock `Delay::wait` → `ToolError::Execution` mentioning timeout.
17. `cancellation_aborts_request` — `cx.cancel.cancel()` mid-flight → `ToolError::Cancelled`.
18. `summarizer_used_when_wired_and_prompt_set` — wires `MockProvider`, enqueues a completion; assert output body is the mock text.
19. `summarizer_failure_falls_back_to_raw` — mock returns error; output includes the markdown body and `[summarization failed: …]` footer.
20. `prompt_without_summarizer_returns_raw_with_note` — set `prompt` field, no summarizer; output includes raw markdown and the "no summarizer configured" footer.

Target ~20 new tests.

`http://` → `https://` upgrade is exercised by direct unit tests on the (private) `upgrade_scheme` helper, which cover the domain-host upgrade, the https passthrough, and the IPv4/IPv6/`localhost` preservation. End-to-end wiremock testing of the upgrade is not feasible without a TLS test rig; the unit tests on the helper are the trade-off. The carve-out exists so wiremock servers bound to `127.0.0.1` work without TLS, and so a homelab user fetching `http://10.0.0.x/...` isn't forcibly upgraded to a TLS endpoint that doesn't exist.

## Risks

- **`htmd` quality on real-world pages.** It's a small crate; markdown output on JS-heavy pages may be ugly. Mitigation: pre-strip `<script>`/`<style>`/`<noscript>`/`<svg>`. We can swap to `mdka` later (one Cargo line and a function rename) if quality is bad in practice.
- **SSRF.** We deliberately allow private IPs (homelab use case). An attacker who controls the model's outputs can probe the operator's intranet. Acceptable — documented in the tool description; matches `BashTool`'s trust model.
- **No caching.** A re-fetch within a session is a full round-trip + up-to-10MB transfer. v2 work; likely `moka::future::Cache` with a 15-minute TTL and a 50 MB byte budget, URL-keyed.
- **Summarizer cost surprise.** If the operator wires a slow/expensive summarizer (e.g. Opus), every WebFetch with a `prompt` now triggers that. Mitigation: doc-comment on `with_summarizer` recommending a small/fast model.
- **Content-Type lies.** Some servers send `text/html` for JSON and vice versa. Acceptable — the markdown converter will produce literal text in those cases; not worth detection logic.
- **Streaming size-cap implementation.** `reqwest::Response::bytes()` reads the entire body before we can check size. Implementation must use `response.chunk()` in a loop and abort when accumulator + new chunk > 10 MB.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean.
- `cargo test --workspace` passes — adds ≥ 18 new tests in `caliban-tools-builtin::web_fetch::tests` (a few of the listed 20 are bundled into shared setup).
- `WebFetchTool` re-exported from `caliban_tools_builtin` and registered in the `caliban` binary's tool registry.
- README's tool list updated with one sentence about `WebFetch`.
- New ADR is **not** required (no architectural commitment beyond what's in this spec; tool just plugs into the existing `Tool` trait).
