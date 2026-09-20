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

## Sandbox

Nested under the `[sandbox]` table. OS-sandbox posture for Bash commands run
under `--workspace` / `--restrict-paths` (#406, [ADR 0054](https://github.com/caliban-ai/caliban/blob/main/docs/adr/0054-sandbox-confinement-posture.md)).

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `sandbox.network` | `"deny" \| "allow"` | `"deny"` (when the fence is active) | Network-egress posture for sandboxed commands. `"deny"` blocks egress (loopback still works) — `git fetch`, `cargo`, `npm install`, `gh`, `curl` fail; `"allow"` restores full egress. Overridden by `--sandbox-network` on the CLI. This is the first user-reachable sandbox setting. |

```toml
[sandbox]
network = "allow"   # opt out of the default egress fence for this workspace
```

```admonish note title="Secret-scrubbing is automatic, not a settings key"
Under `--workspace`, sandboxed commands also run with a scrubbed environment —
secret-named variables (`*KEY*`, `*SECRET*`, `*TOKEN*`, `*PASSWORD*`,
`*CREDENTIAL*`, plus `OTEL_EXPORTER_OTLP_HEADERS`) are dropped from the child's
environment (#405). This is on by default and is not (yet) configured through
`settings.toml`.
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

---

## Agent-Loop Policy

The `[agent_loop]` group gathers **agent-loop policy** — the turn budget and the
spiral-containment guards — under one surface, distinct from the model-inference
knobs (`effort`, `thinking`) that govern a single call (ADR 0058). Every key is
optional and defaults to the value the loop has always used, so an absent group
changes nothing.

```toml
[agent_loop]
max_turns = 80              # or pass --max-turns (CLI wins)
no_edit_nudge_threshold = 8
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `max_turns` | `integer` (≥ 0) | `50` | Hard cap on agent-loop iterations. The `--max-turns` CLI flag overrides this (**precedence: CLI > settings > default**). `0` is a deterministic immediate max-turns stop. |
| `no_edit_nudge_threshold` | `integer` (≥ 0) | `10` | Consecutive zero-edit turns after which the loop injects one neutral "make the edit" nudge (#239). `0` disables the nudge. |
| `empty_turn_nudge_max` | `integer` (≥ 0) | `2` | Maximum consecutive degenerate (output-but-no-work) turns the loop nudges before letting the run end (#249). `0` disables the guard. |
| `max_turn_thinking_chars` | `integer` (≥ 0) | `262144` | Per-turn cap on cumulative *thinking* characters before the run stops with `ThinkingBudgetExhausted` (#62). `0` disables the guard. A backstop far above any legitimate single-turn reasoning. |
| `time_budget_secs` | `integer` (≥ 0) | `0` | Wall-clock **time budget** for the whole agent loop, in seconds. `0`/unset means *no deadline* (today's behavior). A positive value ends the run with `TimeBudgetExceeded` once that many seconds elapse — checked at the top of each turn, so the loop stops between turns rather than mid-turn. Headless: `subtype=time_budget`, exit code `75` (same graceful-bound class as max-turns). |
| `cost_budget_usd` | `number` (≥ 0) | *(none)* | **Cost budget** for the whole agent loop, in USD. Unset or `≤ 0` means *no cost cap*. A positive value ends the run with `CostBudgetExceeded` once the run's accumulated estimated cost (usage × the rate card in `caliban-telemetry`) reaches it — checked between turns. Works in TUI and headless (unlike the CLI-only `--max-budget-usd`). Headless: `subtype=budget_exceeded`, exit code `137`. Cost is `$0.00` (cap inert) for a model with no rate-card entry. |
| `verification_guidance` | `"off" \| "verify-when-cheap" \| "full"` | *(from profile)* | How much the system prompt **encourages the model to verify its own work**. `off` injects no guidance. `verify-when-cheap` encourages a quick reproduction when cheap. `full` strongly encourages reproduce-then-confirm. caliban never runs tests for the model — this only shapes the prompt. When unset, the effective value comes from the resolved **profile** (below); when set, it wins over the profile. |
| `profile` | `"cost-optimized" \| "quality-first" \| "local-guarded"` | *(adaptive)* | Named policy profile bundling the loop knobs (ADR 0058, B6). When unset, the default is chosen from **execution context**: a **local** provider (an OpenAI-compatible endpoint on a loopback/private/LAN host via `OPENAI_BASE_URL`) → `local-guarded`; **cloud** (first-party providers, or OpenAI at its public endpoint) → `cost-optimized`. Naming a profile overrides the context default. |

**Profiles.** Each profile sets a coherent verification posture (the lever eval sub-project A proved load-bearing, whose sign flips with model strength: +8pt on a strong cloud model, −16pt on a weak local one). Budgets (`max_turns` / `time_budget_secs` / `cost_budget_usd`) stay opt-in — a profile never imposes a surprising hard cap.

| Profile | `verification_guidance` | Intent |
|---------|-------------------------|--------|
| `cost-optimized` | `off` | Cloud default: cost-conservative (verification costs ~1.8×, so skip it). |
| `quality-first` | `full` | For a strong model: correctness over cost. |
| `local-guarded` | `verify-when-cheap` | Local default: verify (generation is ~free), paired with the wrong-path/divergence guard once that ships (B4, #664). |

Precedence: an explicit knob (e.g. `verification_guidance`) **>** the named `profile` **>** the context-adaptive default.

> `max_turns` and the three guards were the existing loop knobs; `time_budget_secs`
> (B2, #662), `cost_budget_usd` (B3, #663), `verification_guidance` (B5, #665),
> and `profile` + adaptive defaults (B6, #666) are the additions. The remaining
> child — the eval-gated wrong-path/divergence guard (B4, #664) — will complete
> the `local-guarded` pairing when it lands.

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
