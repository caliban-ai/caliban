# ADR 0025 · Headless `-p` mode + JSON output protocol

- **Status:** accepted
- **Date:** 2026-05-24
- **Spec:** `docs/superpowers/specs/2026-05-24-headless-mode-design.md`

## Context

caliban today only runs as an interactive ratatui TUI. Every potential
CI/scripting/devcontainer/GitHub-Actions consumer is blocked on a
non-interactive entry point. Claude Code's `-p` mode with
`--output-format text|json|stream-json` is the documented contract
those consumers use; mirroring it engine-to-engine is Tier-1 foundation
work. Full spec at
`docs/superpowers/specs/2026-05-24-headless-mode-design.md`; this ADR
records the architectural commitments only.

## Decision

### Headless is a sibling driver, not a fork of the TUI

`caliban -p` enters a `HeadlessDriver` that consumes the same
`AgentBuilder` + `Stream<Event>` surface from `caliban-agent-core`.
The TUI driver is unchanged. Both drivers compose the same hook
chain, permission rules, tool registry, and model router — the only
difference is the *encoder* that turns `Event`s into bytes.

Auto-headless when stdin is non-TTY or stdout is piped, unless
`--no-auto-print` is explicit. Explicit `--print` always wins.

### Three output formats, with `stream-json` as the contract surface

- **text:** the assistant's final message body to stdout. The minimum
  shape. Default.
- **json:** a single JSON object identical to the final `type: result`
  frame of stream-json. Suitable for `jq`-driven scripts that only
  care about the answer + cost.
- **stream-json:** NDJSON. First frame is `system/init` (model, tools,
  MCP servers, plugins, settings sources); per-turn frames are
  `tool_use`, `tool_result`, `content_block_delta` (when
  `--include-partial-messages`), `system/api_retry`, `user` (when
  `--replay-user-messages`), `hook_event` (when
  `--include-hook-events`); last frame is `type: result`.

Stream-json wraps closely around Claude Code's documented shape so
downstream consumers can drop in. Divergences (provider-specific token
fields, etc.) are documented in the README; we do not commit to
byte-identical compatibility because caliban is provider-agnostic.

#### Tool calls appear in two frames; the `message` frame is authoritative

Each successful tool call surfaces in the stream-json output as **two
frames**, by design:

```ndjson
{"type":"tool_use","id":"toolu_01ABC","name":"Glob","input":{"pattern":"**/*.toml"}}
{"type":"tool_result","tool_use_id":"toolu_01ABC","is_error":false,"content":[...]}
{"type":"message","role":"assistant","content":[
  {"type":"text","text":"Searching for TOML files…"},
  {"type":"tool_use","id":"toolu_01ABC","name":"Glob","input":{"pattern":"**/*.toml"}}
]}
```

1. A top-level **short `tool_use` frame** emitted at the moment the
   model finishes streaming the tool's input JSON (paired with a
   `tool_result` frame once the tool completes). This is a *progress
   indicator* — useful for live UIs that want to show "Glob is
   running" before the assistant's final message is assembled.
2. The same `tool_use` block embedded inside the subsequent
   **`message` frame** (full assistant message, content-block array)
   emitted at `TurnEnd`. This is the **authoritative record** — the
   serialized assistant turn as the agent would replay it from a
   session log.

Operators reconstructing the transcript from the stream should read
**the `message` frame** and treat the short `tool_use`/`tool_result`
frames as progress signal. Tools that count `tool_use` blocks must
not double-count (one short frame + one inside `message` = one tool
call, not two).

This mirrors Claude Code, where the assistant `message` event is the
authoritative full content and per-block progress frames are advisory.
The duplication is intentional; do not dedupe.

### Structured input is also NDJSON

`--input-format stream-json` makes stdin a chat transcript: each line is
either a `user` message or a `control/interrupt` frame. The driver
feeds the agent one message per turn. EOF gracefully drains.

