# MCP client — Design

**Date:** 2026-05-23
**Status:** Approved
**Target branch:** `jf/docs/roadmap-post-webfetch`
**Sub-project of:** caliban Rust agent harness
**ADR:** `adrs/0017-mcp-client-architecture.md`
**Depends on:** `caliban-agent-core`, `caliban-provider`

## Goal

Add a `caliban-mcp-client` crate that connects to one or more locally-
launched MCP servers over stdio, discovers their tools at startup, and
exposes each as a `Box<dyn Tool>` in the existing `ToolRegistry`. The
agent and TUI treat MCP tools identically to built-ins: same dispatch
path, same hooks, same cancellation semantics. Operators get the long
tail of integrations (Linear, Notion, Slack, in-house servers) without
caliban needing a built-in tool per service.

## Non-goals

- **SSE transport.** Deferred to v2 — separate dep stack (`reqwest-eventsource`).
- **StreamableHTTP transport.** Deferred to v2 alongside SSE.
- **Per-server OAuth.** Deferred to v2 (lands with SSE/HTTP where it actually matters).
- **Server-side capabilities (`roots`, elicitation handler).** caliban is a client only; we do not host an MCP server endpoint.
- **MCP resources (`mcp__<server>__resources/*`).** Resource read/list is a separate v2 feature; v1 surfaces tools only.
- **Prompts capability.** Server-advertised prompt templates are not exposed in v1.
- **Hot reload of `mcp.toml`.** Edits require caliban restart.
- **`notifications/tools/list_changed` handling.** Server-push tool changes mid-session are ignored in v1.

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
  McpTool: impl Tool ── holds Arc<Conn>, calls service.call_tool(...)
       │  rmcp::transport::child_process
       ▼
  [stdio child: linear-mcp]  [stdio child: filesystem-mcp]  ...
```

`McpClientManager` owns all connections and is the only code that
talks to `rmcp`. `McpTool` holds an `Arc<Conn>` so each tool reaches
its server without going through the manager on the hot path.

## Crate structure

New workspace member: `crates/caliban-mcp-client/`.

```
crates/caliban-mcp-client/
├── Cargo.toml
└── src/
    ├── lib.rs       # re-exports + crate-level docs
    ├── config.rs    # mcp.toml schema + merge logic
    ├── client.rs    # Conn / McpClientManager / startup + shutdown
    ├── tool.rs      # McpTool: impl Tool
    └── registry.rs  # register_all(&mut ToolRegistry) helper
```

`Cargo.toml` deps:

```toml
[dependencies]
caliban-agent-core = { path = "../caliban-agent-core" }
caliban-provider   = { path = "../caliban-provider" }
rmcp        = { version = "0.x", features = ["client", "transport-child-process"] }
tokio       = { workspace = true, features = ["process", "sync", "rt"] }
serde       = { workspace = true, features = ["derive"] }
toml        = { workspace = true }
thiserror   = { workspace = true }
tracing     = { workspace = true }
async-trait = { workspace = true }
dirs        = { workspace = true }   # for ~/.config resolution

[dev-dependencies]
tokio    = { workspace = true, features = ["macros", "rt-multi-thread", "test-util"] }
tempfile = { workspace = true }
```

`rmcp` is the only new external dep; everything else is already
workspace-pinned.

## Server config schema

Path resolution (first existing wins, then merged with project overriding user):

1. `<config_dir>/caliban/mcp.toml` — per-user
   - Linux: `$XDG_CONFIG_HOME/caliban/mcp.toml` or `~/.config/caliban/mcp.toml`
   - macOS: `~/Library/Application Support/caliban/mcp.toml`
   - Windows: `%APPDATA%\caliban\mcp.toml`
2. `<cwd>/.caliban/mcp.toml` — per-project (optional)

Both files are optional; if neither exists, the manager registers
zero tools and logs nothing (this is not an error — most operators
won't use MCP).

### TOML schema

```toml
# ~/.config/caliban/mcp.toml
[server.linear]
command = "npx"
args    = ["-y", "@linear/mcp-server"]
env     = { LINEAR_API_KEY = "${LINEAR_API_KEY}" }
cwd     = "/Users/jane/code"        # optional; defaults to caliban's cwd
disabled = false                    # optional; defaults to false

