# Headless / print mode + JSON output — Design

**Date:** 2026-05-24
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** `adrs/0025-headless-output-protocol.md`

## Goal

Add a non-interactive "print" mode (`caliban -p "task"`) with
structured `text` / `json` / `stream-json` output, structured *input*
(`--input-format stream-json`), and the budget/turn knobs CI runners
need (`--max-turns`, `--max-budget-usd`, `--bare`, `--json-schema`,
`--include-partial-messages`, `--include-hook-events`,
`--replay-user-messages`). Headless mode is the prerequisite for every
CI / scripting / GitHub-Actions / devcontainer integration; it's
Tier-1 foundation work because nothing downstream is reachable without
it.

The goal is **engine parity, not surface parity**: caliban's headless
output is byte-for-byte close to Claude Code's `stream-json` so the
same downstream JSON consumers (Anthropic's `claude-code-action`,
custom scripts) can drop in. We diverge only where the underlying
caliban primitives differ (e.g. provider-specific token-counting
shapes) and document each divergence inline.

## Non-goals

- **GitHub Actions workflow / `claude-code-action` parity.** Once
  headless ships, building a `caliban-action` is mechanical — separate
  initiative.
- **Devcontainer feature.** Same — gated on headless landing.
- **`claude doctor` from shell.** A separate diagnostic command; tracked
  under K. Observability.
- **`--permission-prompt-tool` (MCP-driven permission UX).** Useful but
  niche; lands once MCP v2 is real (ADR 0023). We surface the flag and
  return an error stub.
- **Subscription-credit billing model.** Caliban is provider-agnostic;
  there's no Anthropic subscription mode to bill against. Cost-USD
  tracking is provider-attributable, not subscription-attributable.
- **Replay-from-transcript mode (`--from-pr`).** Reads a Claude Code
  transcript and re-runs it. Distinct enough to defer.

## Architecture

```
caliban (clap)
  │
  ├── interactive (default)  → existing TUI driver
  │
  └── -p / --print           → HeadlessDriver
                                  │
                                  ▼
                            ┌──────────────┐   ┌────────────────┐
                            │ InputReader  │──►│ AgentBuilder   │
                            │ (text or     │   │ (existing API) │
                            │  stream-json)│   └────────────────┘
                            └──────────────┘            │
                                                        ▼
                                                   Stream<Event>
                                                        │
                                                        ▼
                                              ┌──────────────────┐
                                              │ OutputEncoder    │
                                              │  - text          │
                                              │  - json (final)  │
                                              │  - stream-json   │
                                              └──────────────────┘
                                                        │
                                                        ▼
                                                     stdout

  cross-cutting:
    CostAccumulator  ← provider per-turn usage; backs --max-budget-usd
    TurnLimiter      ← decrements --max-turns; halts via CancellationToken
    BareModeGate     ← skips hooks/skills/plugins/MCP/auto-mem/CLAUDE.md
    HookEventSink    ← when --include-hook-events, attach a HookSink to the
                       HookRouter and emit each fired event as a stream frame
```

The HeadlessDriver is a sibling of the existing TUI driver, not a fork
of it; both consume the same `AgentBuilder` + `Stream<Event>` surface
from `caliban-agent-core`. New code is concentrated in
`caliban/src/headless/` (binary crate) — `caliban-agent-core` gains a
small `headless::` module for the cost accumulator and turn limiter
(reusable from any embedder), but the encoding layer stays in the
binary where it can depend on `clap` + `serde_json`.

## Crate structure (deltas only)

```
caliban/                                  # binary crate
├── src/
│   ├── main.rs                           # add CLI flags; dispatch interactive vs headless
│   ├── headless/                         # NEW module
│   │   ├── mod.rs                        # HeadlessDriver::run
│   │   ├── cli.rs                        # clap structs: HeadlessArgs, BareMode, OutputFormat
│   │   ├── input.rs                      # stdin reader (text / stream-json); 10 MB cap
│   │   ├── encoder/                      # output encoders
│   │   │   ├── mod.rs                    # OutputEncoder enum
│   │   │   ├── text.rs                   # plain text final result
│   │   │   ├── json.rs                   # single final JSON object
│   │   │   └── stream_json.rs            # NDJSON event stream
│   │   ├── schema.rs                     # --json-schema parse + validate
│   │   ├── exit.rs                       # exit-code mapping
│   │   └── hooks_sink.rs                 # bridges HookRouter → stream-json
│   └── tui.rs                            # unchanged
crates/caliban-agent-core/
├── src/
│   ├── headless/                         # NEW
│   │   ├── mod.rs
│   │   ├── cost.rs                       # CostAccumulator (per-provider USD)
│   │   ├── turn_limiter.rs               # max-turns enforcement
│   │   └── bare_mode.rs                  # BareModeGate config struct
│   └── lib.rs                            # re-export `headless` types
```

