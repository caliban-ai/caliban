# The stream-json Protocol

`--output-format stream-json` is caliban's full automation contract. It emits newline-delimited JSON (NDJSON) to stdout, one frame per line, in a well-defined order. Downstream programs parse the stream with any JSON library and route on the `type` (and `subtype`) fields.

The protocol mirrors Claude Code's stream-json shape closely enough that most existing consumers work with minimal changes, while remaining provider-agnostic — token field names and cost breakdowns differ by provider and are not byte-identical to Claude Code.

## Output frame types

### `system/init` — first frame of every run

Emitted before any agent activity begins.

```json
{
  "type": "system",
  "subtype": "init",
  "session_id": "a3f7c2d1-...",
  "model": "anthropic/claude-sonnet-4-6",
  "tools": ["Bash", "Edit", "Glob", "Grep", "Read", "Write"],
  "plugins": [],
  "settingSources": ["managed", "user", "project"],
  "mcp_servers": [],
  "bare_mode": false,
  "cwd": "/home/ci/repo",
  "permission_mode": "acceptEdits"
}
```

`settingSources` uses `camelCase` for Claude Code parity. `permission_mode` values are `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`, or `"disabled"` (when `--no-permissions` is in effect).

### `system/api_retry`

Emitted when the provider triggers a retry (rate-limit, overload, transient network error).

```json
{
  "type": "system",
  "subtype": "api_retry",
  "attempt": 2,
  "max_retries": 5,
  "retry_delay_ms": 1500,
  "error_status": 529,
  "error_category": "overloaded"
}
```

`error_category` values: `overloaded`, `rate_limit`, `timeout`, `network`, `server_error`, `other`.

### `user` — echo of the user prompt

Only emitted when `--replay-user-messages` is set.

```json
{
  "type": "user",
  "content": [{"type": "text", "text": "fix the failing tests"}]
}
```

### `text` — incremental assistant text delta

Only emitted when `--include-partial-messages` is set.

```json
{"type": "text", "delta": "Here is the fix: "}
```

### `thinking` — incremental reasoning delta

Emitted under `--include-partial-messages` when the model streams reasoning content (extended thinking models).

```json
{"type": "thinking", "delta": "Let me check the test output…"}
```

### `tool_use` and `tool_result` — progress frames

