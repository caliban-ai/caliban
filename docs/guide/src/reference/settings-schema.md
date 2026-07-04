# Settings Schema

This page is a typed, structured listing of every key in the caliban settings file. For a narrative explanation of how scopes interact, how to locate each file, and how to edit settings interactively, see [Settings Reference](../configuration/reference.md) and [Settings Layering](../configuration/settings-layering.md).

Settings files are TOML by primary convention (`settings.toml` / `settings.local.toml`); JSON is accepted on import only. Unknown top-level keys are tolerated for forward-compat.

---

## Model / Agent

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `agent` | `string` | — | Agent profile name (sub-agent dispatch hint). |
| `model` | `string \| { provider, name }` | — | Primary model. Bare string (e.g. `"claude-sonnet-4-6"`) or qualified object `{ provider = "anthropic", name = "..." }`. |
| `fallback_model` | `string \| { provider, name }` | — | Fallback model when the primary errors. Same shapes as `model`. |
| `model_overrides` | `{ string → string }` | `{}` | Per-route model overrides. Keys are router route names (e.g. `"fast-classifier"`); values are model ids. |
| `effort` | `"low" \| "medium" \| "high" \| "max" \| "auto"` | — | Default reasoning effort level. |

---

## Permissions

Nested under the `[permissions]` table.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `permissions.allow` | `string[]` | `[]` | Patterns that auto-allow (legacy bucket form). |
| `permissions.ask` | `string[]` | `[]` | Patterns that prompt the user (legacy bucket form). |
| `permissions.deny` | `string[]` | `[]` | Patterns that hard-deny (legacy bucket form). |
| `permissions.rules` | `RuleSpec[]` | `[]` | Ordered v2 rule array. When non-empty, takes precedence over the three buckets above. Source order is preserved (first match wins). |
| `permissions.enforce` | `boolean` | — | When `true`, refuse `--no-permissions` / bypass mode at startup. |
| `permissions.default_mode` | `string` | — | Initial permission mode at session start. Values: `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`. |
| `permissions.audit_log` | `boolean` | `true` | Append-only permission-decision log toggle. |

**`RuleSpec` fields** (used in `permissions.rules` entries):

| Field | Type | Description |
|-------|------|-------------|
| `pattern` | `string` | Glob matching `Tool` or `Tool:first-arg-glob` (e.g. `"Bash:git *"`). |
| `action` | `"allow" \| "ask" \| "deny"` | Decision for matching calls. |
| `comment` | `string` (optional) | Human-readable comment shown in `/permissions`. |
| `reason` | `string` (optional) | Deny reason shown to the operator and logged. |
| `expires_at` | ISO 8601 timestamp (optional) | Rule is skipped after this time. |

```toml
[permissions]
# v2 ordered rules (preferred)
[[permissions.rules]]
pattern = "Bash:git *"
action  = "allow"
comment = "git commands OK"

[[permissions.rules]]
pattern = "Bash:rm *"
action  = "deny"
reason  = "use git revert"

[[permissions.rules]]
pattern = "*"
action  = "ask"
```

---

## Hooks

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `hooks` | `{ string → … }` | `{}` | Raw hook event → handler list map (passed to `caliban_agent_core::HooksConfig`). |
| `disable_all_hooks` | `boolean` | `false` | Kill-switch: disable every external hook handler. |
| `allow_managed_hooks_only` | `boolean` | `false` | When `true`, only managed-scope hooks fire. |
| `allowed_http_hook_urls` | `string[]` | `[]` | HTTP-hook URL allowlist (glob patterns). |
| `http_hook_allowed_env_vars` | `string[]` | `[]` | Environment variable names that HTTP hooks are permitted to read. |

---

## MCP Servers

Under `[mcp_servers.<name>]`. Each entry configures one MCP server.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `type` | `"stdio" \| "http" \| "sse"` | `"stdio"` | Transport selector. Also accepted as `transport` (TOML alias). |
| `command` | `string` | `""` | Executable command (stdio only). |
| `args` | `string[]` | `[]` | Argv after the command (stdio only). |
| `env` | `{ string → string }` | `{}` | Environment variables injected for the server process (stdio only). |
| `cwd` | `string` | — | Working directory override (stdio only). |
| `url` | `string` | — | Absolute `http://` or `https://` URL (http/sse transports). |
| `headers` | `{ string → string }` | `{}` | Static request headers (http/sse only). |
| `oauth` | `"off" \| "auto" \| "manual"` | `"off"` | OAuth mode (http/sse only). |
| `disabled` | `boolean` | `false` | Mark this server disabled without removing the entry. |
| `permissions` | object | — | Per-server permission scoping (composes with global rules). |

```toml
[mcp_servers.linear]
command = "npx"
args    = ["-y", "@linear/mcp-server"]
```

