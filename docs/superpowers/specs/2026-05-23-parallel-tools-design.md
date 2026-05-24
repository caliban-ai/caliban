# Parallel Tool Calls — Design

**Date:** 2026-05-23
**Status:** Approved
**Target branch:** `jf/feat/parallel-tools`
**Related:** ADR 0009 (deferred this), `2026-05-23-perf-baseline-design.md` (also deferred this)

## Goal

Run multiple `tool_use` blocks from a single assistant turn concurrently instead of serially. Today `stream.rs:681-718` iterates `ContentBlock::ToolUse` blocks in a `for` loop and awaits each `dispatch_tool` call before starting the next. Models routinely emit 2-6 tool calls per turn (e.g., parallel `Grep`s + `Read`s while exploring a codebase), and the current implementation pays the sum of their wall-clock latencies rather than the max.

## Non-goals

- **Pre-dispatch mid-stream.** Starting a tool before the assistant message finishes streaming — saves the model-done → tool-start gap but requires reworking the streaming state machine. Separate design.
- **Read/write classification or per-tool opt-out via the `Tool` trait.** All current built-ins are independent (Bash spawns fresh subprocesses; Read/Grep/Glob are pure-read; Edit/Write touch files but the model rarely emits overlapping writes). YAGNI — add when a real conflict surfaces.
- **Disabling the model's batching.** OpenAI's `parallel_tool_calls=false` is a server-side knob over whether the model emits parallel blocks; we want the model to keep batching.
- **Subagent / nested-agent parallelism.** Different concern; out of scope.
- **Reordering `tool_result_blocks` in stored history.** History order will match assistant-message order so replay/serialization remain deterministic.

## Scope summary

| # | Item | Layer | LOC est |
|---|---|---|---|
| 1 | `Agent.parallel_tools` + `Agent.parallel_tool_limit` fields + builder methods | agent-core | ~30 |
| 2 | Refactor dispatch loop in `stream.rs` to use `FuturesUnordered` + `Semaphore` | agent-core | ~90 |
| 3 | CLI flags + env vars + plumbing into `AgentBuilder` | caliban bin | ~25 |
| 4 | Tracing event for parallel dispatch (`caliban::tools` target) | agent-core | ~10 |
| 5 | Tests (6 new) | agent-core | ~280 |

Total: ~435 LOC, mostly tests.

## Architecture

### Default behavior

Parallel dispatch is **default on**. The user opts out with `--no-parallel-tools` (or `CALIBAN_NO_PARALLEL_TOOLS=1`).

The default concurrency cap is `std::thread::available_parallelism().map(NonZeroUsize::get).unwrap_or(2).saturating_sub(1).max(1)`. On a 6-core dev laptop this is 5; on a 12-core M-series box it's 11; on a hypothetical single-core CI runner it's 1. The user can override with `--parallel-tool-limit N` (or `CALIBAN_PARALLEL_TOOL_LIMIT=N`).

Rationale for `cores - 1`: leaves one core free for the agent loop, streaming, the TUI render thread, and the SSE parser. Tools are mostly I/O-bound (FS, subprocess spawn, network), so the cap is a soft ceiling against runaway fan-out rather than a hard CPU bound.

### Agent surface (`agent.rs`)

```rust
pub struct Agent {
    // ... existing fields ...
    parallel_tools: bool,
    parallel_tool_limit: NonZeroUsize,
}

impl AgentBuilder {
    pub fn parallel_tools(mut self, enabled: bool) -> Self { ... }
    pub fn parallel_tool_limit(mut self, limit: NonZeroUsize) -> Self { ... }
}
```

`AgentBuilder::Default` initializes:
- `parallel_tools: true`
- `parallel_tool_limit: default_parallel_tool_limit()` — the `available_parallelism` expression above, evaluated once.

### Dispatch loop (`stream.rs`)

Replace the existing `for block in &assistant_message.content { if let ContentBlock::ToolUse(tu) = ... }` loop (currently `stream.rs:681-718`) with a three-phase dispatch:

