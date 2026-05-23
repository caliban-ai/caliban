# ADR 0014 · Default system prompt + TUI stall fixes + debug logging

- **Status:** accepted
- **Date:** 2026-05-23

## Context

Real-use testing revealed two issues with the daily-usable caliban:

1. **No default system prompt** — models had no context that they were
   running in caliban, what tools were available, or which directory they
   were operating in. Behavior was generic-assistant rather than
   harness-aware.

2. **Occasional streaming stalls** — the TUI's event-loop draws once per
   `tokio::select!` iteration. Sometimes the loop appeared to hang: the
   transcript wouldn't update until the user pressed a key, at which point
   it would advance by one line. Input wouldn't echo during the stall.

## Decision

### System prompt

A `caliban-cli/src/system_prompt.rs` module builds a default prompt
auto-derived from current state (caliban identity, cwd, registered tool
names + descriptions, basic operating conventions). Resolves precedence:

- `--system "<text>"` — literal override
- `--system-file <PATH>` — file content
- `--no-system` — no system prompt
- (none) — default

All four are mutually exclusive via clap. The first three produce
`Option<String>`; the default returns `Some(text)`.

**Persistence rule:** the system prompt is inserted as `messages[0]`
(`Role::System`) when a session is FIRST created. Loading an existing
session does NOT replace the prompt — the persisted system prompt is the
contract for that session. Switching models mid-session can produce a
mismatch (e.g., Claude-flavored prompt sent to a GPT model); this is
documented and considered acceptable. Users can edit the session JSON or
start a new session to refresh.

For ephemeral runs (no `--session`), the system prompt is prepended to
the message list at turn-construction time.

### TUI streaming stall fix

Three belt-and-suspenders changes in the TUI event loop:

1. **Tick interval** at 50ms (20 Hz) added to the `tokio::select!`. Even
   with no terminal or agent events, the loop iterates and redraws. This
   masks any missed-wakeup symptoms from either stream source.

2. **Explicit `stdout().flush()`** after each `terminal.draw()`. Ratatui's
   backend should flush internally; this catches any platform-specific
   line-buffering edge cases.

3. **`tokio::task::yield_now()`** between iterations. Ensures runtime
   fairness so neither the EventStream task nor the HTTP-streaming task
   can starve the loop.

If the underlying cause is something deeper (e.g., a missing waker in
`async_stream::try_stream!`), these fixes mask the symptom rather than
addressing the root cause. The debug log (below) will help identify
whether stalls recur.

### Debug logging

`--debug` flag or `CALIBAN_DEBUG=1` env var enables a
`tracing-subscriber` file appender writing to
`<cache_dir>/caliban/debug.log`. Logs each terminal event, agent stream
event, draw, and error. No overhead when disabled (the subscriber is
not installed).

## Consequences

- **Positive:** Models now know their context. Stalls (if not eliminated)
  are masked by tick-based redraws, and diagnostic data is available
  for future investigation. System prompt is configurable per-invocation
  and inspectable via `/system` overlay.
- **Negative:** 20 Hz tick = continuous redraws even when nothing
  changes. Ratatui's diffing keeps wire cost at zero, but CPU spends
  ~50ms-of-work-per-second on the diff. Acceptable for interactive UX.
  System prompt grows with tool count; will need summarization at MCP/
  skills scale (future).
- **Revisit if:** Stalls recur with the tick in place — that indicates a
  deeper bug in the event-stream or agent-stream that we need to dig
  into using the debug logs. Or if profiling shows the 50ms tick is
  expensive (drop to 100ms or 200ms).