The [gonzalo](https://github.com/caliban-ai/gonzalo) code-graph server is a
stdio server consumed the same way — point `command` at the `gonzalo-mcp`
binary and pass the store root (populated with `gonzalo index`) via `env`. Its
tools then surface as `mcp__gonzalo__{search,node,callers,callees,impact,explore}`:

```toml
[mcp_servers.gonzalo]
command = "gonzalo-mcp"
[mcp_servers.gonzalo.env]
GONZALO_ROOT = "/path/to/graph-store"
```

---

## Router

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `router` | object | — | Router config (opaque; schema owned by `caliban-model-router`). Use `caliban.toml` `[router]` for the primary router config. |

---

## Memory

Nested under `[memory]`.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `memory.auto_memory_enabled` | `boolean` | — | Enable / disable auto-memory topic files. |
| `memory.auto_memory_directory` | `string` | Platform default | Directory for auto-memory topic files. |
| `memory.cap_tokens_auto` | `integer` | — | Token budget cap for the auto-memory tier. |
| `memory.cap_tokens_claude_md` | `integer` | — | Token budget cap for the CLAUDE.md tier. |
| `memory.cap_tokens_combined` | `integer` | — | Combined token budget cap across all tiers. |

---

## Plugins

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `plugins` | object | — | Plugin manager knobs (schema owned by `caliban-plugins`). |

---

## UI

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `output_style` | `string` | — | Active output-style name (see [Output Styles](../extending/output-styles.md)). |
| `editor_mode` | `string` | — | Input editing mode: `"vim"` or `"emacs"`. |
| `view_mode` | `string` | — | TUI layout mode: `"compact"` or `"expanded"`. |
| `statusLine.command` | `string` | — | **Required when `statusLine` is set.** Shell command whose stdout prefixes the status bar. |
| `statusLine.timeout_ms` | `integer` (50–5000) | — | Maximum ms to wait for the status-line script. |
| `statusLine.padding` | `integer` (0–8) | — | Spaces of padding around the custom segment. |
| `tui` | object | — | TUI knobs. Known sub-key: `showCostInStatusline` (`boolean`). |

```admonish tip title="statusLine casing"
`statusLine` uses camelCase on disk for Claude Code compatibility. The TOML alias `status_line` is also accepted.
```

---

## Auth

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `api_key_helper` | `string \| object \| object[]` | — | Provider API-key supplier(s). Bare string = command path; object = `{ command, provider?, refreshIntervalMs?, slowHelperWarningMs? }`; array = per-provider list. |

---

## Observability

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable_telemetry` | `boolean` | — | OTel / cost emitter toggle. |

---

## Context-Window Management

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `auto_compact_threshold` | `number` (0–1) or `null` | `0.75` | Pre-turn autocompaction threshold (context utilization fraction). `null` disables autocompact. |
| `micro_compact_enabled` | `boolean` | `true` | Enable the per-turn microcompact (LLM-free supersession) pass. |
| `compact_strategy` | `string` | `"summarize"` | Strategy used by `/compact` and threshold-autocompact: `"summarize"` (LLM summary of older turns — preserves context, incurs a provider call), `"drop-oldest"` (LLM-free; drops oldest turns past the recent window), or `"noop"` (disable). |
| `tool_result_cap_chars` | `integer` (≥ 0) | `50000` | Global per-tool-result cap in characters. `0` disables. |
| `min_cache_block_tokens` | `integer` (≥ 0) | `1024` | Minimum estimated tokens on the last user message to merit the conversation-level cache marker. |

---

## Stream Watchdog

The streaming idle watchdog aborts a run when a response goes silent for too
long. It distinguishes two phases: **prefill** (before the first output token —
where a slow local model with a large context may legitimately pause) and
**mid-content** (after the first token, where a long gap signals a genuine
stall).

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `stream_idle_timeout_ms` | `integer` (≥ 0) | `90000` | Silence (ms) tolerated **after** the first output token before aborting a stalled stream. `0` disables the watchdog entirely. |
| `stream_prefill_timeout_ms` | `integer` (≥ 0) | `300000` | Silence (ms) tolerated **before** the first output token (slow local-model prefill). `0` falls back to the idle window. Frontier models prefill in milliseconds and never approach this. |

For ollama, both budgets can also be overridden per-run via environment
variables (see the [environment variables reference](env-vars.md)) so eval and
emulated runs can widen the window without editing settings.

---

## Enterprise (Managed Scope)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `parent_settings_behavior` | `"block" \| "augment"` | `"augment"` | When `"block"` in the managed scope, the managed layer flips to the top of the merge chain (enterprise lockdown). |

---

## Miscellaneous

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `additional_directories` | `string[]` | `[]` | Extra workspace roots to consult for CLAUDE.md and skills. |
| `claude_md_excludes` | `string[]` | `[]` | Glob patterns to exclude from CLAUDE.md discovery (`claudeMdExcludes`). |
| `env` | `{ string → string }` | `{}` | Environment-variable overrides applied to child processes spawned by caliban. |
