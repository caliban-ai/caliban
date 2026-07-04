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

**Auto discovery** (`oauth = "auto"`): caliban discovers the authorization endpoints from the server's `/.well-known/oauth-protected-resource` and `/.well-known/oauth-authorization-server` documents. Most hosted MCP servers (Sentry, Linear, Notion, …) also advertise a `registration_endpoint`, so caliban **dynamically registers a client (RFC 7591)** on the first run — no app registration, no `client_id`, nothing to configure but the URL:

```toml
[mcp_servers.sentry]
type  = "http"
url   = "https://mcp.sentry.dev/mcp"
oauth = "auto"
```

The first interactive run opens a browser, you authorize, and the token is cached. caliban registers a **public PKCE client** for its loopback callback; a fixed callback port (below) keeps that registration stable across runs.

> **GitHub is the exception** — its authorization server does not offer dynamic registration (`registration_endpoint: null`), so `auto` needs a manually-registered OAuth App's `client_id` (+ `client_secret`) in an `oauth_config` block:
>
> ```toml
> [mcp_servers.github]
> type  = "http"
> url   = "https://api.githubcopilot.com/mcp/"
> oauth = "auto"
>
> [mcp_servers.github.oauth_config]
> client_id     = "${GITHUB_OAUTH_CLIENT_ID}"
> client_secret = "${GITHUB_OAUTH_CLIENT_SECRET}"
> ```
>
> GitHub matches the OAuth App's *Authorization callback URL* exactly, so pin the loopback port: register the callback as `http://127.0.0.1:41870/callback` and run with `CALIBAN_MCP_OAUTH_PORT=41870`. A configured `client_id` always takes precedence over dynamic registration.

**Manual configuration** (`oauth = "manual"`): supply the endpoints yourself in a `[mcp_servers.<name>.oauth_config]` block:

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

The **first interactive run** opens your browser to authorize; the token is cached and reused (and silently refreshed near expiry) on later runs, so no browser is needed again until it fully expires. In **headless/`--print`/non-TTY** runs a server with no cached token fails fast with an actionable error rather than hanging on a callback — authorize it interactively once first. Tokens are stored in the OS keyring; caliban falls back to `$XDG_DATA_HOME/caliban/mcp-tokens.json` (mode 0600) on systems without keychain support.

> **Easiest path for GitHub — reuse the GitHub CLI's token.** If you're logged in with `gh`, its token already authenticates the GitHub MCP server, so you can skip OAuth entirely with a static bearer header and no app registration:
>
> ```toml
> [mcp_servers.github]
> type    = "http"
> url     = "https://api.githubcopilot.com/mcp/"
> oauth   = "off"
> headers = { Authorization = "Bearer ${GITHUB_MCP_TOKEN}" }
> ```
>
> then launch with `GITHUB_MCP_TOKEN=$(gh auth token) caliban …`. Nothing secret is stored on disk and the token is always current. (Any GitHub PAT with the needed scopes works the same way.)

### Environment-variable expansion

Every string value in an `[mcp_servers.<name>]` block — `url`, `headers.*`, `command`, `args`, `env.*`, and the `oauth_config.*` fields — supports `${VAR}`, `${VAR:-default}`, and `${CLAUDE_PROJECT_DIR}` expansion, so credentials can live in the environment instead of in `settings.toml`. A `${VAR}` with no value (and no default) is left as-is and the server is reported as failed rather than silently mis-configured.

### Fixed callback port

Use `--mcp-oauth-port <PORT>` (or `CALIBAN_MCP_OAUTH_PORT`) to pin the loopback callback port instead of an ephemeral one. This is **required** for authorization servers that match the `redirect_uri` exactly (GitHub OAuth Apps) and useful on firewalled machines.

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
