# Runaway-Thinking-Spiral Guard — Implementation Plan (#62)

> **For agentic workers:** TDD. Steps use checkbox syntax.

**Goal:** Per-turn cumulative thinking-char cap that terminates the run with
`StopCondition::ThinkingBudgetExhausted` when a model streams thinking without
progress.

**Architecture:** An `AgentConfig` field + a per-attempt counter in the turn
loop's thinking-delta branch that trips the standard `break 'outer` terminal
path; a new `StopCondition` variant with `is_failure`/`surface`.

**Tech Stack:** Rust, `caliban-agent-core`.

## Global Constraints

- `max_turn_thinking_chars: usize`, **0 disables** (convention).
- Default `262_144` chars — a backstop far above legitimate reasoning.
- Trip path mirrors existing terminal stops (`break 'outer`); additive.
- Full local gate before PR: fmt / clippy `-D warnings` / build / test, workspace.

---

### Task 1: StopCondition variant + config field + guard + tests

**Files:**
- Modify: `crates/caliban-agent-core/src/stream/mod.rs`
  - `StopCondition::ThinkingBudgetExhausted` (enum ~460, `is_failure` ~480,
    `surface` ~510).
  - Per-attempt `let mut thinking_chars: usize = 0;` beside
    `emitted_content_this_turn` (~1582); read
    `let max_turn_thinking_chars = self.config.max_turn_thinking_chars;` once.
  - In the `ActiveBlock::Thinking` delta branch (~1687): after `push_str` +
    `yield`, `thinking_chars = thinking_chars.saturating_add(s.chars().count());`
    then `if max_turn_thinking_chars > 0 && thinking_chars > max_turn_thinking_chars
    { stopped_for = StopCondition::ThinkingBudgetExhausted; break 'outer; }`.
- Modify: `crates/caliban-agent-core/src/agent.rs`
  - Add `pub max_turn_thinking_chars: usize` field (near `empty_turn_nudge_max`)
    + `262_144` in `Default` + assert in the config default test.
- Test: `crates/caliban-agent-core/tests/thinking_budget_guard.rs` (new).

**Interfaces:**
- Produces: `StopCondition::ThinkingBudgetExhausted`;
  `AgentConfig.max_turn_thinking_chars`.

- [ ] **Step 1: Write failing tests** — (a) trips over small cap; (b) under-cap
  turn completes with `EndOfTurn` + text streams; (c) `0` disables. Model on
  `recovery_stream_idle.rs`; script thinking-only deltas via
  `MockProvider::enqueue_stream` (`StreamingContentType::Thinking` +
  `StreamingDelta::Thinking`).
- [ ] **Step 2: Run, watch fail** — `cargo test -p caliban-agent-core --test
  thinking_budget_guard` → compile fail (variant/field absent).
- [ ] **Step 3: Implement** the variant, config field, counter + trip; fix any
  exhaustive `AgentConfig { .. }` construction sites the build flags.
- [ ] **Step 4: Run, watch pass** — same command green; run `agent.rs` config
  test + `empty_turn_nudge` + `recovery_*` for no regression.
- [ ] **Step 5: touch changed .rs, then full gate** — fmt/clippy/build/test.
- [ ] **Step 6: Commit** — `feat(agent-core): bound runaway thinking spirals with
  a per-turn thinking-char cap (#62)`.

---

## Self-Review

- Spec coverage: variant, config+default, counter/trip, three test cases — all
  in Task 1.
- No placeholders. Type consistency: `max_turn_thinking_chars: usize` and
  `ThinkingBudgetExhausted` used identically across struct, loop, and tests.
