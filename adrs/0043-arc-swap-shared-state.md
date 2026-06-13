# ADR 0043 · `arc-swap` as the read-mostly shared-state primitive

- **Status:** accepted
- **Date:** 2026-05-26

## Context

Several read-mostly shared-state surfaces in the workspace use
`arc_swap::ArcSwap` rather than `tokio::sync::RwLock`:

- `caliban-agent-core::permission_mode::SharedPermissionMode` —
  `Arc<ArcSwap<PermissionMode>>` for the active permission mode
  (read on every tool call; written when the user toggles via the
  TUI overlay or a slash command).
- `caliban-model-router::breaker::CircuitBreaker` —
  `ArcSwap<BreakerState>` for the per-provider breaker state (read on
  every routed request; written on rolling-window state transitions).
- `caliban-settings::SettingsHandle` —
  `Arc<ArcSwap<Settings>>` for the live settings snapshot (read by
  many subsystems; written when `SettingsWatcher` fires a reload).

The choice was made per-surface during the parity sweep but never
documented at the workspace level. The 2026-05-25 ADR conformance
audit (Finding 7) flagged the gap.

## Decision

Prefer `arc-swap` for shared state when **all three** apply:

1. **Readers outnumber writers by ≥ 10×.** The replacement cost on
   each write is justified only when reads dominate.
2. **Writers can tolerate full `Arc` replacement.** `arc-swap` swaps
   a whole `Arc`; partial mutation requires a load-modify-store
   pattern (cheap but susceptible to lost updates without external
   coordination).
3. **Read latency is on the hot path.** A `tokio::sync::RwLock` is
   already cheap, but `arc_swap.load()` is *measurably* cheaper:
   it's lock-free, allocation-free, and has no contention even with
   100s of concurrent readers.

Use `tokio::sync::RwLock` for surfaces with **frequent partial
mutation** (e.g., long-lived per-key state where rewriting the whole
`Arc` would thrash GC), or where **writer fairness matters** more
than reader throughput.

Use plain `std::sync::Mutex` for short critical sections that don't
need to await across the lock.

## Consequences

- **Lock-free reads.** Every `load()` returns an `Arc<T>` snapshot
  via a guard with no contention.
- **No priority inversion** under load: readers never block writers,
  writers never block readers.
- **Slightly higher memory churn on writes:** each `store` allocates
  a new `Arc`. Acceptable for the listed surfaces because writes are
  rare (mode toggle, breaker state transition, settings reload).
- **No fairness guarantees between concurrent writers.** Acceptable
  because writers are rare; if two writers race, the later `store`
  wins per the swap's release semantics.
- **Snapshot semantics for readers.** A reader sees a single
  consistent value; subsequent reads may observe a different swapped
  value. Callers that need a stable snapshot across multiple reads
  should hoist the `load()` to a local. (No subsystem in the
  workspace currently relies on inter-read consistency for `arc-swap`
  surfaces.)
- **Cognitive load for new contributors** unfamiliar with the
  semantics: `load()` returns a snapshot, not a live reference. The
  module-level comments on each `ArcSwap` field call this out.

## Revisit if

- A surface using `arc-swap` grows a need for partial mutation that
  the swap pattern can't model cleanly — switch to `tokio::sync::RwLock`
  at that surface only.
- The `arc-swap` crate's maintenance status changes materially (it's
  small and stable, but watch for unmaintained markers).
- The workspace adds a surface with writer fairness requirements; do
  not stretch `arc-swap` to cover it.

## References

- `arc-swap` crate: https://crates.io/crates/arc-swap
- Surfaces:
  - `crates/caliban-agent-core/src/permission_mode.rs:124-140`
  - `crates/caliban-model-router/src/breaker.rs:68-79`
  - `crates/caliban-settings/src/lib.rs:70-83`
- Backfills a decision flagged as previously unrecorded by a 2026-05-25
  ADR conformance review (Finding 7).
