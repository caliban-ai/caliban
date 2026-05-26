# ADR 0016 · Parallel tool dispatch (supersedes ADR 0009 §"sequential tools")

- **Status:** accepted
- **Date:** 2026-05-23
- **Supersedes:** ADR 0009 (in part — sequential tool dispatch only)

## Context

ADR 0009 chose **sequential** tool dispatch within a single assistant turn
as a v1 simplification: "Parallelism is a follow-on (Hooks-pluggable
dispatch strategy)." Real workloads bore out the cost. Models routinely
emit 2–6 `tool_use` blocks per turn (parallel `Grep`s + `Read`s while
exploring a codebase, repeated `WebFetch`es to compare sources), and the
serial loop paid the sum of their wall-clock latencies rather than the
max. The follow-on landed on `jf/feat/parallel-tools` in commits
`b624110` → `4751746` → `b5fba58`.

This ADR records the resulting architectural commitment.

## Decision

- **Parallel tool dispatch is default-on.** `AgentBuilder` initializes
  `parallel_tools: true`. Operator opt-out via
  `--no-parallel-tools` / `CALIBAN_NO_PARALLEL_TOOLS=1` falls through
  the same code path with `permits = 1`, preserving serial semantics
  without a separate branch.
- **Bounded concurrency via an `Arc<tokio::sync::Semaphore>`.** The
  default cap is
  `available_parallelism().get().saturating_sub(1).max(1)` —
  leave one core for the agent loop, streaming, and the TUI render
  thread. Tools are mostly I/O-bound, so this is a soft ceiling against
  runaway fan-out rather than a hard CPU bound. Operator override:
  `--parallel-tool-limit N` / `CALIBAN_PARALLEL_TOOL_LIMIT=N`.
- **`before_tool` hooks run serially.** The hook is the synchronization
  point for permissions, auditing, and `Deny` short-circuiting. The
  serial gate produces a `Vec<DispatchPlan>` of `Allowed` /
  `Denied` entries; only `Allowed` entries fan out to a
  `FuturesUnordered`. `Denied` results are yielded first, in
  assistant-message order, so the TUI sees deny notices before any
  in-flight tool resolves.
- **`Tool::invoke()` runs concurrently** for `Allowed` plans. Results
  arrive in completion order on the event stream (best TUI liveness)
  and are then reordered back into assistant-message order when
  appended to the persisted `tool_result_blocks` so history and replay
  remain deterministic.
- **Cancellation propagates** through the shared
  `tokio_util::sync::CancellationToken`. A cancel at any point aborts
  all in-flight tools; partial results are dropped.
- **Per-tool `is_parallel_safe()` flag is deferred.** All current
  built-ins are independent: `Bash` spawns fresh subprocesses;
  `Read` / `Grep` / `Glob` are pure-read; `Edit` / `Write` touch files
  but the model rarely emits overlapping writes on the same path.
  YAGNI — add the flag if write contention is observed in practice
  (e.g. two `Edit` calls on the same file in one turn).

## Rationale

The semaphore-bounded `FuturesUnordered` pattern keeps the agent loop
single-threaded while extracting most of the available parallelism from
the model's batching. The serial `before_tool` gate keeps the existing
hook contract intact — permission systems don't have to reason about
race conditions across concurrent tool calls. Streaming `ToolCallEnd`
events in completion order means the TUI shows whichever tool finishes
first immediately, instead of waiting for the slowest one in batch
order.

## Consequences

- **Positive.** Multi-tool turns clear in roughly `max(t_i)` rather
  than `sum(t_i)`. `parallel_tools=false` still works as an opt-out
  for users who want strict deterministic ordering in the event stream
  (e.g. for snapshot testing).
- **Negative.** Tracing output interleaves across tools within a turn;
  log readers need to follow `tool_use_id` to reconstruct per-tool
  sequences. The new `caliban::tools` tracing event surfaces
  dispatched/denied counts and total wall time per turn so the
  `perf-baseline` numbers stay legible.
- **ADR 0009's "sequential tools" guidance is superseded.** The rest
  of ADR 0009 — stream-as-primitive, opt-in compaction, conservative
  retry classifier — remains in force.
- **Sub-agent primitive** (forward link to
  `0021-sub-agent-primitive.md` when written) inherits this dispatch
  model: each sub-agent runs its own bounded parallel loop, and the
  parent agent's semaphore is independent of the child's.
- **Revisit if:** write contention surfaces in real use (add
  `is_parallel_safe()` and a per-tool exclusion policy), or if
  profiling shows the semaphore itself is a contention point at
  high concurrency (unlikely; tokio's `Semaphore` is fair and cheap).

## References

- Design spec: `docs/superpowers/specs/2026-05-23-parallel-tools-design.md`
- Commits: `b624110` (design), `6b71a6c` (plan), `4751746`
  (builder fields), `b5fba58` (FuturesUnordered + Semaphore refactor)
- Implementation: `crates/caliban-agent-core/src/agent.rs`
  (`parallel_tools` / `parallel_tool_limit` fields),
  `crates/caliban-agent-core/src/stream.rs` (three-phase dispatch)

## Revised 2026-05-26

The original Decision deferred a per-tool `is_parallel_safe()` flag,
noting that no built-in had write contention. That observation was
true in 2024 (Bash / Read / Grep / Glob). It is no longer true: ADRs
0028 + 0035 introduced Edit / Write / MultiEdit / NotebookEdit /
WriteMemoryTopic, all of which can collide on the same target within
one turn.

**Revised mechanism:** `parallel_conflict_key(&self, input) ->
Option<String>` on the `Tool` trait. Returns `None` for fully
parallel-safe tools (the default; matches the original 2024 posture).
Returns a conflict-identity string for tools whose effect is keyed to
a target — typically the canonicalized path for filesystem writes;
for `WriteMemoryTopic`, a `memory:{type}:{name}` string. The dispatcher
builds a per-key `tokio::sync::Mutex` map and each tool's dispatch
future awaits its key's mutex (FIFO) before acquiring the
`parallel_tool_limit` semaphore. Same-key calls serialize in
submission order; different-key calls and `None`-key calls parallelize.

**What this preserves.** Read / Grep / Glob / Bash continue to behave
exactly as before (default `None`). Two `Edit`s on different files
still parallelize. The parallel-tools differentiator from Claude Code
is intact.

**What this fixes.** Two `Edit`s on the same file (whether via the
same path string, a `./`-prefixed variant, or a symlink that
canonicalizes to the same inode) now serialize in submission order
rather than interleaving non-deterministically.

**Per-tool overrides shipped:** `Edit`, `Write`, `MultiEdit`,
`NotebookEdit` all key on the canonicalized path
(`crates/caliban-tools-builtin/src/parallel.rs::canonical_key`).
`WriteMemoryTopic` keys on `memory:{type}:{name}`.

**Tests:** `crates/caliban-agent-core/tests/parallel_conflict_key.rs`
covers distinct-key parallelism, same-key serialization,
keyed + plain mixing, and shared-key + independent triples.
