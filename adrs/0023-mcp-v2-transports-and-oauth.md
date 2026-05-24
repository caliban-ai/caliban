# ADR 0023 · MCP v2 — transports, OAuth, elicitation, resources

- **Status:** proposed
- **Date:** 2026-05-24
- **Spec:** `docs/superpowers/specs/2026-05-24-mcp-v2-design.md`
- **Supersedes scope of:** ADR 0017 deferred items

## Context

ADR 0017 shipped caliban's MCP client as a config-only scaffold:
`McpClientManager::start` is a no-op, `McpTool::invoke` is unwritten,
and the only working pieces are TOML parsing and server-name
validation. Closing the gap to Claude Code requires (a) actually
wiring `rmcp` so stdio servers spawn and discover tools, and (b)
adding HTTP/SSE transports + OAuth + elicitation + resources.

## Decision

### Phased delivery — three sub-PRs

v2 ships in three independently-mergeable phases:

- **Phase A — stdio wiring.** Implement `Conn::start` for stdio and
  `McpTool::invoke`. In-tree test server. Closes the deferred
  "rmcp wiring" follow-up from ADR 0017.
- **Phase B — HTTP + SSE transports.** Adds `Transport::Http` and
  `Transport::Sse` over the corresponding `rmcp` transport modules.
  `oauth = "off"` only at this phase — for self-hosted endpoints
  behind a fixed bearer or no auth.
- **Phase C — OAuth + elicitation + resources.** `McpOAuthFlow` (PKCE
  + loopback callback + `keyring` token storage), `ElicitationBridge`
  (TUI modal + non-interactive auto-decline), `McpResource`
  (`@server:resource` autocomplete and inline read).

Each phase ticks rows in `docs/parity-gap-matrix.md` from 🔴 → ✅ in the
PR that lands it.

### Transport selection is a config field, not separate crates

`ServerConfig.transport: "stdio" | "http" | "sse"` (default `"stdio"`)
selects which `rmcp` transport constructor to call. The manager is
otherwise transport-agnostic — `Conn` exposes the same
`rmcp::client::RunningService<…>` regardless of transport. This keeps
the agent-side code path uniform: `Hooks`, dispatch, cancellation,
and serialization see no MCP-transport details.

### OAuth uses PKCE + a loopback callback on a random port

Hosted MCP servers behind OAuth use the authorization-code flow with
PKCE (S256). caliban spawns a short-lived `axum` server on
`127.0.0.1:0`, prints the auth URL, captures the callback, and
exchanges the code for tokens. Tokens persist in the OS keyring
(`keyring` crate); fallback to `$XDG_DATA_HOME/caliban/mcp-tokens.json`
mode 0600 on systems without keychain support. `--mcp-oauth-port` and
`CALIBAN_MCP_OAUTH_PORT` override the random port for firewalled
machines.

We pick PKCE + loopback over device-code or out-of-band paste because
it's what Claude Code uses and what RFC 8252 recommends for native
clients. A v2.1 follow-up may add a paste-back fallback if real
demand emerges from operators on hardened networks.

### Elicitation is a side-channel, not a tool

`ElicitationBridge` is a separate caliban-side type with its own mpsc
queue; it does **not** extend the `Tool` trait. The TUI subscribes;
non-interactive callers (`--print`, CI) get a default auto-`Decline`
handler. Elicitation requests are gated by the existing permission
rule grammar via a new pattern: `Elicit(<server>)`.

### Resources are pulled lazily

Resources are not eagerly listed at startup. The first time the user
types `@<server>:`, caliban calls `resources/list` for that server and
caches the result; `resources/list_changed` notifications invalidate
the cache. Resource templates like
`github://repos/{owner}/{repo}/issues/{id}` are expanded positionally
from arguments typed after the resource name.

### Per-server permission scoping lifted into our rule grammar

Claude Code's `allowedMcpServers` / `deniedMcpServers` settings become
inline `[server.X.permissions]` blocks in `mcp.toml`. They merge with
global permissions in a documented order:
`global deny → server deny → server ask → server allow → global ask
→ global allow → default(Ask)`. The `/mcp` overlay shows the effective
rule for a focused tool.

### Env-var contract — `CALIBAN_*` primary, `MCP_*` fallback

caliban reads `CALIBAN_MCP_TIMEOUT`, `CALIBAN_MCP_TOOL_TIMEOUT`,
`CALIBAN_MAX_MCP_OUTPUT_TOKENS`. If those are unset and the
Claude-Code-style `MCP_TIMEOUT` / `MCP_TOOL_TIMEOUT` are set, we honor
them for compat. We do **not** read `MAX_MCP_OUTPUT_TOKENS` without
the `CALIBAN_` prefix because servers may set it themselves.

## Consequences

- **Positive:** Closes nine 🔴 rows in the parity matrix in one
  multi-PR initiative. Transport plurality makes hosted-MCP
  ecosystems reachable; OAuth unblocks every commercial server that
  uses it. Elicitation is a meaningful UX upgrade (servers can ask
  before destructive ops without baking confirmation into every
  tool). Resources turn MCP from "tools only" into "tools + data
  references" — closes the `@server:resource` parity gap.
- **Negative:** Dependency footprint grows by ~5 crates (`rmcp` HTTP/SSE
  features, `oauth2`, `axum`, `keyring`). Loopback OAuth assumes the
  user can open a browser; hardened workstations may need
  `oauth = "manual"`. Token storage adds a per-OS contract surface to
  test. Elicitation introduces a new modal flow the TUI must handle
  alongside the Ask modal.
- **Revisit if:** Hosted MCP ecosystem standardizes on a different
  auth flow; if `rmcp` evolves a higher-level OAuth helper, our
  bespoke flow can shrink. If resource discovery latency becomes a
  problem (large `resources/list` responses), promote to eager fetch
  with a background refresh task.
