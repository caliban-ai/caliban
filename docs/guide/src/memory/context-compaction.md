# Context & Compaction

Every provider has a finite context window. Caliban tracks utilization in real
time and provides several tools — automatic and manual — to keep long sessions
healthy without losing important history.

## Context tracking

Caliban maintains a `ContextWindow` counter that accumulates token usage from
every provider response. This is independent of the telemetry subsystem: the
`/context` command and the TUI status-bar percentage work for all users
regardless of whether `CALIBAN_ENABLE_TELEMETRY` is set.

```text
/context
  input tokens used : 62 430 / 200 000  (31%)
  output tokens used: 4 812
  ⚠ approaching limit (warn threshold: 80%)
```

`/context` shows a per-message-kind breakdown and warns when utilization
reaches 80%. See [Telemetry & Cost](../observability/telemetry.md) for OTLP
export of context metrics.

## Auto-compaction

When the context-window utilization reaches `auto_compact_threshold`, caliban
automatically runs the configured compactor before the next turn. The default
threshold is `0.75` (75% utilization).

Configure in `settings.toml`:

```toml
auto_compact_threshold = 0.75   # 0.0–1.0; unset or null disables autocompact
```

Set `auto_compact_threshold` to `null` (or omit it) to disable autocompact
entirely and rely on manual `/compact` invocations.

## Micro-compaction

Micro-compaction is an LLM-free per-turn pass that supersedes stale
`ToolResult` blocks in the conversation history without making any API calls.

The logic is per-tool:

| Tool                   | Supersession key              |
|------------------------|-------------------------------|
| `Read`                 | File path                     |
| `Grep`, `Glob`         | Exact argument string         |
| `WebFetch`             | URL                           |
| `Bash`                 | Never superseded              |

When a newer result for the same key exists, the older result block is replaced
with a `[superseded: <tool>(<key>)]` placeholder, keeping message structure
intact but recovering tokens.

Enable or disable in `settings.toml`:

```toml
micro_compact_enabled = true    # default: true
```

## Manual `/compact`

`/compact` triggers an immediate compaction of the current conversation through
the configured `Compactor` (the same path used by autocompact). A
`compact.event` log entry is emitted and a `compact.event` metric is recorded if
telemetry is enabled.

```text
/compact
```

No flags. The compactor strategy (summarizing vs. micro) is determined by the
active configuration — see [Hooks](../extending/hooks.md) for the
`PreCompact` / `PostCompact` hook events that fire around each compaction.

## `/clear`

`/clear` resets the conversation to an empty state and zeroes the
`ContextWindow` counter. The session file is updated. Use it to start a
fresh sub-task without opening a new session.

```text
/clear
```

## PreCompact and PostCompact hooks

Caliban fires `PreCompact` before compaction begins and `PostCompact` after it
completes. These hook events are available to external scripts and MCP handlers.

```toml
# In settings.toml [hooks]
[hooks]
PreCompact  = [{ type = "command", command = "echo compacting…" }]
PostCompact = [{ type = "command", command = "notify-send 'compact done'" }]
```

See [Hooks](../extending/hooks.md) for the full hook configuration reference.

## Prompt caching

Caliban uses Anthropic-style prompt caching by default to reduce cost on
repeated turns. A cache marker is placed on the last user message when its
estimated token count meets the minimum threshold.

| Setting / flag             | Default | Description                                                  |
|----------------------------|---------|--------------------------------------------------------------|
| `--no-prompt-cache`        | off     | Disable prompt caching for this run                          |
| `CALIBAN_NO_PROMPT_CACHE`  | unset   | Same as `--no-prompt-cache` via environment variable         |
| `min_cache_block_tokens`   | —       | Minimum tokens on the last user message to merit a cache marker |

Configure `min_cache_block_tokens` in `settings.toml`:

```toml
min_cache_block_tokens = 1024   # omit to use the upstream default
```

```admonish tip title="When to disable prompt caching"
Use `--no-prompt-cache` during development when you want to measure raw latency
without cache effects, or when debugging unexpected responses that might be
served from a stale cache hit.
```

## Tool result size cap

Caliban can cap the character length of individual tool results before they are
appended to the conversation. This prevents a single large `Read` or `Bash`
output from consuming a disproportionate share of the context window.

```toml
tool_result_cap_chars = 65536   # 0 disables the cap (default)
```

## Summary of relevant settings

| Setting key                | Type    | Default | Description                                        |
|----------------------------|---------|---------|----------------------------------------------------|
| `auto_compact_threshold`   | float   | `0.75`  | Utilization (0–1) that triggers autocompact; `null` disables |
| `micro_compact_enabled`    | bool    | `true`  | Enable the LLM-free per-turn supersession pass     |
| `min_cache_block_tokens`   | integer | —       | Minimum tokens to place the prompt cache marker    |
| `tool_result_cap_chars`    | integer | `0`     | Per-result character cap; `0` disables             |