**Phase 1 — Plan:** Collect `(original_index, ToolUseBlock)` pairs from `assistant_message.content`. Run `before_tool` hook **serially** in assistant-message order (preserves today's deny-gate semantics — a deny can short-circuit before any tool starts). Build a `Vec<DispatchPlan>` where each entry is either `Denied { original_index, result_block }` or `Allowed { original_index, id, name, input }`.

```rust
enum DispatchPlan {
    Denied { original_index: usize, result_block: ContentBlock },
    Allowed { original_index: usize, id: String, name: String, input: serde_json::Value },
}
```

**Phase 2 — Dispatch:** Build a `FuturesUnordered<JoinHandle<...>>` (or `FuturesUnordered<BoxFuture<...>>` to avoid spawn complexity) of allowed invocations, gated by `Arc<Semaphore>` with `permits = self.parallel_tool_limit.get()`. Yield `TurnEvent::ToolCallEnd` for `Denied` entries up front (in assistant-message order — they were already gated serially). Then drive the futures stream:

```rust
let sem = Arc::new(Semaphore::new(self.parallel_tool_limit.get()));
let agent_ref: &Agent = &self;
let mut pending = FuturesUnordered::new();
for plan in allowed_plans {
    if cancel.is_cancelled() {
        stopped_for = StopCondition::Cancelled;
        break 'outer;
    }
    let permit = Arc::clone(&sem).acquire_owned().await.unwrap();
    let cancel_for_tool = cancel.clone();
    let DispatchPlan::Allowed { original_index, id, name, input } = plan else {
        unreachable!("denied plans were handled in phase 1");
    };
    pending.push(async move {
        let _permit = permit; // released on drop
        let res = dispatch_tool(agent_ref, turn_index, &id, &name,
                                input, &cancel_for_tool).await;
        (original_index, id, res)
    });
}

let mut ordered_results: Vec<Option<ToolResultBlock>> =
    vec![None; assistant_message.content.len()];

while let Some((idx, id, dispatch_res)) = pending.next().await {
    match dispatch_res {
        Err(stop) => {
            stopped_for = stop;
            // drain remaining pending so no orphan futures escape the loop
            while pending.next().await.is_some() {}
            break 'outer;
        }
        Ok(tool_result) => {
            let is_error = tool_result.is_error;
            let content = tool_result.content.clone();
            yield TurnEvent::ToolCallEnd {
                turn_index,
                tool_use_id: id,
                is_error,
                content,
            };
            ordered_results[idx] = Some(tool_result);
        }
    }
}
```

When `parallel_tools` is false **or** the limit is `1`, the same code path runs with `permits = 1`, giving identical semantics to today's serial loop (with one extra hop through `FuturesUnordered`, which is negligible). No separate code path; one implementation.

**Phase 3 — Reorder for history:** Build `tool_result_blocks: Vec<ContentBlock>` by walking `ordered_results` in index order and pushing each `Some(tr)` as `ContentBlock::ToolResult(tr)`. `None` slots correspond to assistant content blocks that weren't `ToolUse` (text, thinking) and are skipped. Denied results are spliced in here too — Phase 1 wrote them into `ordered_results` directly. The resulting `tool_result_blocks` matches the assistant-message tool_use order, so persisted history is deterministic.

### Hook ordering

| Hook | Order | Rationale |
|---|---|---|
| `before_tool` | Serial, assistant-message order | Deny gate must be predictable and short-circuit before invoke. Cheap (typical impls are pattern-matching). |
| `invoke` | Parallel, completion order | The point of this design. |
| `after_tool` | Parallel (fires inside each `dispatch_tool` after its invoke) | Each tool's `after_tool` is independent. Order is non-deterministic. |

This is documented in the `Hooks` trait doc-comment so implementors don't assume serial `after_tool` order.

### `TurnEvent` stream ordering

| Event | Order under parallel | Order under serial (today) |
|---|---|---|
| `ToolCallStart` | Assistant-stream order | Assistant-stream order |
| `ToolCallInputDelta` | Assistant-stream order | Assistant-stream order |
| `ToolCallEnd` (denied) | Assistant-message order, emitted before any invokes start | Assistant-message order |
| `ToolCallEnd` (executed) | **Completion order** | Assistant-message order |
| `TurnEnd` | After all `ToolCallEnd` events | After all `ToolCallEnd` events |

Completion order is the deliberate UX choice: the TUI renders each tool's result inline as it completes, so the user sees fast tools finish first instead of all results appearing in a burst after the slowest one. The TUI already correlates by `tool_use_id`, so arrival order is fine.

### Cancellation

The existing `CancellationToken` is cloned into each invoke. On cancel:

1. **In-flight tools** observe `cancel.is_cancelled()` and return `ToolError::Cancelled` (existing behavior).
2. **The dispatch loop** checks `cancel.is_cancelled()` before each `acquire_owned().await` and refuses to start new invocations.
3. **Drain.** When a `dispatch_tool` returns `Err(StopCondition::Cancelled)`, the loop drains `pending` (`while pending.next().await.is_some() {}`) before breaking, so no future escapes to be polled after the stream ends.
4. **Outer break** propagates `StopCondition::Cancelled` to the run outcome.

This preserves today's "cancel waits for in-flight tools" semantics but bounds it to *only* the in-flight set, not all-remaining.

### Tracing

Add one event per turn after dispatch completes:

```rust
tracing::info!(
    target: "caliban::tools",
    turn = turn_index,
    parallel_tools_enabled = self.parallel_tools,
    parallel_tool_limit = self.parallel_tool_limit.get(),
    dispatched = allowed_plans.len(),
    denied = denied_count,
    total_wall_ms = dispatch_elapsed.as_millis() as u64,
    "parallel tool dispatch",
);
```

Lets users compare `total_wall_ms` against the per-tool spans to confirm parallelism is actually happening for their workload.

### CLI plumbing (`caliban/src/main.rs`)

Two new flags on the run command (mirror `--no-prompt-cache`):

```rust
/// Disable parallel tool execution (run tool_use blocks one at a time).
#[arg(long, env = "CALIBAN_NO_PARALLEL_TOOLS")]
no_parallel_tools: bool,

/// Max concurrent tool invocations per turn. Defaults to (CPU cores - 1, min 1).
#[arg(long, env = "CALIBAN_PARALLEL_TOOL_LIMIT")]
parallel_tool_limit: Option<NonZeroUsize>,
```

Wired into `AgentBuilder` after parsing:

```rust
let mut builder = Agent::builder().provider(provider).tools(registry);
if args.no_parallel_tools { builder = builder.parallel_tools(false); }
if let Some(n) = args.parallel_tool_limit { builder = builder.parallel_tool_limit(n); }
```

## Testing

Six new tests in `crates/caliban-agent-core/tests/parallel_tools.rs` (new file) using `MockProvider`:

1. **Wall-time parallel wins.** A `SleepyTool` that calls `tokio::time::sleep(Duration::from_millis(50))` then returns. MockProvider yields an assistant message with three `ToolUse` blocks all naming `SleepyTool`. With `parallel_tools=true, limit=3`, total dispatch wall < 90ms; with `parallel_tools=false`, > 140ms. Uses `tokio::time::pause()` + `advance` for determinism.

2. **History order preserved.** Three tools (A, B, C) where C's sleep is 5ms, A's is 50ms, B's is 25ms. Assert: the `final_messages` entry for the tool-result message has `ContentBlock::ToolResult` blocks in order A, B, C (matching assistant-message order). Assert: the `TurnEvent::ToolCallEnd` events arrived in order C, B, A.

3. **Deny preserves semantics.** A `DenyingHooks` impl denies tool B by name. Tools A and C run in parallel; B's `ToolResult` is the synthesized denial; history order is A, B-denied, C. Asserted via `final_messages`.

4. **Cancellation drains in-flight.** Start a turn with three `SleepyTool` calls at 100ms each. Fire `cancel.cancel()` after 10ms. Assert: run terminates with `StopCondition::Cancelled`; no `TurnEnd` is emitted (today's behavior is `TurnEnd` is emitted only on successful completion or hook-deny); all three tools observe the cancel (counter incremented to 3 inside the tool's `invoke`).

5. **Limit honored.** A `TrackingTool` that increments a shared `Arc<Mutex<usize>>` on entry and decrements on exit, recording the peak. MockProvider yields five `ToolUse` blocks. With `parallel_tool_limit=2`, peak concurrent == 2. With `limit=5`, peak == 5.

6. **`parallel_tools(false)` equivalent to `limit=1`.** Same setup as test 5; with `parallel_tools=false`, peak concurrent == 1, and `ToolCallEnd` events arrive in assistant-message order (matching today's behavior).

## Risks

- **Concurrent FS writes to the same path.** If the model emits two `Edit`s or `Write`s on the same file, the result is non-deterministic under parallel execution. The model rarely does this — it almost always batches *reads*. Mitigation: documented as a known limitation in the user-facing CLI help; `--parallel-tool-limit 1` is the escape hatch. If real workloads hit this, the v2 escape is a `Tool::is_parallel_safe()` flag or a path-based serializer; out of scope here.
- **Bash subprocess fan-out.** A turn emitting 8 parallel `Bash` calls can briefly fork 8 subprocesses. The default cap (cores - 1) bounds this; users with constrained environments (Docker with CPU limits) can lower it. Acceptable.
- **Tracing span interleaving.** Per-tool `#[instrument]` spans now overlap. Each carries `tool_use_id` and `tool_name` fields, so logs remain readable but are no longer linearly ordered per turn. Acceptable; the new `caliban::tools` event gives a single-line summary.
- **Hook implementor surprise.** Existing `Hooks` implementors may assume `after_tool` fires in tool-use order. Mitigated by updating the trait doc-comment to explicitly call out that `after_tool` order is non-deterministic under parallel dispatch.

## Open questions

None blocking.

## Future work

- **Per-tool `is_parallel_safe()` flag** if real workloads exhibit write contention.
- **Pre-dispatch mid-stream** (start invoking the first tool while the model is still streaming subsequent tool_use blocks). Larger scope; needs streaming-state-machine rework.
- **Adaptive cap** (lower the limit if observed tool latency variance suggests resource contention). YAGNI for now.
