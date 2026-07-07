# Runaway-Thinking-Spiral Guard — Design (#62)

**Goal:** Bound a turn that streams *thinking* indefinitely without producing a
tool call or final text, returning control to the user with a clear stop rather
than hanging.

## Background / finding

A local reasoning model spiralled: it emitted thousands of thinking deltas
(~50–80 ms apart) with no tool call and no final text, and the session only
ended when the HTTP connection died. The only loop watchdog, `WatchedStream`,
fires on **stream idle** (no chunk within the timeout) — but the spiral streamed
continuously, so it never went idle. `MaxTokens` recovery doesn't help either:
it *raises* the budget on each hit, handing a spiral more rope. **Nothing bounds
a model that streams continuously but makes no semantic progress.**

## Decision

Add a per-turn **cumulative thinking-character cap**, independent of the idle
watchdog and the `max_tokens` budget. This is proposal option 1 (a thinking-token
cap). The alternatives are rejected:

- *Per-turn wall-clock ceiling* — hardware-flaky (a slow local model legitimately
  takes long), non-deterministic, penalizes slow-but-progressing turns.
- *Repetition / no-progress detection* — heuristic, false-positive-prone,
  over-engineered (YAGNI).

A char cap is deterministic, hermetically testable, and additive: the default is
set far above any legitimate single-turn reasoning, so it never trips in normal
use — it is a pure backstop against an unbounded spiral.

### Mechanism

- New `AgentConfig` field `max_turn_thinking_chars: usize` (**0 disables**,
  matching the `tool_result_cap_chars` / `stream_idle_timeout_ms` conventions).
  Default `262_144` (256 KiB ≈ ~65k tokens — ~4× the `escalated_max_tokens`
  16 384 budget, so a turn bounded by normal `MaxTokens` recovery stays far
  under it; only a genuinely unbounded spiral reaches it).
- In the turn loop's stream-drain, accumulate a per-attempt `thinking_chars`
  counter in the `AssistantThinkingDelta` branch (`stream/mod.rs`). When
  `max_turn_thinking_chars > 0 && thinking_chars > max_turn_thinking_chars`,
  set `stopped_for = StopCondition::ThinkingBudgetExhausted` and `break 'outer`
  — the same terminal path every other stop condition uses (provider error,
  stream idle, etc.).
- New `StopCondition::ThinkingBudgetExhausted` variant:
  - `is_failure()` → `true`.
  - `surface()` → a framed `[caliban: …]` line at `StopLevel::Error`, e.g.
    "thinking budget exhausted — the model kept reasoning without answering;
    try /effort low to reduce reasoning budget", mirroring the
    `MaxTokensExhausted` surface.

### Why per-attempt, not per-run

The spiral occurs within a single stream drain (the evidence turn emitted 4 486
deltas in one attempt). The counter resets with the other per-attempt state
(`acc`, `emitted_content_this_turn`); a spiral trips the cap within the first
spiralling attempt regardless of retries.

### Interaction with existing guards

Orthogonal to the #249 empty/degenerate-turn *nudge* (which handles a turn that
*ends* with only thinking, by nudging and taking another turn) and to `MaxTokens`
recovery. This guard handles a turn that *never ends on its own*. It only trips
above the cap, so it never pre-empts those softer mechanisms in normal operation.

## Files

- `crates/caliban-agent-core/src/agent.rs` — add the config field + default +
  the default-value assertion in the existing config test.
- `crates/caliban-agent-core/src/stream/mod.rs` — the `ThinkingBudgetExhausted`
  variant (enum + `is_failure` + `surface`); the per-attempt counter + trip.
- Any exhaustive `AgentConfig { … }` construction site the compiler flags
  (add the field or `..Default::default()`).

## Testing

New `crates/caliban-agent-core/tests/thinking_budget_guard.rs`, modeled on
`recovery_stream_idle.rs`:

- **Trips:** `max_turn_thinking_chars` set small (e.g. 100); a `MockProvider`
  stream of thinking-only deltas exceeding the cap (no text, no tool) → the run
  ends with `StopCondition::ThinkingBudgetExhausted`, promptly (does not hang,
  does not consume the whole scripted stream).
- **Does not trip normal turns:** a thinking block *under* the cap followed by a
  final text/`EndTurn` → completes with `StopCondition::EndOfTurn` and the text
  streams through (positive assertion, per #334).
- **Disabled (0):** `max_turn_thinking_chars = 0` with over-default thinking →
  completes normally (guard off).

## Acceptance

A turn that streams thinking past the configured cap without a tool call or final
text terminates the run with `ThinkingBudgetExhausted` and a clear user-facing
message, instead of hanging. Default is a high backstop that never affects
normal reasoning.
