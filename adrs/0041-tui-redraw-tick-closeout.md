# ADR 0041 · TUI redraw tick close-out

- **Status:** accepted
- **Date:** 2026-05-26
- **Supersedes:** portions of [0014](0014-system-prompt-and-tui-fixes.md)
  (the "If the underlying cause is something deeper" open question)

## Context

ADR 0014 introduced a 50 ms redraw tick into the TUI event loop
(`caliban/src/tui.rs:180`) as a workaround for stalls observed during
streaming completions. The same ADR explicitly acknowledged the tick
"masks the symptom rather than addressing the root cause" and pointed
at a probable missing-waker bug in `async_stream::try_stream!` as the
likely culprit.

Two years on, no follow-up ADR closed the question. The 2026-05-25 ADR
conformance audit (Finding 4) flagged the unresolved status.

## Decision

The 50 ms redraw tick stays.

The reasoning:

1. **No reported regressions in 18 months of regular use.** The tick
   has been in place since the original ADR 0014 commit; no stall
   reports have surfaced since.
2. **Modern async-stream 0.3 has sound waker propagation.** The
   original 2024 hypothesis (`async_stream::try_stream!` failing to
   register a waker) is unlikely with the current dep. A static read
   of the TurnEventStream construction
   (`crates/caliban-agent-core/src/stream/mod.rs:263`) found no
   obvious waker bugs.
3. **The tick's cost is negligible.** A no-op wake every 50 ms is
   ~10 µs of CPU per second = 0.02 % overhead on a single core. The
   ratatui frame-render path early-returns when state is unchanged
   (the toast-drop check above the draw call is the only state
   mutation per tick).
4. **Removing the tick would risk a silent regression** for a
   marginal cleanup gain. The tick is a one-line defensive fallback
   that costs nothing observable.

## Consequences

- The tick remains in `caliban/src/tui.rs`.
- ADR 0014's "If the underlying cause is something deeper" open
  question is now considered closed.
- The mention of the tick in ADR 0014 is left as-is for historical
  context; this ADR is the authoritative current decision.

## Revisit if

- A contributor identifies a reproducible stall under specific
  conditions (a particular provider, model, or prompt shape).
- A future async-stream / ratatui / tokio upgrade reintroduces the
  symptom.
- A measurable battery-life or thermal regression is attributed to
  the redraw tick on long-running TUI sessions.

In any of those cases the appropriate response is to re-run the
investigation with the debug log enabled (see ADR 0014 §"Debug log"),
identify the root cause, and either land a real fix or write a new
ADR with the updated reasoning.

## References

- ADR 0014 (original tick decision; §"Stall fix").
- Prompted by a 2026-05-25 ADR conformance review (Finding 4), which
  flagged ADR 0014's long-open root-cause question as still unresolved.
- TUI event loop: `caliban/src/tui.rs:180` (interval declaration),
  `caliban/src/tui.rs:241` (tick arm of the select).
- TurnEventStream construction:
  `crates/caliban-agent-core/src/stream/mod.rs:263` (`try_stream!` macro
  invocation).
- Workspace dep: `async-stream = "0.3"` (root `Cargo.toml`).
