# Settings Reference

Every key recognized by `settings.toml` (and its JSON equivalent) is listed below, grouped by topic. For merge semantics see [Settings Layering](settings-layering.md); for file paths see [File Locations](locations.md).

All fields are optional. Unknown top-level keys are tolerated for forward-compatibility (they are collected and ignored rather than causing a parse error).

---

## Model / Agent

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `agent` | `string` | — | Agent profile name used as a sub-agent dispatch hint |
| `model` | `string` or `{ provider, name }` | provider default | Primary model. Bare string (e.g. `"claude-sonnet-4-7"`) or qualified object (e.g. `{ provider = "anthropic", name = "claude-sonnet-4-7" }`). CLI `--model` / `--provider` override this |
| `fallback_model` | `string` or `{ provider, name }` | — | Model used when the primary returns an error. Wired through `caliban-model-router`. CLI `--fallback-model` overrides this |
| `model_overrides` | `{ route → model }` | `{}` | Per-named-route model overrides passed to the router (e.g. `{ "fast-classifier" = "claude-haiku-4-7" }`) |

For provider and model selection details see [Model Selection](../providers/models.md) and [The Model Router](../providers/router.md).

---

## Permissions

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `permissions.allow` | `string[]` | `[]` | Patterns that auto-allow without prompting. Concatenated across scopes |
| `permissions.ask` | `string[]` | `[]` | Patterns that prompt the user. Concatenated across scopes |
| `permissions.deny` | `string[]` | `[]` | Patterns that hard-deny. Concatenated across scopes |
| `permissions.rules` | `RuleSpec[]` | `[]` | v2 ordered rule array. When non-empty, takes precedence over the three-bucket form above. Source order is preserved; first match wins |
| `permissions.enforce` | `bool` | `false` | When `true`, refuse `--no-permissions` / bypass mode at startup |
| `permissions.default_mode` | `string` | `"default"` | Initial permission mode at session start. Valid values: `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions` |
| `permissions.audit_log` | `bool` | `true` | Enable the append-only decision log |

Each entry in `permissions.rules` supports:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `pattern` | `string` | yes | Glob pattern matching `Tool` or `Tool:first-arg-glob` |
| `action` | `"allow"` \| `"ask"` \| `"deny"` | yes | Decision when this rule matches |
| `comment` | `string` | no | Human-readable note shown in `/permissions` |
| `reason` | `string` | no | Deny reason surfaced to the operator and logged |
| `expires_at` | ISO-8601 datetime | no | Rule is skipped after this timestamp |

See [Permissions Concepts](../permissions/concepts.md) and [Pattern Grammar](../permissions/patterns.md) for full detail.

---

## Hooks

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `hooks` | `{ event → handler[] }` | `{}` | Hook event map. Keys are event names (e.g. `"PreToolUse"`, `"SessionEnd"`); values are handler lists |
| `disable_all_hooks` | `bool` | `false` | Kill-switch that disables every external hook handler. In-process hooks (permissions, audit) still run |
| `allow_managed_hooks_only` | `bool` | `false` | When `true`, only hooks defined in the managed scope fire |
| `allowed_http_hook_urls` | `string[]` | `[]` | Glob allowlist for HTTP hook endpoint URLs |
| `http_hook_allowed_env_vars` | `string[]` | `[]` | Env-var names that HTTP hook handlers are allowed to read |

See [Hooks](../extending/hooks.md) for the full event list and handler shapes.

---

## MCP Servers

`mcp_servers` is a map of server name to server configuration. Each entry deep-merges across scopes so a project scope can add environment variables to a user-scope server without redefining the whole entry.