## Config / CLI schema

```
caliban [SUBCOMMAND] [GLOBAL FLAGS] [HEADLESS FLAGS] [PROMPT...]

Headless-mode flags:

  -p, --print                      Enable headless mode. Required for non-TTY.
      --output-format <FMT>        text | json | stream-json     (default: text)
      --input-format  <FMT>        text | stream-json            (default: text)
      --max-turns     <N>          Hard cap on agent turns       (default: unlimited)
      --max-budget-usd <FLOAT>     Halt when accumulated cost ≥ N (default: unlimited)
      --bare                       Skip hooks/skills/plugins/MCP/auto-memory/CLAUDE.md
      --json-schema   <FILE|JSON>  Force structured final output matching schema
      --include-partial-messages   Emit assistant text deltas as separate frames
      --include-hook-events        Emit a frame per fired hook event
      --replay-user-messages       Echo user messages as stream frames
      --no-session-persistence     Do not write to ~/.caliban/sessions/
      --session-id    <ID>         Reuse an existing session-id (resume)
      --fallback-model <NAME>      Provider+model to use on primary failure
      --allowed-tools <CSV>        Tool allowlist (overrides settings)
      --disallowed-tools <CSV>     Tool denylist
      --permission-mode <MODE>     default|acceptEdits|plan|auto|dontAsk|bypassPermissions
      --append-system-prompt <S>   Append text to the system prompt
      --append-system-prompt-file <F>  Same, from a file
```

`PROMPT...` is the user message. If absent and `--input-format text`,
read stdin (capped at 10 MB; larger → exit code 78
`EX_CONFIGURATION_ERROR`). If `--input-format stream-json`, ignore
positional args and stream user messages from stdin (see below).

### Bare mode semantics

`--bare` disables, in order:

1. **Hooks** — `disable_all_hooks` set true in the in-memory settings;
   in-process `PermissionsHook` still runs against `default_rules()`
   unless `--permission-mode dontAsk` is also set.
2. **Skills** — `caliban-skills::load_all` is skipped; only built-in
   tools are registered.
3. **Plugins** — when ADR 0030 lands, plugin discovery is skipped.
   No-op until then.
4. **MCP** — `McpClientManager::start` is skipped; no servers are
   spawned and no `mcp__*` tools register.
5. **Auto-memory** — `caliban-memory::auto::load` is skipped.
6. **CLAUDE.md auto-discovery** — neither ancestor walk nor nested
   on-demand load runs. `--append-system-prompt[-file]` is still honored.

Bare mode is the CI default in spirit but not by default — operators
opt in. We do *not* attempt Claude Code's "becoming the default in a
future release" semantic; caliban's default headless behavior is to
inherit user/project settings.

## Stream event shape

NDJSON: one JSON object per line, `\n`-terminated, no trailing comma,
UTF-8. Wraps closely around Claude Code's `stream-json` for
drop-in-ish compatibility.

### `system/init` (always first when stream-json)

```json
{
  "type": "system",
  "subtype": "init",
  "session_id": "01HW...",
  "model": { "provider": "anthropic", "name": "claude-sonnet-4-7" },
  "tools": ["Bash","Read","Write","Edit","Grep","Glob","WebFetch","TodoWrite","Skill","AgentTool","EnterPlanMode","ExitPlanMode","mcp__linear__list_issues", "..."],
  "mcp_servers": [{ "name": "linear", "transport": "stdio", "status": "ok", "tools": 18 }],
  "plugins": [],
  "plugin_errors": [],
  "permission_mode": "default",
  "cwd": "/home/me/proj",
  "settings": { "source_chain": ["managed", "user", "project"], "bare_mode": false }
}
```