[server.filesystem]
command = "mcp-server-filesystem"
args    = ["--root", "/Users/jane/notes"]
```

Field semantics:

| Field      | Type                  | Required | Default        | Notes                                                        |
| ---------- | --------------------- | -------- | -------------- | ------------------------------------------------------------ |
| `command`  | string                | yes      | —              | Executable path or PATH-resolvable name                      |
| `args`     | array of strings      | no       | `[]`           | Passed verbatim                                              |
| `env`      | table of string→string| no       | `{}`           | Merged with parent env; `${VAR}` syntax expands from os env  |
| `cwd`      | string (path)         | no       | caliban's cwd  | Resolved relative to caliban's cwd if not absolute           |
| `disabled` | bool                  | no       | `false`        | Skip this server entirely; useful for project-level disable  |

Server names (`linear`, `filesystem` in the example) must match
`^[a-z0-9_-]{1,32}$`. Invalid names abort startup with a clear error
(this is a config bug, not a runtime failure).

### `${VAR}` env expansion

`env = { FOO = "${BAR}" }` reads `BAR` from caliban's process env at
startup. If `BAR` is unset, the spawn is skipped and a warning logged.
Only full-value substitution is supported; inline interpolation
(`"prefix-${BAR}-suffix"`) is rejected — keeps the implementation a
few lines.

### Merge rule

Project-level `[server.<name>]` entries replace user-level entries of
the same name wholesale (not field-merge). This matches how git and
ssh config behave and keeps semantics easy to explain.

## Tool surface

For each `(server, tool)` pair discovered at startup, register one
`McpTool`:

```rust
pub struct McpTool {
    server_name: String,
    tool_name: String,
    full_name: String,           // "mcp__<server>__<tool>"
    description: String,         // from server
    input_schema: serde_json::Value,
    conn: Arc<Conn>,             // shared per-server connection
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str { &self.full_name }
    fn description(&self) -> &str { &self.description }
    fn input_schema(&self) -> &serde_json::Value { &self.input_schema }
    async fn invoke(&self, input: Value, cx: ToolContext)
        -> Result<Vec<ContentBlock>, ToolError> { /* see below */ }
}
```

Name shape `mcp__<server>__<tool>` matches Claude Code. `server` is
the config-file table key; `tool` is normalized to
`[a-zA-Z0-9_-]` by replacing other chars with `_` so providers that
restrict tool names accept it.

## Connection lifecycle

```rust
struct Conn {
    service: rmcp::client::RunningService<...>,  // rmcp owns the child + transport
    child_pid: Option<u32>,                      // for diagnostics
}
```

### Startup

For each enabled server in the merged config:

1. Build a `tokio::process::Command` with `command`, `args`, expanded
   `env`, `cwd`, `stdin/stdout` set to `Stdio::piped()`, `stderr`
   set to `Stdio::piped()` and drained to `tracing::warn!` on a
   background task. `kill_on_drop(true)`.
2. Hand the command to `rmcp::transport::child_process::TokioChildProcess::new`,
   then `rmcp::client::serve_client(..)` to drive the `initialize`
   handshake with a **5-second timeout** on the first response.
3. On handshake success, call `service.list_tools(None).await`.
4. For each advertised tool, push an `McpTool` into the manager's
   pending-registration vec.
5. On any failure (spawn error, handshake timeout, `list_tools`
   error), log `tracing::warn!("mcp server '{name}' failed: {err}")`
   and continue with the next server. Drop the partially-initialized
   child (the `kill_on_drop` cleans up).

### During session

Each `McpTool::invoke` acquires its `Arc<Conn>` and calls
`service.call_tool(...)`. The `rmcp` service is itself `Send + Sync`
and handles concurrent calls internally (one per JSON-RPC request
id), so no manager-level lock is held during invocation.

### Shutdown

`McpClientManager::shutdown(self)` (consuming) iterates all `Conn`s,
calls `service.cancel().await` with a 1-second budget per server,
then drops the service. `kill_on_drop` reaps any stragglers. Called
from the binary's drop path; if caliban panics, `Drop` on `Conn`
still triggers `kill_on_drop`.

## Tool invocation flow

```
agent loop                        McpTool                   rmcp::Service
    │                                │                            │
    │ dispatch(tool_use)             │                            │
    ├───────────────────────────────►│                            │
    │                                │ call_tool(name, input)     │
    │                                ├───────────────────────────►│
    │                                │                            │ ─► child stdin (JSON-RPC)
    │                                │                            │ ◄─ child stdout (JSON-RPC)
    │                                │ CallToolResult             │
    │                                │◄───────────────────────────┤
    │ Vec<ContentBlock>              │                            │
    │◄───────────────────────────────┤                            │
