# MCP Servers

Caliban implements the [Model Context Protocol](https://modelcontextprotocol.io/) client side, letting you connect any MCP-compatible server as a source of additional tools. Connected servers' tools appear in the same `ToolRegistry` as built-ins, with the naming convention `mcp__<server>__<tool>`.

## Configuring servers

Servers are declared in the `mcp_servers` table of your unified settings file, or in the legacy `mcp.toml` when no unified settings are present.

### Minimal stdio server

```toml
# .caliban/settings.toml

[mcp_servers.linear]
command = "npx"
args    = ["-y", "@linear/mcp-server"]
env     = { LINEAR_API_KEY = "${LINEAR_API_KEY}" }
```

### HTTP server

```toml
[mcp_servers.notion]
type    = "http"
url     = "https://mcp.notion.com/v1"
headers = { Authorization = "Bearer ${NOTION_TOKEN}" }
```

### SSE server

```toml
[mcp_servers.legacy-api]
type = "sse"
url  = "https://api.example.com/mcp/sse"
```

## Server configuration reference

| Field | Applies to | Description |
|---|---|---|
| `type` / `transport` | all | `"stdio"` (default), `"http"`, `"sse"` |
| `command` | stdio | Executable to spawn |
| `args` | stdio | CLI arguments |
| `env` | stdio | Environment variables; `${VAR}` and `${VAR:-default}` expanded |
| `cwd` | stdio | Working directory; relative paths resolve from caliban's cwd |
| `url` | http, sse | Absolute `http://` or `https://` URL |
| `headers` | http, sse | Static request headers; values support `${VAR}` expansion |
| `oauth` | http, sse | OAuth mode: `"off"` (default), `"auto"`, `"manual"` |
| `disabled` | all | `true` to skip this server entirely |
| `permissions` | all | Per-server permission scoping (see below) |

`${CLAUDE_PROJECT_DIR}` expands to the current workspace root in all string fields, so plugin-bundled servers can reference binaries relative to the workspace without hardcoding paths.

## OAuth (`oauth = "auto"` and `"manual"`)

For HTTP/SSE servers behind OAuth, caliban performs the authorization-code flow with PKCE and a loopback callback server.

**Auto discovery** (`oauth = "auto"`): caliban discovers endpoints from the server's `/.well-known/oauth-protected-resource` and `/.well-known/oauth-authorization-server` documents.

**Manual configuration** (`oauth = "manual"`): provide a `[mcp_servers.<name>.oauth_config]` block:

```toml
[mcp_servers.my-server]
type  = "http"
url   = "https://api.example.com/mcp"
oauth = "manual"

[mcp_servers.my-server.oauth_config]
client_id  = "${MY_CLIENT_ID}"
auth_url   = "https://auth.example.com/authorize"
token_url  = "https://auth.example.com/token"
scopes     = ["read", "write"]
```

Tokens are stored in the OS keyring; caliban falls back to `$XDG_DATA_HOME/caliban/mcp-tokens.json` (mode 0600) on systems without keychain support.

Use `--mcp-oauth-port <PORT>` (or `CALIBAN_MCP_OAUTH_PORT`) to fix the loopback callback port on firewalled machines instead of letting caliban pick an ephemeral one.

## Per-server permissions

Each server can declare scoped permission rules that compose with the global rule grammar. Patterns match the *unprefixed* tool name; caliban expands them to `mcp__<server>__<tool>` when evaluating against the global engine.

```toml
[mcp_servers.linear.permissions]
allow = ["read_*", "list_*"]
deny  = ["delete_*"]
ask   = ["create_*", "update_*"]
```

Merge order when multiple rules match a call:
`global deny → server deny → server ask → server allow → global ask → global allow → default (Ask)`

## Discovery and the `/mcp` overlay

At startup, caliban connects to every non-disabled server, sends `initialize`, and registers one `McpTool` per advertised tool. Failures (spawn error, handshake timeout) are logged at `warn` and skipped — they do not abort startup.

The `/mcp` slash command shows per-server status:

| Glyph | Meaning |
|---|---|
| `●` | Connected |
| `◐` | Connecting / partial |
| `○` | Disabled or failed |

## `@server:resource` references

Type `@<server>:` in the input bar to trigger resource autocomplete for that server. Caliban calls `resources/list` lazily on first use and caches the result; `resources/list_changed` notifications invalidate the cache.

## Elicitation

When an MCP server needs additional input from the user (for example, before a destructive operation), it sends an elicitation request. In interactive mode, caliban shows a TUI modal. In `--print` / CI mode, elicitation requests are automatically declined.

## Controls

| Flag / env | Effect |
|---|---|
| `--no-mcp` | Skip all MCP server discovery and registration |
| `CALIBAN_NO_MCP=1` | Same, via environment variable |
| `--mcp-oauth-port <PORT>` | Fix the loopback OAuth callback port |
| `CALIBAN_MCP_OAUTH_PORT=<PORT>` | Same, via environment variable |

```admonish tip title="Config file location"
The preferred location for MCP server config is the `mcp_servers` table in `.caliban/settings.toml` (project scope) or `~/.config/caliban/settings.toml` (user scope). The legacy `mcp.toml` is still supported as a fallback when no unified settings file is present — project overrides user at the same server name, wholesale.
```

## Related pages

- [Plugins](./plugins.md) — plugins can bundle MCP server configs
- [Permissions concepts](../permissions/concepts.md)
- [Slash Command Index](../reference/slash-index.md) — `/mcp` overlay