### `system/api_retry`

```json
{ "type": "system", "subtype": "api_retry",
  "attempt": 2, "max_retries": 5,
  "retry_delay_ms": 1500,
  "error_status": 529,
  "error_category": "overloaded" }
```

`error_category` ∈ `overloaded | rate_limit | timeout | network |
server_error | other`. Maps from provider-specific errors via
`caliban-provider::error`.

### Assistant text deltas (`--include-partial-messages`)

```json
{ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hello" } }
```

Falls back to full assistant messages without `--include-partial-messages`.

### `tool_use`

```json
{ "type": "tool_use", "id": "toolu_01ABC", "name": "Bash", "input": { "command": "ls" } }
```

### `tool_result`

```json
{ "type": "tool_result", "tool_use_id": "toolu_01ABC",
  "is_error": false,
  "content": [{ "type": "text", "text": "Cargo.toml ..." }] }
```

### `user` (when `--replay-user-messages`)

```json
{ "type": "user", "content": [{ "type": "text", "text": "fix the bug" }] }
```

### `hook_event` (when `--include-hook-events`)

```json
{ "type": "hook_event",
  "event": "PreToolUse",
  "matcher": "Bash",
  "handler": { "type": "command", "command": "guard-rm.sh" },
  "decision": "allow",
  "duration_ms": 12 }
```

### Final `result`

The last frame of any stream-json run. Also the single body in
`--output-format json`.

```json
{
  "type": "result",
  "session_id": "01HW...",
  "subtype": "ok",
  "result": "All tests pass.",
  "structured_output": { "ok": true, "failures": [] },
  "total_cost_usd": 0.0473,
  "total_input_tokens": 12450,
  "total_output_tokens": 894,
  "model_breakdown": [
    { "provider": "anthropic", "model": "claude-sonnet-4-7",
      "input_tokens": 12450, "output_tokens": 894, "cost_usd": 0.0473 }
  ],
  "turns": 3,
  "stop_reason": "end_turn",
  "duration_ms": 12380
}
```

`subtype` ∈ `ok | error | max_turns | max_budget | cancelled`.
`structured_output` only present when `--json-schema` was set and
parsing succeeded.

## Structured input (`--input-format stream-json`)

Stdin is NDJSON. Each line is one of:

```json
{ "type": "user", "content": [{ "type": "text", "text": "first message" }] }
{ "type": "user", "content": [{ "type": "text", "text": "follow-up" }] }
{ "type": "control", "subtype": "interrupt" }
```

The driver feeds user messages into the agent in order. Each subsequent
user message starts a new turn; the agent runs to completion, then
waits for the next stdin line. `{"type":"control","subtype":"interrupt"}`
cancels the in-flight turn (best-effort; tools may not honor it). EOF
on stdin gracefully terminates after the in-flight turn finishes.

## Structured output (`--json-schema`)

