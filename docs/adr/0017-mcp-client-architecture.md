# ADR 0017 · MCP client architecture

- **Status:** accepted
- **Date:** 2026-05-23

## Context

caliban's `/mcp` overlay is a stub (see ADR 0013). Adding real MCP
(Model Context Protocol) client support is priority #1 on the
post-WebFetch roadmap: it unlocks the long tail of integrations
(Linear, Notion, Slack, in-house servers) without needing a built-in
tool per service. The full implementation spec lives at
`docs/superpowers/specs/2026-05-23-mcp-client-design.md`; this ADR
records the architectural commitments only.

## Decision

### Transport: stdio in v1; SSE + StreamableHTTP deferred

v1 ships **stdio transport only**. Each configured server is launched
as a child process; JSON-RPC frames travel over its stdin/stdout. SSE
and StreamableHTTP transports are non-trivial separate deps
(`reqwest-eventsource`, hyper streaming) and gate on real-world
demand. They land in v2.

### SDK: `rmcp` (official Rust SDK)

We adopt the `rmcp` crate (the official Rust MCP SDK published by the
Model Context Protocol org) over the community `mcp-client` crate.
Rationale: official maintenance, broader trait coverage (Client +
Server + transports), and a working `transport::child_process` module
we'd otherwise reimplement. Pinned to `rmcp = "0.x"` (latest released
line at adoption time); workspace-pinned to keep upgrades atomic
across our crates.

### Auth: env-var only in v1; OAuth deferred

v1 supports passing secrets to MCP servers via the `env` table in the
server-config TOML (with optional `${VAR}` expansion from the
operator's environment). Per-server OAuth — the protocol's full
authentication story for hosted MCP servers — is deferred to v2 and
will land alongside SSE/HTTP transports, where it's actually
relevant. Stdio servers overwhelmingly authenticate via env vars
today.

### Tools surface as `Box<dyn Tool>` in the existing registry

MCP-discovered tools wrap in an `McpTool` struct that implements the
`caliban_agent_core::Tool` trait and registers in the same
`ToolRegistry` as built-ins. **Naming convention:**
`mcp__<server>__<tool>` (double underscores) — mirrors Claude Code so
operators recognize the surface. `<server>` is the config-file table
name; `<tool>` is the server-advertised tool name; both are
ASCII-snake-case-normalized at registration so names match what the
provider's tool-use API accepts.

`Tool::input_schema()` returns the schema the server advertised, with
no rewriting. `Tool::invoke()` proxies via `rmcp` and translates the
response into caliban `ContentBlock`s. Hooks (`before_tool` /
`after_tool`) fire for MCP tools exactly as they do for built-ins —
no special case — which means existing permission UX, audit logging,
and deny-rules cover MCP automatically.

### Server config file

Two TOML files, merged at startup with project overriding user:

- `~/.config/caliban/mcp.toml` (per-user; XDG-aware on Linux, cache_dir on macOS)
- `.caliban/mcp.toml` (per-project, relative to cwd; optional)

Schema is fully specified in the design doc. Project-level config can
disable a user-level server by setting `disabled = true` for the same
name. Config-file location and merge semantics will be revisited
when the broader `.caliban/` config story lands (separate spec); the
MCP spec is the prior art that pattern will follow.

### Discovery: best-effort at startup

At caliban startup, for each non-disabled server entry: spawn the
child process, send `initialize`, list tools, and register an
`McpTool` per advertised tool. A failure (spawn fails, handshake
times out, server reports an error) **logs a warning and continues**
— it does not abort startup. The TUI's `/mcp` overlay surfaces
per-server status (connected / failed / disabled) so the operator
can see what's missing without watching stderr.

Tools are not re-discovered after startup in v1; if a server adds a
tool mid-session, the user restarts caliban. Server-push
notifications (`notifications/tools/list_changed`) are deferred.

### Lifecycle: session-scoped; cleanup on exit

Servers run for the duration of the caliban session. On shutdown
(clean exit, Ctrl-C, panic) the `McpClientManager`'s `Drop`
sends `notifications/cancelled` to each server and drops its
`tokio::process::Child` (configured with `kill_on_drop(true)`), so
no servers leak even on unclean exit.

## Consequences

- **Positive:** Unblocks integration with the dozens of stdio MCP
  servers already published. Tool surface is uniform — same trait,
  same registry, same hooks — so the agent and TUI need no MCP-
  specific code paths after registration. Stdio-first keeps the
  initial dep surface small.
- **Negative:** SSE/HTTP servers aren't reachable in v1; operators
  who want a hosted MCP server have to wait for v2 or wrap it in a
  local stdio proxy. The `mcp__<server>__<tool>` name shape is long
  and noisy in transcripts; acceptable for parity with Claude Code.
  Each server is one extra child process — RAM and FD overhead is
  per-server, not amortized.
- **Revisit if:** Real demand emerges for hosted (SSE/HTTP) servers —
  promote the v2 work earlier. If `rmcp`'s release cadence lags
  protocol changes, evaluate `mcp-client`. If tool-name collisions
  become common (two servers exposing a tool with the same short
  name), the `mcp__<server>__` prefix already handles it, but UX
  may want a friendly alias mechanism.
