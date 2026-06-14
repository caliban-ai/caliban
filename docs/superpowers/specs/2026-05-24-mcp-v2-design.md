# MCP v2 — Design (rmcp wiring + transports + OAuth + elicitation)

**Date:** 2026-05-24
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** `docs/adr/0023-mcp-v2-transports-and-oauth.md`
**Supersedes scope of:** v1 (`docs/adr/0017-mcp-client-architecture.md`) — v1
shipped the config schema + manager scaffold; v2 makes it actually work
and broadens transport coverage.

## Goal

Bring caliban's MCP support to feature parity with Claude Code: stdio
servers spawn and discover tools at startup, HTTP and SSE servers connect
over the network with OAuth where required, elicitation lets servers
prompt the user mid-call, resources surface via `@server:resource`
references, and operators can manage all of it from a `/mcp` overlay in
the TUI.

## Non-goals (still deferred to v3)

- **Hot reload of `mcp.toml`** — edits still require restart.
- **`notifications/tools/list_changed` handling** — server-push tool
  changes mid-session remain ignored. (We *do* honor it for prompts and
  resources where they're cheaper to refresh — see "Resources" below.)
- **Hosting an MCP server ourselves** (`claude mcp serve` parity). v2 is
  client-only.
- **Sampling / completions capability** — clients receiving sampling
  requests from servers is rare in practice and adds another surface.
- **Plugin-bundled MCP servers** — picked up once the plugin system
  (ADR 0030) lands; v2 reads `.mcp.json` from a plugin dir only if the
  plugin system has already wired it in.

## Architecture

```
caliban binary
  build_tool_registry()
    register_builtin(&mut r)
    McpClientManager::start(cfg).register_all(&mut r)
       │
       ▼
caliban-mcp-client
  McpClientManager  ── HashMap<server_name, Arc<Conn>>
  McpTool: impl Tool ── Arc<Conn>; per-call timeout + cancellation
  McpResource         ── @server:resource lookup table
  ElicitationBridge   ── server→user prompts, deliver back via tokio::sync::oneshot
       │
       ├─ Transport::Stdio ─► rmcp::transport::child_process
       ├─ Transport::Http  ─► rmcp::transport::streamable_http (POST + chunked)
       └─ Transport::Sse   ─► rmcp::transport::sse (long-lived SSE)
                  │
                  └─ McpOAuthFlow ── browser-redirect + local callback port
```

Two new dimensions vs v1:

1. **`Transport` enum** in `Conn`. The startup path picks one based on
   `ServerConfig.transport`; everything downstream is uniform — `Conn`
   exposes the same `service: rmcp::client::RunningService<…>` regardless.
2. **`ElicitationBridge` and `McpResource`** sit alongside `McpTool` —
   they're new caliban-side types, not extensions of `Tool`. Elicitation
   is delivered through a side channel the TUI subscribes to; resources
   are pulled lazily by `@server:resource` references in user messages.

## Crate structure (delta from v1)

```
crates/caliban-mcp-client/
├── Cargo.toml            # add rmcp HTTP/SSE features, oauth2, axum (callback)
└── src/
    ├── lib.rs            # re-exports (no breaking changes to v1 surface)
    ├── config.rs         # extend ServerConfig with transport, oauth, timeouts
    ├── error.rs          # add OAuth + Elicitation variants
    ├── manager.rs        # spawn + handshake wiring for all transports
    ├── client.rs         # NEW: Conn wrapper over rmcp service
    ├── tool.rs           # McpTool: Tool impl (was a stub in v1)
    ├── resource.rs       # NEW: McpResource cache + @-mention resolver
    ├── elicitation.rs    # NEW: ElicitationBridge + ElicitationRequest types
    ├── oauth.rs          # NEW: McpOAuthFlow (PKCE + loopback callback)
    └── registry.rs       # register_all(&mut ToolRegistry) — now actually registers
```

### Cargo deps (added)

```toml
[dependencies]
rmcp = { version = "1.7", features = [
    "client",
    "transport-child-process",
    "transport-streamable-http-client",
    "transport-sse-client",
] }
oauth2     = "5"              # PKCE OAuth flow
axum       = { version = "0.7", default-features = false, features = ["tokio", "http1"] }
reqwest    = { workspace = true }   # already in workspace
url        = { workspace = true }
keyring    = "3"              # OAuth token storage (Keychain/Secret Service/credential-store)
```

## Config schema (extended)

```toml
# ~/.config/caliban/mcp.toml

# --- stdio (v1, unchanged) ---
[server.linear]
command = "npx"
args    = ["-y", "@linear/mcp-server"]
env     = { LINEAR_API_KEY = "${LINEAR_API_KEY}" }
timeout = "60s"                   # per-tool-call timeout; default 60s
startup_timeout = "5s"            # handshake timeout; default 5s

# --- streamable HTTP ---
[server.notion]
transport = "http"
url       = "https://mcp.notion.example/v1"
headers   = { X-Workspace = "demo" }
oauth     = "auto"                # "auto" | "off" | "manual"

# --- SSE (legacy MCP-over-EventSource) ---
[server.legacy]
transport = "sse"
url       = "https://legacy.example/mcp/sse"
oauth     = "off"

# --- per-server permission rules (lift Claude Code's allowedMcpServers) ---
[server.linear.permissions]
allow = ["read_*"]
deny  = ["delete_*"]
```

### Field semantics (delta from v1)

| Field             | Type                  | Required | Default            | Notes                                                                                  |
| ----------------- | --------------------- | -------- | ------------------ | -------------------------------------------------------------------------------------- |
| `transport`       | `"stdio"`/`"http"`/`"sse"` | no   | `"stdio"`          | Determines which transport rmcp uses; the other transport-specific fields are required only for their transport |
| `url`             | string                | yes for http/sse | —          | Absolute URL; supports `${VAR}` and `${CLAUDE_PROJECT_DIR}` expansion                  |
| `headers`         | table str→str         | no       | `{}`               | Static request headers; supports `${VAR}` expansion                                    |
| `oauth`           | `"auto"`/`"off"`/`"manual"` | no | `"auto"`           | `auto` → discover via MCP `/.well-known/oauth-protected-resource` + RFC 8414; `manual` → use `oauth` block below |
| `timeout`         | humantime duration    | no       | `"60s"`            | Per-tool-call deadline; max 10 min; honors per-tool override via `[server.X.tools.Y.timeout]` |
| `startup_timeout` | humantime duration    | no       | `"5s"`             | Maximum wait for `initialize` reply                                                    |
| `output_limit`    | bytes                 | no       | `2 MiB`            | Truncate tool result content if larger; surfaces a warning block                       |
| `permissions`     | table (allow/deny/ask) | no      | inherit            | Tool-name globs scoped to this server                                                  |
| `[server.X.oauth]`| table                 | no       | discovered         | Manual override: `client_id`, `client_secret` (optional, PKCE preferred), `auth_url`, `token_url`, `scopes` |

Env-var expansion (`${VAR}`, `${CLAUDE_PROJECT_DIR}`) is applied to
`command`, `args`, `env`, `cwd`, `url`, `headers.*`, and any string in the
manual `oauth` block. Defaults are scoped (`${VAR:-default}`).

## Transport selection

```rust
pub enum Transport {
    Stdio { command: String, args: Vec<String>, env: BTreeMap<String, String>, cwd: Option<PathBuf> },
    Http  { url: Url, headers: HeaderMap, oauth: OauthMode },
    Sse   { url: Url, headers: HeaderMap, oauth: OauthMode },
}
```

`Conn::start(transport, server_name, timeouts)` is the single entry
point; manager doesn't care which variant it's holding. Each variant
maps directly to an rmcp transport constructor:

| Variant | rmcp constructor                                                                            |
| ------- | ------------------------------------------------------------------------------------------- |
| Stdio   | `rmcp::transport::child_process::TokioChildProcess::new(command)`                            |
| Http    | `rmcp::transport::streamable_http_client::StreamableHttpClientTransport::with_client(...)`   |
| Sse     | `rmcp::transport::sse_client::SseClientTransport::start(url, http_client)`                    |

All three are wrapped with `rmcp::client::serve_client_with_ct(transport, cancel_token)`.

## OAuth flow (PKCE + loopback)

When `transport ∈ {http, sse}` and `oauth = "auto"|"manual"`, before
`serve_client` runs:

1. **Token cache lookup** — `keyring::Entry::new("caliban-mcp", &format!("{server}:{audience}"))`. If a valid token (not within 60s of expiry) exists, use it.
2. **Discovery** (`auto`) — `GET <url>/.well-known/oauth-protected-resource`; parse `authorization_servers`; for each, `GET /.well-known/oauth-authorization-server` to get `authorization_endpoint`, `token_endpoint`, `scopes_supported`. With `manual`, skip discovery and use the configured endpoints.
3. **PKCE auth** — generate code verifier + S256 challenge; spawn an `axum` server on `127.0.0.1:0` (random port); print the auth URL with `redirect_uri=http://127.0.0.1:<port>/callback` to the TUI; the user opens it in their browser; the callback receives `code` + `state`; exchange for tokens.
4. **Persist** — write `access_token`, `refresh_token`, `expires_at` to keyring. Refresh inline when `expires_at - now < 60s` via `refresh_token`.
5. **Inject** — add `Authorization: Bearer <access_token>` to the HTTP/SSE request headers and to every JSON-RPC frame's HTTP-transport request.

If OAuth fails:
- **`auto` + discovery fails**: try without auth; log warning; the server will likely 401 on the first request and we'll surface that.
- **Browser flow fails or user cancels**: log error, skip the server, surface a "needs auth" badge in `/mcp`.
- **Refresh fails (401)**: clear the cached token; re-trigger the browser flow on next call.

A CLI flag `--mcp-oauth-port <PORT>` overrides the random port (for users behind firewalls). Env var `CALIBAN_MCP_OAUTH_PORT` honored too.

### Token storage layout

`keyring` service: `caliban-mcp`. Account: `<server>:<audience>`. Value: JSON of `{access_token, refresh_token, expires_at, scopes}`. On systems without a keyring (CI), fall back to `$XDG_DATA_HOME/caliban/mcp-tokens.json` mode 0600 with a warning logged.

## Elicitation

Servers can ask the user for input mid-tool-call via the `elicitation/create` request. caliban surfaces this through `ElicitationBridge`:

```rust
pub struct ElicitationRequest {
    pub server: String,
    pub message: String,
    pub schema: Option<serde_json::Value>,   // requested-content JSON schema
    pub cancel: CancellationToken,
}

pub enum ElicitationResponse {
    Accept(serde_json::Value),
    Decline,
    Cancel,
}

pub struct ElicitationBridge {
    tx: tokio::sync::mpsc::UnboundedSender<(ElicitationRequest, oneshot::Sender<ElicitationResponse>)>,
}
```

The TUI subscribes to the receiver and renders a modal (built on the same overlay infrastructure as the existing Ask modal — ADR 0027 covers the UI). Non-interactive callers (`--print`, CI) get an immediate `Decline` from a default handler.

Servers requesting elicitation are gated by the same permission rule grammar — `Elicit(<server>)` is allowed/denied/asked alongside tool calls.

## Resources (`@server:resource`)

MCP servers can advertise *resources* (read-only data references, often pointing at server-side documents or DB rows). caliban surfaces them two ways:

1. **`@<server>:<resource>` in a user message** — autocompletes from a per-server resource cache; on submit, caliban calls `resources/read` and inlines the result as a user-visible content block.
2. **`resources/list_changed` notification** — invalidate the cache for that server; lazily re-list next time the user opens `@`.

Resource list is fetched on demand (first `@<server>:` typed) — not eagerly at startup, to keep startup fast.

Resource templates (`uri_template: "github://repos/{owner}/{repo}/issues/{id}"`) are expanded with positional `{}` parameters typed after the resource name (`@github:issue 1234`).

## `/mcp` slash command

The TUI overlay (replacing the v1 stub) shows per-server state:

```
┌─ MCP servers ─────────────────────────────────────────────────────────┐
│ ● linear      stdio  18 tools  · auth ok        [d] disable [r] reload│
│ ● notion      http   42 tools  · oauth ok       [a] auth   [d] disable│
│ ◐ legacy      sse    0 tools   · auth expired   [a] re-auth           │
│ ○ flaky       stdio  failed: handshake timeout  [r] retry  [s] stderr │
│ ○ disabled    http   disabled by .caliban/mcp.toml                    │
└───────────────────────────────────────────────────────────────────────┘
[esc] close   [↑/↓] navigate   [enter] focus   [t] show tools
```

State glyphs:

| Glyph | Meaning              |
| ----- | -------------------- |
| ●     | Connected            |
| ◐     | Connected, but needs reauth |
| ○     | Disabled or failed   |

Keys: `d` toggle disabled (writes to `.caliban/mcp.toml`), `r` reload (re-spawn + re-list), `a` start OAuth flow, `s` view last 200 lines of stderr (stdio only), `t` show this server's tool list with input schemas.

## Per-server permission scoping

Lifts Claude Code's `allowedMcpServers`/`deniedMcpServers` into our rule grammar:

```toml
[server.linear.permissions]
allow = ["read_*", "list_*"]
deny  = ["delete_*"]
ask   = ["create_*", "update_*"]
```

These tool-name globs apply only within the server (`mcp__linear__*`). They merge with global permissions: a global `deny: ["mcp__*"]` overrides everything; a global `allow: ["mcp__linear__read_*"]` augments. Order: global deny → server deny → server ask → server allow → global ask → global allow → default (Ask).

## Timeouts and limits

Honor Claude Code's env-var contract for parity:

| Env var                      | Maps to                                          | Default |
| ---------------------------- | ------------------------------------------------ | ------- |
| `CALIBAN_MCP_TIMEOUT`        | Per-server `startup_timeout` if not set in TOML  | 5s      |
| `CALIBAN_MCP_TOOL_TIMEOUT`   | Per-server `timeout` if not set in TOML          | 60s     |
| `CALIBAN_MAX_MCP_OUTPUT_TOKENS` | warning threshold on result token count       | 10000   |

We use `CALIBAN_` prefix instead of `MCP_` so we don't conflict with servers that read `MCP_*` themselves. If `MCP_TIMEOUT`/`MCP_TOOL_TIMEOUT` are set and the `CALIBAN_` variants aren't, we honor them for Claude Code compat.

## Phased delivery

The user-visible v2 ships in three sub-PRs to keep CI tight and reviews focused. Each sub-PR is independently mergeable.

### Phase A — rmcp stdio wiring (PR/A)

- Wire `Conn::start` for `Transport::Stdio` over `rmcp::transport::child_process`.
- Implement `McpTool::invoke` (was a stub).
- In-tree integration test server (`crates/caliban-mcp-client/tests/fixtures/test_server.rs`) advertises `echo`, `fail`, `slow`, `hang_init` tools.
- 18 tests: 8 config (carried over from v1), 10 integration.
- `/mcp` overlay renders connected/failed/disabled (stdio only).
- **Closes:** the "rmcp wiring" deferred follow-up from `docs/adr/0017`.

### Phase B — HTTP + SSE transports (PR/B)

- `Conn::start` for `Transport::Http` and `Transport::Sse`.
- `oauth: "off"` only (no OAuth flow yet) — for self-hosted servers behind a fixed bearer or with public-no-auth endpoints.
- 6 tests: HTTP server with `wiremock`, SSE server with a fixture binary.
- `/mcp` overlay shows transport column.

### Phase C — OAuth + elicitation + resources (PR/C)

- `McpOAuthFlow` (PKCE + loopback callback + keyring persistence).
- `ElicitationBridge` + TUI modal.
- `McpResource` + `@server:resource` autocomplete in the TUI.
- 10 tests: OAuth happy path, refresh, cancelled flow, elicitation accept/decline/cancel, resource fetch, resource template expansion.
- `/mcp` overlay shows auth status + re-auth action.

Each phase tick rows in `docs/parity-gap-matrix.md` from 🔴/🟡 to ✅.

## Error handling

```rust
pub enum McpError {
    /* … v1 variants … */
    Transport     { server: String, kind: TransportKind, source: rmcp::Error },
    OauthDiscovery{ server: String, source: reqwest::Error },
    OauthFlow     { server: String, source: oauth2::Error },
    Elicitation   { server: String, source: ElicitationError },
    Keyring       { server: String, source: keyring::Error },
    OutputTooLarge{ server: String, tool: String, bytes: usize, limit: usize },
}
```

`Transport` and `OauthDiscovery` log + skip-server. `OauthFlow` shows the user the failure via the `/mcp` overlay. `OutputTooLarge` truncates with a `[truncated: {bytes}>{limit}]` notice.

## Cancellation

Each invocation races the rmcp future against `cx.cancel.cancelled()` *and* the per-tool deadline (`tokio::time::timeout(server.timeout, …)`). On cancellation we drop the in-flight future; rmcp sends `notifications/cancelled` automatically for HTTP/SSE; for stdio we synthesize the notification before dropping.

OAuth flows can be cancelled too (e.g. user hits Esc on the "open browser" prompt) — `axum` server is shut down on cancel.

## Testing strategy

Adds ~30 new tests over the three phases:

**Phase A (10):**
- spawn + handshake + list_tools happy path
- spawn fails (nonexistent binary)
- handshake timeout
- tool invocation roundtrip (echo)
- tool reports error (fail)
- tool times out (slow + 200ms server.timeout)
- cancellation aborts mid-call (slow + Ctrl-C synthetic)
- shutdown terminates child
- env var expansion in command/args
- stderr drained without blocking

**Phase B (6):**
- HTTP transport happy path (wiremock)
- HTTP transport 502 retry
- HTTP transport bearer header injection
- SSE transport happy path (in-tree binary)
- SSE transport reconnect after disconnect
- Custom static headers applied

**Phase C (14):**
- OAuth discovery + first auth (mock authorization server)
- OAuth token persist + reuse
- OAuth refresh inline
- OAuth flow cancelled (user closes browser)
- OAuth manual config (skip discovery)
- Keyring fallback to file when keychain absent
- Elicitation request → modal → accept
- Elicitation decline
- Elicitation cancel via timeout
- Elicitation in `--print` mode → auto-decline
- Resources list cached + invalidated on `list_changed`
- Resource read inlines content
- Resource template expansion
- `@server:resource` autocomplete suggests by prefix

## Risks

- **rmcp 1.7 still has API churn** at the transport boundary. Mitigation: pin `=1.7.x`, lock in a workspace minor.
- **OAuth audit surface.** Tokens persisted to a keyring are pickled JSON; if the keyring backend is compromised, all tokens leak. Mitigation: scope tokens minimally (whatever the auth server allows), document the trust boundary in README.
- **Elicitation deadlock.** A poorly-behaved server could issue an elicitation request and then refuse to make progress until the user responds, blocking the tool call. Mitigation: 5-minute hard cap on elicitation; on expiry, return `Decline` and surface a warning.
- **Per-server permission rules conflict with global rules.** The merge order is explicit (above), but corner cases (a `mcp__linear__delete_X` allowed at server level, denied globally) need clear UX. Mitigation: `/mcp` overlay shows the effective rule for a focused tool.
- **Loopback OAuth on hardened machines** (corporate proxies that block 127.0.0.1 callbacks). Mitigation: `--mcp-oauth-port` override and a documented `oauth = "manual"` path with `code` paste-back; out-of-band fallback can land in a v2.1.
- **Resource templates are user-typed.** Templates like `github://repos/{owner}/{repo}/issues/{id}` need positional unpacking; ambiguous numbers should fail loudly rather than silently substituting in the wrong slot.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean.
- Across the three phases, ≥30 new tests passing.
- `caliban-mcp-client::lib.rs` exports `Transport`, `OauthMode`, `ElicitationBridge`, `McpResource` (Phase B+C).
- caliban binary starts MCP servers from `mcp.toml`, registers their tools, and `/mcp` overlay shows live state.
- All 9 rows under "**H. MCP**" in `docs/parity-gap-matrix.md` move 🔴 → ✅ (resources, elicitation, OAuth, transports, /mcp slash, `${CLAUDE_PROJECT_DIR}` expansion, timeouts envs, rmcp wiring).
- README's "MCP" section documents `oauth = "auto"`, points at the per-server permission table, and demonstrates `@server:resource` syntax.
- ADR 0023 in `accepted` status (this spec's prerequisite).