The flag value is either a path to a JSON Schema file (`.json`) or an
inline JSON-Schema literal. The agent's final response is fed to the
provider with `tool_choice = { type: "json_schema", schema }` semantics
(via `caliban-model-router`'s structured-output adapter). The result
must validate against the schema; failure populates the result frame
with `subtype: "error"` and `result: "<schema validation error>"`.

For providers without native structured-output (Ollama, some Google
endpoints), the driver falls back to prompting + jsonschema validation
of the parsed reply, with up to 2 self-correction retries.

## Exit codes

| Code | Meaning                                              |
|------|------------------------------------------------------|
| 0    | Success (`subtype = ok`)                              |
| 1    | Generic runtime error (uncategorized)                 |
| 2    | Tool error or assistant failure (`subtype = error`)   |
| 64   | `EX_USAGE` — bad flags or invalid args                |
| 66   | `EX_NOINPUT` — required input absent (no prompt, no stdin) |
| 78   | `EX_CONFIGURATION_ERROR` — settings parse failure, stdin > 10 MB |
| 124  | Cancelled (Ctrl-C / external SIGTERM)                 |
| 130  | `--max-turns` exceeded (`subtype = max_turns`)        |
| 137  | `--max-budget-usd` exceeded (`subtype = max_budget`)  |

Codes 130/137 are nudges; downstream CI tooling can tell "budget"
from "turn cap" from "real error". 1/2 follow `sysexits.h` convention.

## Cost accumulator

```rust
// crates/caliban-agent-core/src/headless/cost.rs

#[derive(Debug, Default)]
pub struct CostAccumulator {
    pub total_usd: f64,
    pub model_breakdown: Vec<ModelCost>,
}

pub struct ModelCost {
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_create_tokens: u64,
    pub cost_usd: f64,
}

impl CostAccumulator {
    pub fn record(&mut self, usage: &caliban_provider::Usage, pricing: &Pricing);
    pub fn at_limit(&self, max_usd: Option<f64>) -> bool;
}
```

Pricing is keyed by `(provider, model)` from a static table at
`caliban-agent-core/src/headless/pricing.json` (refreshed by hand from
the provider websites; mismatch logs WARN and treats cost as `0.0`
rather than failing). The table covers the providers caliban
currently supports (`anthropic`, `openai`, `google`, `ollama`-local
which is always `0.0`).

The accumulator is hooked into `Provider::stream` via a small wrapper
that intercepts the final `Usage` of each turn; integration is a single
line in `AgentBuilder::with_cost_accumulator`.

## Turn limiter

```rust
pub struct TurnLimiter {
    max: Option<u32>,
    seen: AtomicU32,
    cancel: CancellationToken,
}

impl TurnLimiter {
    pub fn before_turn(&self) -> Result<(), TurnLimitReached> { … }
}
```

Hooked via the existing `before_turn` hook. On limit reached, the
limiter triggers `cancel`, the agent loop drains, and the encoder emits
`subtype: "max_turns"`.

## Routing between drivers

```rust
// caliban/src/main.rs

let args = Cli::parse();
let headless = args.print
    || !std::io::stdin().is_terminal()
    || args.output_format.is_some();

if headless {
    headless::HeadlessDriver::new(args).run().await
} else {
    tui::TuiDriver::new(args).run().await
}
```

Auto-headless when stdout is piped or stdin is non-TTY (mirrors Claude
Code), unless `--no-auto-print` is set. The TUI driver is unchanged.

## TUI integration (intentional)

When the TUI is the driver, `--include-hook-events` is silently
ignored (the TUI already renders hook activity). All other flags
remain interactive-mode-relevant (`--max-turns`, `--max-budget-usd`).

## Testing strategy

~15 enumerated tests under `caliban/tests/headless_*.rs` and
`caliban-agent-core/src/headless/*` unit tests:

1. **`--print "hello"` → exits 0; stdout is the assistant's final text.**
2. **`--output-format json` → stdout is one JSON object with `result`, `total_cost_usd`, `session_id`.**
3. **`--output-format stream-json` → first frame is `system/init`; last frame is `type:result`.**
4. **`--input-format stream-json` reads two user messages, agent answers each.**
5. **`--input-format stream-json` + `control/interrupt` cancels the current turn; agent resumes on next user message.**
6. **`--max-turns 2` exits 130 with `subtype: max_turns` after 3rd turn would start.**
7. **`--max-budget-usd 0.0001` exits 137 after first paid turn.**
8. **`--bare` skips hook router, skills, MCP, auto-memory, CLAUDE.md.** Snapshot the `system/init` frame's `settings.bare_mode = true` + empty `mcp_servers`.
9. **`--bare` still honors `--append-system-prompt`.**
10. **`--json-schema` enforces the schema on the final assistant reply.** Mock provider returns matching object; assert `structured_output` populated.
11. **`--json-schema` on non-native provider falls back to validate-then-retry.**
12. **`--include-partial-messages` emits `content_block_delta` frames.**
13. **`--include-hook-events` emits `hook_event` frames for each PreToolUse / PostToolUse fired.** Uses a fixture `hooks.toml` with one command handler.
14. **`--replay-user-messages` emits `user` frames for stdin input.**
15. **Stdin > 10 MB → exit 78.**
16. **Auto-headless when stdout is piped.** Bash spawns `caliban "hi" | cat`; asserts no terminal-control sequences in output.
17. **`api_retry` frame emitted on 529.** Mock provider returns 529 once; assert frame shape.
18. **Cancellation via SIGTERM produces a final `subtype: cancelled` frame and exit 124.**
19. **Cost accumulator sums correctly across provider switches mid-session** (model router picks `FastClassifier` then `MainLoop`).
20. **`--no-session-persistence` skips writing to `~/.caliban/sessions/`.**

Cross-cutting integration tests reuse the in-process mock provider
(`caliban-provider::mock`) so no real API calls fire in CI.

## Integration with `AgentBuilder` + `Stream`

The existing `AgentBuilder` already supports `with_hooks`,
`with_tool_registry`, `with_messages`, `with_cancel_token`. We add:

```rust
impl AgentBuilder {
    pub fn with_cost_accumulator(self, cost: Arc<Mutex<CostAccumulator>>) -> Self;
    pub fn with_turn_limit(self, limit: Option<u32>) -> Self;
    pub fn with_bare_mode(self, bare: BareModeGate) -> Self;
}
```

The headless driver constructs an `AgentBuilder` from the parsed CLI,
wires these in, then drives `stream()`'s `Stream<Event>` through the
chosen encoder until completion. The TUI driver doesn't use these
additions (yet) — but they're available for it if `--max-turns` ever
becomes user-meaningful in interactive mode.

## Hook events vs. `--include-hook-events`

The `--include-hook-events` flag attaches a `HookSink` (in-process
`Hooks` impl) at the *outermost* position in the hook composition
chain. It observes every event the router and inner hooks see and
emits a JSON frame. Crucially:

- It runs **after** `HookRouter` but **before** `PermissionsHook`, so
  the JSON frame reports the router's decision plus the permissions
  layer's verdict separately (`router_decision`, `permission_action`).
- Async hooks emit a frame on **both** dispatch and completion (so
  observability isn't lost behind fire-and-forget).
- Frames carry `event`, `matcher`, `handler` summary, `decision`,
  `duration_ms`, and (when relevant) `tool_use_id`.

## Risks

- **Output format drift from Claude Code.** Our `stream-json` is close
  but not byte-identical — provider-specific token fields differ.
  Mitigation: document divergences in the README; provide a `claude-`
  compat translator in `docs/examples/` if downstream demand emerges.
- **Cost table staleness.** Pricing changes regularly; an outdated
  table silently undercounts. Mitigation: log WARN on missing
  `(provider, model)` and include the table's `as_of` date in
  `system/init`.
- **Bare-mode footgun.** Operators may forget `--bare` in CI and inherit
  user-level CLAUDE.md or hooks, producing non-reproducible runs.
  Mitigation: `system/init` frame surfaces all loaded sources;
  `caliban -p` prints a warning when interactive-only configs are
  active in a non-TTY context.
- **Structured output retry loops.** A pathological model may never
  produce valid JSON. Mitigation: 2-retry cap, then fail with
  `subtype: error`.
- **Stdin cap is arbitrary.** 10 MB matches Claude Code; some workloads
  may want more. Mitigation: documented; can be raised via
  `--max-stdin-bytes` if demand emerges (out of scope for v1).
- **Auto-headless detection on weird terminals.** Some CI runners
  present as TTYs. Mitigation: explicit `--print` always wins;
  document the precedence.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean; `cargo fmt --all -- --check`
  clean.
- ≥20 new tests across `caliban/tests/headless_*.rs` and
  `caliban-agent-core/src/headless/*`, all passing.
- `caliban -p "hello"` exits 0 and prints the assistant's reply.
- `caliban -p --output-format stream-json "hello"` emits a valid
  NDJSON stream beginning with `system/init` and ending with
  `type: result`.
- `caliban --output-format json "hello"` exits 0 and prints a single
  JSON object on stdout, suitable for `jq`.
- All rows under **J. Headless / CI** in
  `docs/parity-gap-matrix.md` move 🔴 → ✅ except "GitHub Actions
  workflow" and "Devcontainer feature" (both gated on this landing,
  but they're separate sub-projects). `claude doctor from shell` also
  remains 🔴 (separate diagnostic command).
- README's "Headless mode" section documents the stream-json shape,
  exit codes, and the `--bare` opt-in.
- ADR 0025 in `accepted` status alongside this implementation.