This makes caliban scriptable from any language that can emit JSON
lines, without juggling pseudo-TTYs.

#### Input frame schema (canonical)

The simple, caliban-canonical shape:

```ndjson
{"type":"user","content":"hello"}
{"type":"user","content":[{"type":"text","text":"hello"}]}
{"type":"control","subtype":"interrupt"}
```

`user.content` accepts either a JSON string or an array of content
blocks (each `{"type":"text","text":"…"}`). Both flatten to the same
text on the way into the agent.

Unknown `type` values, malformed JSON, or extra unrecognized fields
on `user`/`control` frames are **hard parse errors** (exit 64,
`EX_USAGE`). The driver flushes any in-flight assistant frames first,
emits one final `result` frame with `subtype: "error"`, and only then
returns. This is to avoid the failure mode where an operator sends a
Claude-Code-shaped envelope (`{"type":"user","message":{"role":"user",
"content":[...]}}`) and the driver silently runs the agent with a
blank prompt because `serde` accepted the unknown `message` field.

#### `--input-format stream-json` requires stdin

When `--input-format stream-json` is in effect, an explicit prompt is
**incompatible** with the stream-json input path. The binary rejects
the combination at clap-parse time with `EX_USAGE` (exit 64) so
operators can't accidentally bypass the frame parser via a positional
prompt or `--prompt …`. The allowed entry points are:

- No prompt args at all (stdin is read as the NDJSON stream); or
- `-p -` / `--print -` / `--prompt -` (the `-` sentinel explicitly
  delegates to stdin and is treated as a no-op alongside
  `--input-format stream-json`).

### `--bare` is opt-in, not the CI default

`--bare` disables hooks, skills, plugins, MCP, auto-memory, and
CLAUDE.md auto-discovery. It's the documented "deterministic CI"
mode. Unlike Claude Code's stated direction of making it the default,
caliban's headless default keeps inheriting user/project settings —
operators must opt out explicitly. Rationale: caliban's first
deployments are mostly local-shell automation where inherited settings
are useful; CI runners are well-trained to add flags.

### Exit codes follow `sysexits.h` plus two budget signals

| Code | Meaning |
|------|---------|
| 0    | success |
| 1    | generic runtime error |
| 2    | tool/assistant error |
| 64   | `EX_USAGE` (bad flags) / malformed stream-json input |
| 66   | `EX_NOINPUT` (`--resume <missing>`, empty stream-json stdin) |
| 75   | `EX_TEMPFAIL` — `--max-turns` exceeded (F12 follow-up: was `130`, which collided with `128 + SIGINT`) |
| 78   | `EX_CONFIGURATION_ERROR` (stdin > 10 MB; settings parse failure) |
| 124  | cancelled (SIGTERM / Ctrl-C from the agent loop) |
| 130  | reserved for real `SIGINT` reaching the harness (`128 + 2`); the signal handler in `caliban/src/main.rs` exits with this on a second Ctrl-C |
| 137  | `--max-budget-usd` exceeded |

CI tooling can distinguish "budget exhausted" from "real failure"
without parsing stdout. **Update 2026-05-27 (F12):** `--max-turns`
exhaustion previously exited `130`, which is `128 + SIGINT` in the
UNIX convention — CI scripts reading `$?` reasonably concluded the
operator had Ctrl-C'd. It now exits `75` (`EX_TEMPFAIL`), distinct
from any signal-derived code. Consumers wanting the structured signal
should read the matching `result` frame's `subtype: "max_turns"`.

### Result-frame shape — structured fields for non-success runs

The final `result` frame's body depends on `subtype`:

- **`subtype: "success"`** — the assistant's reply lives in the
  `result` string field. Token/cost/turn totals are always present.
  Structured-output payloads are surfaced under `structured_output`
  when `--json-schema` succeeded. This is the load-bearing contract
  for downstream `jq` scripts and is **not** changed by the F7
  follow-up below.