Each tool invocation produces a `tool_use` frame (emitted once the model finishes streaming the tool's input JSON) immediately followed by a `tool_result` frame (emitted once the tool completes).

```json
{"type": "tool_use", "id": "toolu_01ABC", "name": "Bash", "input": {"command": "cargo test"}}
{"type": "tool_result", "tool_use_id": "toolu_01ABC", "is_error": false, "content": [{"type": "text", "text": "test result: ok. 42 passed"}]}
```

### `message` — full assistant message (authoritative)

Emitted at the end of each turn when `--include-partial-messages` is **not** set. When `--include-partial-messages` is set, text deltas stream via `text` frames instead and no `message` frame is emitted.

```json
{
  "type": "message",
  "role": "assistant",
  "content": [
    {"type": "text", "text": "All tests pass now."},
    {"type": "tool_use", "id": "toolu_01ABC", "name": "Bash", "input": {"command": "cargo test"}}
  ]
}
```

```admonish note title="Tool call duplication is intentional"
Each tool call appears in **both** a short `tool_use`/`tool_result` pair and inside the subsequent `message` frame's content array. The short pair is a progress indicator; the `message` frame is the authoritative record for transcript reconstruction. Do not deduplicate — count one tool call per `tool_use` frame, not two.
```

### `hook_event`

Only emitted when `--include-hook-events` is set.

```json
{
  "type": "hook_event",
  "hookEventName": "PreToolUse",
  "hookSpecificOutput": {"matcher": "Bash", "decision": "allow"}
}
```

`hookEventName` and `hookSpecificOutput` are `camelCase` (ADR 0024 parity).

### `warning`

Non-fatal informational frames that do not terminate the run. Currently emitted for model substitution detected at the provider level.

```json
{
  "type": "warning",
  "subtype": "model_mismatch",
  "message": "model mismatch: requested \"llama3.1\" but provider responded with \"llama3.2\"",
  "details": {"requested": "llama3.1", "actual": "llama3.2"}
}
```

### `result` — always the last frame

```json
{
  "type": "result",
  "subtype": "success",
  "result": "All 42 tests pass.",
  "session_id": "a3f7c2d1-...",
  "total_cost_usd": 0.0034,
  "turns": 3,
  "total_input_tokens": 8210,
  "total_output_tokens": 621
}
```

`subtype` values:

| subtype | Meaning | Key fields |
|---------|---------|------------|
| `success` | Run completed normally | `result` (assistant reply) |
| `error` | Provider error, hook denial, tool crash, or schema validation failure | `error`, `last_assistant_text`, `tool_calls_seen` |
| `max_turns` | `--max-turns` was reached (exit 75) | `last_assistant_text`, `tool_calls_seen` |
| `budget_exceeded` | `--max-budget-usd` was reached (exit 137) | `last_assistant_text`, `tool_calls_seen` |
| `cancelled` | Run was cancelled by Ctrl-C / SIGTERM (exit 124) | `last_assistant_text`, `tool_calls_seen` |
| `max_tokens` | Per-turn output token budget exhausted | `last_assistant_text`, `tool_calls_seen` |

For non-`success` subtypes, `result` is absent. Read `last_assistant_text` for the most recent assistant reply and `tool_calls_seen` to distinguish an actively-looping agent (many tool calls, no clean finish) from one that stalled silently.

## Stream-json input (`--input-format stream-json`)

Pass `--input-format stream-json` to make caliban read NDJSON `user` frames from stdin instead of a single prompt. This lets you drive multi-turn conversations from any language without a pseudo-TTY.

```json
{"type": "user", "content": "fix the lint warnings"}
{"type": "user", "content": [{"type": "text", "text": "now run the tests"}]}
```

`content` can be a plain string or an array of `{"type":"text","text":"…"}` blocks. Unknown fields on `user` frames, unknown `type` values, and malformed JSON are **hard parse errors** — the run aborts with exit 64 and a `result` frame with `subtype: "error"`. This is intentional: silent parsing of an unknown field would let a wrong envelope shape run the agent with a blank prompt.

A `control/interrupt` frame is accepted on stdin but the interrupt is not yet honored; caliban emits a stderr warning and continues.

When `--input-format stream-json` is active, an inline prompt is incompatible and is rejected at startup. Pass `-` (or omit the prompt entirely) to read from stdin.

## Example NDJSON exchange

```bash
printf '{"type":"user","content":"how many Rust source files are here?"}\n' \
  | caliban --output-format stream-json \
            --input-format stream-json \
            --replay-user-messages \
            --bare
```

```text
{"type":"system","subtype":"init","session_id":"b1c2...","model":"anthropic/claude-sonnet-4-6","tools":["Bash","Glob","Grep","Read"],"plugins":[],"settingSources":[],"mcp_servers":[],"bare_mode":true,"cwd":"/repo","permission_mode":"default"}
{"type":"user","content":[{"type":"text","text":"how many Rust source files are here?"}]}
{"type":"tool_use","id":"toolu_01","name":"Bash","input":{"command":"find . -name '*.rs' | wc -l"}}
{"type":"tool_result","tool_use_id":"toolu_01","is_error":false,"content":[{"type":"text","text":"142"}]}
{"type":"message","role":"assistant","content":[{"type":"text","text":"There are 142 Rust source files."},{"type":"tool_use","id":"toolu_01","name":"Bash","input":{"command":"find . -name '*.rs' | wc -l"}}]}
{"type":"result","subtype":"success","result":"There are 142 Rust source files.","session_id":"b1c2...","total_cost_usd":0.0012,"turns":1,"total_input_tokens":3100,"total_output_tokens":48}
```

## Optional frame flags

| Flag | Effect |
|------|--------|
| `--include-partial-messages` | Emit `text` and `thinking` delta frames as the model streams |
| `--include-hook-events` | Emit a `hook_event` frame for each fired hook |
| `--replay-user-messages` | Echo each user prompt back as a `user` frame |

## Related pages

- [Print Mode](./print-mode.md) — activating headless mode and output formats
- [CI Patterns](./ci.md) — parsing stream-json in scripts and Actions