```

`McpTool::invoke` body sketch:

```rust
async fn invoke(&self, input: Value, cx: ToolContext)
    -> Result<Vec<ContentBlock>, ToolError>
{
    let call = self.conn.service.call_tool(
        rmcp::CallToolRequestParam {
            name: self.tool_name.clone().into(),
            arguments: input.as_object().cloned(),
        }
    );
    let result = tokio::select! {
        biased;
        () = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
        r = call => r.map_err(ToolError::execution)?,
    };
    if result.is_error.unwrap_or(false) {
        // MCP-side error: propagate as Execution with the server's text content
        return Err(ToolError::Execution(format_error(&result.content).into()));
    }
    Ok(translate_content(result.content))
}
```

### Content translation

| MCP `content` variant       | caliban `ContentBlock`                            |
| --------------------------- | ------------------------------------------------- |
| `text { text }`             | `ContentBlock::Text(TextBlock { text, .. })`      |
| `image { data, mime_type }` | `ContentBlock::Image { source: Base64 { .. } }`   |
| `resource { … }`            | `ContentBlock::Text` describing the resource URI  |
| `audio { … }`               | `ContentBlock::Text` noting audio (not supported) |

Image and resource passthrough mirrors how the built-in tools emit
content; downstream providers (Anthropic / OpenAI / Google) already
handle multi-block tool results.

## Error handling

`caliban-mcp-client` defines an internal `McpError`:

```rust
#[derive(thiserror::Error, Debug)]
pub enum McpError {
    #[error("config parse error in {path}: {source}")]
    ConfigParse { path: PathBuf, source: toml::de::Error },
    #[error("invalid server name '{0}' (must match [a-z0-9_-]{{1,32}})")]
    InvalidServerName(String),
    #[error("env var '{var}' referenced by server '{server}' is not set")]
    MissingEnv { server: String, var: String },
    #[error("server '{server}' failed to start: {source}")]
    Spawn { server: String, source: std::io::Error },
    #[error("server '{server}' handshake timed out after 5s")]
    HandshakeTimeout { server: String },
    #[error("server '{server}': {source}")]
    Rpc { server: String, source: rmcp::ServiceError },
}
```

Only `ConfigParse` and `InvalidServerName` abort startup — those are
operator bugs. All other variants log a warning and skip the server.
Per-invocation errors map to `ToolError::Execution`; cancellations
map to `ToolError::Cancelled`.

## Cancellation

The agent's existing `CancellationToken` flows into `McpTool::invoke`
via `ToolContext`. The invocation races the RPC against
`cx.cancel.cancelled()`. On cancel, we drop the in-flight call and
return `ToolError::Cancelled`; `rmcp` is documented to send
`notifications/cancelled` for the abandoned request id, so the server
can free its work. We do not wait for an ack. This matches
`BashTool`'s behavior.

Session shutdown cancellation is separate: `McpClientManager::shutdown`
explicitly stops each service before drop.

## Hooks integration

MCP tool calls go through the existing dispatch path in
`caliban_agent_core::Agent`, which calls `Hooks::before_tool` and
`Hooks::after_tool` regardless of whether the tool is a built-in or
an MCP wrapper. **No MCP-specific hook surface is added.** Operators
who want to deny specific MCP tools write a hook that inspects
`ToolCtx::tool_name` for the `mcp__` prefix and applies their
policy — exactly the same way they'd block `Bash` or `Write` today.

This is the load-bearing design choice: it means audit logging,
permission prompts, and deny-by-default policies all work for MCP
without code changes to the agent or TUI.

## Testing strategy

### Unit tests (in `caliban-mcp-client`)

1. `config_parses_minimal_server` — one server with just `command`.
2. `config_parses_full_server` — all fields populated.
3. `config_rejects_invalid_server_name` — uppercase, > 32 chars, special chars.
4. `config_expands_env_vars` — `${HOME}` resolves at parse time.
5. `config_missing_env_var_warns` — unset var → server is skipped, warning logged.
6. `config_project_overrides_user` — same name in both files → project wins wholesale.
7. `config_disabled_skipped` — `disabled = true` → not registered.
8. `tool_name_normalization` — server-advertised name with `/` becomes `_`.
9. `content_translation_text` — MCP text content → `ContentBlock::Text`.
10. `content_translation_image` — MCP image content → `ContentBlock::Image`.
11. `content_translation_audio_falls_back_to_text` — emits a "not supported" notice.

### Integration tests

Spawn an in-tree minimal MCP test server built with `rmcp` (lives in
`crates/caliban-mcp-client/tests/fixtures/test_server.rs`, built as a
binary via `[[test]]` harness). It advertises two tools:
`echo` (returns the input as text) and `fail` (returns
`is_error = true`).

12. `discovers_and_registers_tools` — start manager against the test server, assert both tools registered as `mcp__test__echo` and `mcp__test__fail`.
13. `invokes_tool_and_returns_content` — call `mcp__test__echo` with `{"msg": "hi"}` → result block contains `hi`.
14. `surfaces_server_side_error` — call `mcp__test__fail` → `ToolError::Execution` with server's error text.
15. `cancellation_aborts_call` — long-running `slow` tool (added to test server); cancel token fires; tool returns `Cancelled` within 100ms.
16. `failed_server_does_not_abort_startup` — point at a nonexistent binary; manager startup returns Ok with zero tools registered for that server; other servers register normally.
17. `handshake_timeout_skips_server` — test server with `--hang-init` flag; manager logs warning and skips.
18. `shutdown_terminates_children` — start manager, assert child PID alive, shutdown, assert PID gone within 1s.

Target ~17 new tests across unit + integration.

CI: the integration tests require `cargo` to build the in-tree test
server binary as a sibling artifact. We don't depend on any external
MCP server binary (`mcp-server-everything` was considered, but adding
a Node toolchain to CI is a worse trade than a 100-LOC `rmcp` test
server).

## Risks

- **`rmcp` API churn.** Pre-1.0; breaking changes may force version bumps mid-development. Mitigation: pin exact minor in the workspace; bump deliberately.
- **Tool name collisions across servers.** Two servers both expose a `search`. The `mcp__<server>__` prefix handles it, but a friendlier alias mechanism is a v2 ask.
- **Server stderr noise.** Some MCP servers chat heavily on stderr. We drain to `tracing::warn!`; if spammy, downgrade to `debug`.
- **Sequential startup.** 5 servers × ~1s spawn = 5s before caliban is interactive. Follow-up (v1.1): parallel spawn via `futures::future::join_all`. Most operators run 0–2 servers so it's not a v1 blocker.
- **Schema mismatch with provider expectations.** Servers may advertise JSON Schemas with fields Anthropic's tool-use API rejects (`$schema`, cyclic `$defs`). v1 passes through unchanged; add a sanitization pass if real failures show up.
- **`kill_on_drop` uses SIGKILL on Unix;** well-behaved servers prefer SIGTERM + grace. v2 may add graceful shutdown via the `cancelled` notification first.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean.
- `cargo test --workspace` passes — adds ≥ 15 new tests across `caliban-mcp-client::{config, tool, integration}::tests`.
- New crate `caliban-mcp-client` is a workspace member and listed in the root `Cargo.toml`.
- caliban binary calls `McpClientManager::start(...).register_all(&mut registry)` during startup; failure to load `mcp.toml` does not abort.
- `/mcp` overlay (ADR 0013) is updated to list configured servers and per-server status (connected / failed / disabled). Stub text is removed.
- README's tool list mentions MCP as a tool source, with a one-line pointer at `~/.config/caliban/mcp.toml`.
- ADR 0017 is in `accepted` status (this spec's prerequisite).
