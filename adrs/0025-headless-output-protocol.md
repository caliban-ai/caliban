# ADR 0025 · Headless `-p` mode + JSON output protocol

- **Status:** proposed
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

### Structured input is also NDJSON

`--input-format stream-json` makes stdin a chat transcript: each line is
either a `user` message or a `control/interrupt` frame. The driver
feeds the agent one message per turn. EOF gracefully drains.

This makes caliban scriptable from any language that can emit JSON
lines, without juggling pseudo-TTYs.

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
| 64   | `EX_USAGE` (bad flags) |
| 66   | `EX_NOINPUT` |
| 78   | `EX_CONFIGURATION_ERROR` (stdin > 10 MB; settings parse failure) |
| 124  | cancelled (SIGTERM / Ctrl-C) |
| 130  | `--max-turns` exceeded |
| 137  | `--max-budget-usd` exceeded |

CI tooling can distinguish "budget exhausted" from "real failure"
without parsing stdout.

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