```toml
[mcp_servers.linear]
command = "npx"
args    = ["-y", "@linear/mcp-server"]

[mcp_servers.silverbullet]
type = "http"
url  = "https://mcp.example.com/mcp"
headers = { Authorization = "Bearer ${SB_TOKEN}" }
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `type` | `"stdio"` \| `"http"` \| `"sse"` | `"stdio"` | Transport. Also accepted as `transport` (legacy alias) |
| `command` | `string` | `""` | Executable (stdio only) |
| `args` | `string[]` | `[]` | Argv after command (stdio only) |
| `env` | `{ key → value }` | `{}` | Environment variables (stdio only) |
| `cwd` | `string` | — | Working directory override (stdio only) |
| `url` | `string` | — | Absolute HTTP/HTTPS URL (http/sse only) |
| `headers` | `{ key → value }` | `{}` | Static request headers (http/sse only) |
| `oauth` | `"off"` \| `"auto"` \| `"manual"` | `"off"` | OAuth mode (http/sse only) |
| `permissions.allow` | `string[]` | `[]` | Per-server allow list (composed with global rules) |
| `permissions.deny` | `string[]` | `[]` | Per-server deny list |
| `disabled` | `bool` | `false` | Skip this server on startup |

See [MCP Servers](../extending/mcp.md) for configuration examples and the OAuth flow.

---

## Router

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `router` | object | — | Opaque config blob passed to `caliban-model-router`. The router crate owns the schema; see [The Model Router](../providers/router.md) |

---

## Memory

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `memory` | object | — | Memory tier knobs passed to `caliban_memory::MemoryConfig`. Sub-keys include `auto_memory_enabled` (bool), `auto_memory_directory` (string), `cap_tokens_auto`, `cap_tokens_claude_md`, `cap_tokens_combined` (integers) |

See [Memory Tiers](../memory/tiers.md) and [CLAUDE.md & Imports](../memory/claude-md.md).

---

## Plugins

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `plugins` | object | — | Plugin manager knobs. Schema is owned by the plugin subsystem; see [Plugins](../extending/plugins.md) |

---

## UI / Output

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `output_style` | `string` | `"default"` | Active output-style name. See [Output Styles](../extending/output-styles.md). **Restart-required** |
| `editor_mode` | `"vim"` \| `"emacs"` | — | Input-line editing mode |
| `view_mode` | `string` | — | Compact vs. expanded TUI layout |
| `statusLine` | object | — | Custom statusline command. Also accepted as `status_line` (TOML-friendly alias) |
| `tui` | object | — | TUI theme and layout knobs (e.g. `showCostInStatusline`) |

`statusLine` sub-keys:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `command` | `string` | — | Shell command whose stdout is used as the statusline text. **Required** |
| `timeout_ms` | `integer` | — | Per-invocation timeout in ms (50–5000) |
| `padding` | `integer` | — | Horizontal padding cells (0–8) |

---

## Authentication

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `api_key_helper` | `string`, object, or `object[]` | — | Provider API-key supplier. Three shapes: bare command string; single `{ command, provider, refreshIntervalMs, slowHelperWarningMs }` object; or array of provider-keyed objects. Executed without a shell; cached for `refreshIntervalMs` (default 5 min) or until a 401 is received |

Auth precedence per provider: per-provider helper → wildcard helper → environment variable → keyring → anonymous.

See [Configuring Providers & API Keys](../providers/configuration.md).

---

## Observability

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable_telemetry` | `bool` | `false` | Enable OpenTelemetry / cost emitter |

See [Telemetry & Cost](../observability/telemetry.md).

---

## Context-Window Management

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `auto_compact_threshold` | `float` or `null` | `0.75` | Pre-turn auto-compaction threshold as a utilization fraction in `[0, 1]`. `null` disables auto-compact |
| `micro_compact_enabled` | `bool` | `true` | Enable per-turn microcompact (LLM-free supersession pass) |
| `compact_strategy` | `string` | `"summarize"` | Strategy for `/compact` + threshold-autocompact: `"summarize"`, `"drop-oldest"`, or `"noop"` |
| `tool_result_cap_chars` | `integer` | `50000` | Global per-tool-result character cap. `0` disables |
| `min_cache_block_tokens` | `integer` | `1024` | Minimum estimated tokens on the last user message to place a conversation-level prompt-cache marker |

See [Context & Compaction](../memory/context-compaction.md).

---

## Managed Scope Control

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `parent_settings_behavior` | `"block"` \| `"augment"` | `"augment"` | When `"block"` is set in the **managed scope**, the managed scope moves to the top of the merge chain, overriding all user, project, local, and CLI settings. Has no effect when set in other scopes |

---

## Miscellaneous

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `additional_directories` | `string[]` | `[]` | Extra workspace roots for file and shell tools to consider |
| `claude_md_excludes` | `string[]` | `[]` | Glob patterns for CLAUDE.md paths to skip during discovery |
| `env` | `{ key → value }` | `{}` | Environment-variable overrides applied to every child process launched by caliban (tools, hooks, MCP servers). Deep-merged across scopes; highest-priority scope wins per key |