- **All non-`success` subtypes** (`error`, `max_turns`,
  `budget_exceeded`, `cancelled`) — the `result` field is **omitted**;
  consumers must read the structured fields instead:
  - `last_assistant_text` — the most recent non-empty assistant text
    body the agent produced. `null` (field absent) when the run
    terminated before any assistant text landed. Distinct from the
    prior protocol, which set `result` to the concatenation of every
    streamed assistant fragment across the truncated run — a value
    that ranged from a stale plan preamble to literally `""` and
    couldn't be distinguished from a clean answer.
  - `tool_calls_seen` — running count of `ToolCallEnd` events
    observed across the entire run. Lets consumers tell an
    empty-but-active run (tool loop) from an empty-and-idle one.
  - `error` — populated for `subtype: "error"` only; carries the
    `StopCondition::ProviderError` / `HookDenied` / `CompactionFailed`
    / `Refusal` / `ContentFilter` / schema-validation message verbatim.

Pairs with the exit-code table above: the `result` frame's `subtype`
and the process exit code agree on what the terminal condition was,
so consumers can pick either signal.

### Cost accumulator lives in `caliban-agent-core::headless`

A `CostAccumulator` (per-`(provider, model)`) wraps each provider call
and accumulates USD against a static pricing table at
`caliban-agent-core/src/headless/pricing.json`. Pricing misses log a
WARN and treat cost as zero rather than failing — staleness is real,
and we'd rather emit "best-effort, cost may be undercount" than refuse
to run. Pricing table refreshes are by-hand PRs against the provider
websites; the `as_of` date surfaces in the `system/init` frame.

### Structured output via `--json-schema` uses provider-native first, falls back to validate-and-retry

For Anthropic / OpenAI native structured-output: the model router
issues the final reply with `json_schema` semantics, returns the parsed
object as `structured_output`. For providers without native support
(Ollama, some Google endpoints): prompt + validate + up-to-2 retries
with a "this didn't validate; retry, here's the error" follow-up. After
the retry budget, the result frame's `subtype` is `error`.

### Hook events are observable in headless mode

`--include-hook-events` attaches an in-process `HookSink` at the
outermost position in the hook chain. Each fired event becomes a
`hook_event` frame, including the router's decision and the
permissions layer's verdict separately. Async handlers emit two frames
(dispatch + completion) so observability isn't lost behind
fire-and-forget. This is the only headless flag that produces zero-cost
visibility into the new hook taxonomy (ADR 0024).

## Consequences

- **Positive:** Closes nearly all rows under "J. Headless / CI" in
  `docs/parity-gap-matrix.md` in one PR. Unblocks GitHub Actions and
  devcontainer integrations (each a separate sub-project, but neither
  is reachable without this). Makes caliban scriptable from any
  language. Cost accumulator gives operators (and the eventual `/usage`
  slash) a single source of truth for $ spent. Stream-json is the
  contract surface for everything downstream — once it's stable, we
  can iterate the TUI without breaking automation consumers.
- **Negative:** Pricing table is a maintenance hazard; staleness leads
  to silent undercounts. Stream-json diverges from Claude Code in
  per-provider token shapes — exact byte-for-byte parity isn't
  achievable while remaining provider-agnostic. Bare mode adds another
  axis of "what was actually configured during this run" that
  operators must reason about (mitigated by `system/init` surfacing
  the source chain). Structured-output fallback retry loop is bounded
  but adds two extra provider calls in the worst case.
- **Revisit if:** Downstream consumers demand byte-for-byte
  Claude-Code stream-json parity — we'd add a compat translator
  rather than rework the encoder. If pricing maintenance becomes
  untenable, host the table behind a hosted JSON file refreshed on a
  schedule. If `--bare` semantics need to expand (skipping
  `--system-prompt-file`, etc.), promote it to a typed
  `BareModeFlags` struct rather than a single bool.
