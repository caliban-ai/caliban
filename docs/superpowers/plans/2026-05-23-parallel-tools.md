# Parallel Tool Calls Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Dispatch a single assistant turn's `tool_use` blocks concurrently (bounded by a semaphore, default cap = CPU cores - 1) instead of serially, preserving history order while streaming `ToolCallEnd` events in completion order.

**Architecture:** `Agent` gains `parallel_tools: bool` and `parallel_tool_limit: NonZeroUsize` fields. The dispatch loop in `stream.rs` is refactored from a serial `for` loop into a three-phase pipeline: (1) serial `before_tool` gate produces a `Vec<DispatchPlan>`; (2) allowed plans are pushed onto `FuturesUnordered`, each acquiring a permit from `Arc<Semaphore>`; (3) results are collected and reordered into assistant-message order for history while their corresponding `ToolCallEnd` events stream in completion order.

**Tech Stack:** Rust 1.95, tokio 1.x (Semaphore from `sync`), `futures::stream::FuturesUnordered`, `tokio_util::sync::CancellationToken`, `async_stream::try_stream`.

**Spec:** `docs/superpowers/specs/2026-05-23-parallel-tools-design.md`

---

## File map

| Path | Action | Responsibility |
|---|---|---|
| `crates/caliban-agent-core/src/agent.rs` | modify | Add `parallel_tools`, `parallel_tool_limit` fields + builder methods + `default_parallel_tool_limit()` helper |
| `crates/caliban-agent-core/src/stream.rs` | modify | Refactor dispatch loop (~lines 678-734); add `DispatchPlan` enum; add tracing event |
| `crates/caliban-agent-core/src/hooks.rs` | modify | Doc-comment update on `after_tool` |
| `crates/caliban-agent-core/tests/parallel_tools.rs` | create | 6 integration tests |
| `caliban/src/main.rs` | modify | Add `--no-parallel-tools` and `--parallel-tool-limit` flags; wire into builder |

---

## Task 1: Agent config — fields, defaults helper, builder methods

**Files:**
- Modify: `crates/caliban-agent-core/src/agent.rs`

- [ ] **Step 1: Write failing unit tests for the default helper and builder**

Append at the bottom of `crates/caliban-agent-core/src/agent.rs`:

```rust
#[cfg(test)]
mod parallel_tools_config_tests {
    use super::*;

    #[test]
    fn default_limit_is_at_least_one() {
        let n = default_parallel_tool_limit();
        assert!(n.get() >= 1, "default cap must be >= 1");
    }

    #[test]
    fn default_limit_matches_cores_minus_one() {
        let cores = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(2);
        let expected = cores.saturating_sub(1).max(1);
        assert_eq!(default_parallel_tool_limit().get(), expected);
    }

    #[test]
    fn builder_defaults_parallel_tools_on() {
        let b = AgentBuilder::default();
        assert!(b.parallel_tools, "parallel_tools should default to true");
        assert!(b.parallel_tool_limit.get() >= 1);
    }

    #[test]
    fn builder_parallel_tools_setter() {
        let b = AgentBuilder::default().parallel_tools(false);
        assert!(!b.parallel_tools);
    }

    #[test]
    fn builder_parallel_tool_limit_setter() {
        let limit = std::num::NonZeroUsize::new(3).unwrap();
        let b = AgentBuilder::default().parallel_tool_limit(limit);
        assert_eq!(b.parallel_tool_limit.get(), 3);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-agent-core --lib parallel_tools_config_tests`
Expected: FAIL with errors like "no field `parallel_tools` on type `AgentBuilder`" / "no function `default_parallel_tool_limit`".

- [ ] **Step 3: Add the default helper**

Insert at the top of `crates/caliban-agent-core/src/agent.rs` after the existing `use` block (around line 10, after the existing imports):

```rust
use std::num::NonZeroUsize;

/// Default per-turn parallel tool dispatch limit.
///
/// Returns `available_parallelism().get() - 1`, clamped to at least 1, so that
/// the agent loop, streaming, and the renderer can keep a core to themselves.
/// Falls back to 1 when `available_parallelism()` is unavailable.
#[must_use]
pub fn default_parallel_tool_limit() -> NonZeroUsize {
    let n = std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(2)
        .saturating_sub(1)
        .max(1);
    NonZeroUsize::new(n).expect("max(1) guarantees nonzero")
}
```

- [ ] **Step 4: Add fields to `Agent` and `AgentBuilder`**

Find the `pub struct Agent` block (around line 55) and add the two fields after `prompt_cache: bool` (line 64):

```rust
pub struct Agent {
    pub(crate) provider: Arc<dyn Provider + Send + Sync>,
    pub(crate) tools: ToolRegistry,
    pub(crate) config: AgentConfig,
    pub(crate) compactor: Arc<dyn crate::compact::Compactor + Send + Sync>,
    pub(crate) retry: crate::retry::RetryPolicy,
    pub(crate) hooks: Arc<dyn Hooks + Send + Sync>,
    pub(crate) prompt_cache: bool,
    /// When true, multiple `tool_use` blocks in one assistant turn run
    /// concurrently (bounded by `parallel_tool_limit`). When false, they
    /// run serially.
    pub(crate) parallel_tools: bool,
    /// Maximum concurrent tool invocations per turn. Ignored when
    /// `parallel_tools` is false (equivalent to `1`).
    pub(crate) parallel_tool_limit: NonZeroUsize,
}
```

Find `pub struct AgentBuilder` (around line 103) and add matching fields after `prompt_cache: bool`:

```rust
pub struct AgentBuilder {
    provider: Option<Arc<dyn Provider + Send + Sync>>,
    tools: ToolRegistry,
    config: AgentConfig,
    compactor: Option<Arc<dyn crate::compact::Compactor + Send + Sync>>,
    retry: Option<crate::retry::RetryPolicy>,
    hooks: Option<Arc<dyn Hooks + Send + Sync>>,
    prompt_cache: bool,
    parallel_tools: bool,
    parallel_tool_limit: NonZeroUsize,
}
```

- [ ] **Step 5: Update the `AgentBuilder::default` initializer**

Find `impl Default for AgentBuilder` (around line 113) and update:

```rust
impl Default for AgentBuilder {
    fn default() -> Self {
        Self {
            provider: None,
            tools: ToolRegistry::default(),
            config: AgentConfig::default(),
            compactor: None,
            retry: None,
            hooks: None,
            // Prompt caching is default-on. Anthropic users get cache hits
            // from turn 2 onward; non-Anthropic providers ignore the markers.
            prompt_cache: true,
            parallel_tools: true,
            parallel_tool_limit: default_parallel_tool_limit(),
        }
    }
}
```

Note: making the field `parallel_tools` directly accessible (no `pub(crate)`) is fine — the surrounding `pub struct AgentBuilder` already exposes builder semantics through methods. The test accesses `b.parallel_tools` directly, which works because the test is in the same module.

- [ ] **Step 6: Add builder methods**

After the `prompt_cache` builder method (around line 203-206), add:

```rust
    /// Enable or disable parallel tool dispatch. Default: enabled.
    ///
    /// When `false`, all `tool_use` blocks in a single assistant turn run
    /// serially in assistant-message order (the v1 behavior). When `true`,
    /// they run concurrently bounded by [`Self::parallel_tool_limit`].
    #[must_use]
    pub fn parallel_tools(mut self, on: bool) -> Self {
        self.parallel_tools = on;
        self
    }

    /// Set the maximum concurrent tool invocations per turn. Default:
    /// [`default_parallel_tool_limit()`] (typically `cores - 1`).
    #[must_use]
    pub fn parallel_tool_limit(mut self, limit: NonZeroUsize) -> Self {
        self.parallel_tool_limit = limit;
        self
    }
```

- [ ] **Step 7: Update `AgentBuilder::build` to populate the new fields**

Find `build` (around line 213) and update the `Ok(Agent { ... })` block:

```rust
        Ok(Agent {
            provider,
            tools: self.tools,
            config: self.config,
            compactor: self
                .compactor
                .unwrap_or_else(|| Arc::new(crate::compact::NoopCompactor)),
            retry: self.retry.unwrap_or_default(),
            hooks: self.hooks.unwrap_or_else(|| Arc::new(NoopHooks)),
            prompt_cache: self.prompt_cache,
            parallel_tools: self.parallel_tools,
            parallel_tool_limit: self.parallel_tool_limit,
        })
```

- [ ] **Step 8: Re-export the helper from the crate root**

Edit `crates/caliban-agent-core/src/lib.rs`. Update the `agent` re-export line (line 17):

```rust
pub use agent::{Agent, AgentBuilder, AgentConfig, default_parallel_tool_limit};
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p caliban-agent-core --lib parallel_tools_config_tests`
Expected: 5 tests pass.

- [ ] **Step 10: Run the full lib test suite to ensure no regressions**

Run: `cargo test -p caliban-agent-core --lib`
Expected: All existing tests still pass; 5 new tests added.

- [ ] **Step 11: Commit**

```bash
git add crates/caliban-agent-core/src/agent.rs crates/caliban-agent-core/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(agent-core): add parallel_tools + parallel_tool_limit Agent fields

Default-on; cap defaults to available_parallelism().get() - 1 (min 1).
Surface via AgentBuilder::parallel_tools(bool) and
::parallel_tool_limit(NonZeroUsize). Default helper re-exported from
crate root for downstream binaries.

Dispatch loop changes come next.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Refactor dispatch loop in stream.rs

**Files:**
- Modify: `crates/caliban-agent-core/src/stream.rs`

This is the central change. Refactor `stream_until_done`'s per-turn tool dispatch from a serial `for` loop into three phases (plan / dispatch / reorder). When `parallel_tools` is false **or** the limit is 1, behavior is identical to today (single permit ⇒ serial execution through `FuturesUnordered`).

- [ ] **Step 1: Add new imports to `stream.rs`**

Edit the imports block at the top of `crates/caliban-agent-core/src/stream.rs`. Add:

```rust
use std::sync::Arc;

use futures::stream::FuturesUnordered;
use tokio::sync::Semaphore;
```

`std::sync::Arc` and the `futures` prelude are already in the file. Re-check after edit; only add what isn't already there.

- [ ] **Step 2: Add the `DispatchPlan` enum**

Insert near the top of `stream.rs` (after the `TurnTiming` block, before `TurnEvent`):

```rust
// ---------------------------------------------------------------------------
// Per-turn dispatch plan
// ---------------------------------------------------------------------------

/// A single tool dispatch plan, produced by the serial `before_tool` gate.
///
/// `original_index` is the position of the corresponding `ContentBlock::ToolUse`
/// within the assistant message; it's used to reorder results back into
/// assistant-message order for history.
enum DispatchPlan {
    /// `before_tool` returned `Allow`; the invoke will run.
    Allowed {
        original_index: usize,
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// `before_tool` returned `Deny`; the synthesized denial `ToolResult`
    /// stands in for the invoke.
    Denied {
        original_index: usize,
        result: ToolResultBlock,
    },
}
```

- [ ] **Step 3: Locate the dispatch loop**

The existing loop is at `crates/caliban-agent-core/src/stream.rs:678-718` (the `// ---- Dispatch tools sequentially ----` block, ending just before `// Build the tool-results message (if any tools were called).`).

- [ ] **Step 4: Replace the dispatch loop**

Replace the entire block from the `// ---- Dispatch tools sequentially ----` comment through the close of the `for block in &assistant_message.content { ... }` loop (i.e., everything up to the `// Build the tool-results message ...` comment) with the new three-phase implementation:

```rust
                // ---- Phase 1: plan (serial before_tool gate) ----
                let mut plans: Vec<DispatchPlan> = Vec::new();
                for (idx, block) in assistant_message.content.iter().enumerate() {
                    if cancel.is_cancelled() {
                        stopped_for = StopCondition::Cancelled;
                        break 'outer;
                    }
                    let ContentBlock::ToolUse(tu) = block else { continue };

                    let tool_ctx = ToolCtx {
                        turn_index,
                        tool_use_id: &tu.id,
                        tool_name: &tu.name,
                        input: &tu.input,
                    };
                    let decision = match self.hooks.before_tool(&tool_ctx).await {
                        Ok(d) => d,
                        Err(e) => {
                            stopped_for = StopCondition::HookDenied(
                                format!("before_tool hook failed: {e}"),
                            );
                            break 'outer;
                        }
                    };

                    match decision {
                        HookDecision::Deny(msg) => {
                            let content = vec![ContentBlock::Text(TextBlock {
                                text: format!("Tool call denied: {msg}"),
                                cache_control: None,
                            })];
                            // Mirror dispatch_tool: notify after_tool of the denial.
                            let denial_err = ToolError::execution(std::io::Error::other(
                                format!("denied: {msg}"),
                            ));
                            if let Err(e) =
                                self.hooks.after_tool(&tool_ctx, &Err(denial_err)).await
                            {
                                tracing::warn!(
                                    tool = %tu.name, error = %e,
                                    "after_tool hook error (non-fatal)"
                                );
                            }
                            let result = ToolResultBlock {
                                tool_use_id: tu.id.clone(),
                                content,
                                is_error: true,
                            };
                            // Emit the denied ToolCallEnd up front, in
                            // assistant-message order.
                            yield TurnEvent::ToolCallEnd {
                                turn_index,
                                tool_use_id: result.tool_use_id.clone(),
                                is_error: true,
                                content: result.content.clone(),
                            };
                            plans.push(DispatchPlan::Denied {
                                original_index: idx,
                                result,
                            });
                        }
                        HookDecision::Allow => {
                            plans.push(DispatchPlan::Allowed {
                                original_index: idx,
                                id: tu.id.clone(),
                                name: tu.name.clone(),
                                input: tu.input.clone(),
                            });
                        }
                    }
                }

                // ---- Phase 2: dispatch (parallel invoke + after_tool) ----
                let permits = if self.parallel_tools {
                    self.parallel_tool_limit.get()
                } else {
                    1
                };
                let sem = Arc::new(Semaphore::new(permits));
                let dispatch_started_at = Instant::now();
                let agent_ref = &self;

                let mut ordered_results: Vec<Option<ToolResultBlock>> =
                    vec![None; assistant_message.content.len()];
                let mut denied_count: usize = 0;
                let mut dispatched_count: usize = 0;

                let mut pending = FuturesUnordered::new();
                for plan in plans {
                    match plan {
                        DispatchPlan::Denied { original_index, result } => {
                            denied_count += 1;
                            ordered_results[original_index] = Some(result);
                        }
                        DispatchPlan::Allowed { original_index, id, name, input } => {
                            if cancel.is_cancelled() {
                                stopped_for = StopCondition::Cancelled;
                                break;
                            }
                            dispatched_count += 1;
                            let permit = Arc::clone(&sem)
                                .acquire_owned()
                                .await
                                .expect("semaphore not closed");
                            let cancel_for_tool = cancel.clone();
                            pending.push(async move {
                                let _permit = permit; // released on drop
                                let res = dispatch_tool(
                                    agent_ref,
                                    turn_index,
                                    &id,
                                    &name,
                                    input,
                                    &cancel_for_tool,
                                )
                                .await;
                                (original_index, id, res)
                            });
                        }
                    }
                }

                // Drive the pending set. ToolCallEnd events fire in
                // completion order; ordered_results preserves history order.
                let mut fatal_stop: Option<StopCondition> = None;
                while let Some((idx, id, dispatch_res)) = pending.next().await {
                    match dispatch_res {
                        Err(stop) => {
                            fatal_stop = Some(stop);
                            // Continue draining the loop so no future escapes.
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

                let dispatch_elapsed = dispatch_started_at.elapsed();
                tracing::info!(
                    target: "caliban::tools",
                    turn = turn_index,
                    parallel_tools = self.parallel_tools,
                    parallel_tool_limit = self.parallel_tool_limit.get(),
                    dispatched = dispatched_count,
                    denied = denied_count,
                    total_wall_ms = u64::try_from(dispatch_elapsed.as_millis())
                        .unwrap_or(u64::MAX),
                    "parallel tool dispatch",
                );

                if let Some(stop) = fatal_stop {
                    stopped_for = stop;
                    break 'outer;
                }

                // ---- Phase 3: collect results in assistant-message order ----
                let mut tool_result_blocks: Vec<ContentBlock> = Vec::new();
                for slot in ordered_results.into_iter().flatten() {
                    tool_result_blocks.push(ContentBlock::ToolResult(slot));
                }
```

The lines that follow (`// Build the tool-results message (if any tools were called).` onward) stay unchanged. Make sure the `tool_result_blocks: Vec<ContentBlock>` variable they expect still exists with the same name.

- [ ] **Step 5: Check it compiles**

Run: `cargo check -p caliban-agent-core --all-targets`
Expected: Clean compile. Common issues to address inline:
- If `ToolError` isn't in scope where the deny synthesis happens: add `use crate::tool::ToolError;` to the imports.
- If `TextBlock` isn't imported (it already is at line 8-12): fine.
- If `Arc` collision with the existing `use std::sync::Arc`: dedupe.

- [ ] **Step 6: Run existing tests for regression**

Run: `cargo test -p caliban-agent-core`
Expected: All existing tests still pass. The refactor is behavior-preserving for single-tool turns (only one permit ever in flight) and for sequential mode (permits=1).

- [ ] **Step 7: Commit**

```bash
git add crates/caliban-agent-core/src/stream.rs
git commit -m "$(cat <<'EOF'
feat(agent-core): parallel tool dispatch via FuturesUnordered + Semaphore

Refactor the per-turn tool dispatch loop in stream_until_done from a
serial for-loop into a three-phase pipeline:
  1. Serial before_tool gate produces a Vec<DispatchPlan>
  2. Allowed plans run concurrently via FuturesUnordered, bounded by
     Arc<Semaphore> sized to parallel_tool_limit
  3. Results are reordered into assistant-message order for history

ToolCallEnd events stream in completion order (best TUI liveness).
Denied results emit up front in assistant-message order.
parallel_tools=false runs through the same path with permits=1, so
serial semantics are preserved with one extra hop.

caliban::tools tracing event captures dispatched/denied counts and
total wall time per turn so the perf baseline gains a parallel-tools
measurement.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Test infrastructure — `SleepyTool`, `TrackingTool`, helpers

**Files:**
- Create: `crates/caliban-agent-core/tests/parallel_tools.rs`

- [ ] **Step 1: Create the test file with shared helpers and one smoke test**

Create `crates/caliban-agent-core/tests/parallel_tools.rs`:

```rust
//! Integration tests for parallel tool dispatch.
//!
//! Every test uses `MockProvider` to script a turn with multiple
//! `tool_use` blocks. Tools are crafted to expose timing, ordering, and
//! concurrency invariants.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, ContentBlock, HookDecision, Hooks, Message, StopCondition, TextBlock, Tool, ToolContext,
    ToolCtx, ToolError, ToolRegistry, TurnEvent,
};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Test tools
// ---------------------------------------------------------------------------

/// A tool that sleeps for `delay`, then returns a single text block whose
/// content is the tool's name. Used to measure parallel vs serial wall time.
struct SleepyTool {
    name: String,
    delay: Duration,
    schema: serde_json::Value,
}

impl SleepyTool {
    fn new(name: &str, delay: Duration) -> Self {
        Self {
            name: name.to_string(),
            delay,
            schema: serde_json::json!({"type": "object"}),
        }
    }
}

#[async_trait]
impl Tool for SleepyTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "sleepy test tool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        tokio::select! {
            () = tokio::time::sleep(self.delay) => {}
            () = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
        }
        Ok(vec![ContentBlock::Text(TextBlock {
            text: self.name.clone(),
            cache_control: None,
        })])
    }
}

/// A tool that increments a shared `(current, peak)` counter on entry,
/// sleeps, then decrements `current`. Used to assert the semaphore cap.
struct TrackingTool {
    name: String,
    state: Arc<Mutex<(usize, usize)>>,
    delay: Duration,
    schema: serde_json::Value,
}

impl TrackingTool {
    fn new(name: &str, state: Arc<Mutex<(usize, usize)>>, delay: Duration) -> Self {
        Self {
            name: name.to_string(),
            state,
            delay,
            schema: serde_json::json!({"type": "object"}),
        }
    }
}

#[async_trait]
impl Tool for TrackingTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "tracking test tool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        {
            let mut s = self.state.lock().unwrap();
            s.0 += 1;
            s.1 = s.1.max(s.0);
        }
        tokio::time::sleep(self.delay).await;
        {
            let mut s = self.state.lock().unwrap();
            s.0 -= 1;
        }
        Ok(Vec::new())
    }
}

/// `Hooks` impl that denies tool calls whose name appears in `deny_names`.
struct DenyingHooks {
    deny_names: Vec<String>,
}

#[async_trait]
impl Hooks for DenyingHooks {
    async fn before_tool(
        &self,
        ctx: &ToolCtx<'_>,
    ) -> caliban_agent_core::Result<HookDecision> {
        if self.deny_names.iter().any(|n| n == ctx.tool_name) {
            Ok(HookDecision::Deny(format!("denied: {}", ctx.tool_name)))
        } else {
            Ok(HookDecision::Allow)
        }
    }
}

// ---------------------------------------------------------------------------
// Mock-provider scripting helpers
// ---------------------------------------------------------------------------

/// Stream events for an assistant turn that emits `tool_use` blocks for each
/// `(tool_use_id, name)` pair, all at distinct content-block indices, then
/// stops with `StopReason::ToolUse`.
fn parallel_tool_turn(tools: &[(&str, &str)]) -> Vec<caliban_provider::Result<StreamEvent>> {
    let mut events = Vec::new();
    events.push(Ok(StreamEvent::MessageStart {
        id: "msg_par".into(),
        model: "mock-model".into(),
    }));
    for (i, (id, name)) in tools.iter().enumerate() {
        let idx = u32::try_from(i).unwrap();
        events.push(Ok(StreamEvent::ContentBlockStart {
            index: idx,
            content_type: StreamingContentType::ToolUse {
                id: (*id).to_string(),
                name: (*name).to_string(),
            },
        }));
        events.push(Ok(StreamEvent::Delta {
            index: idx,
            delta: StreamingDelta::ToolUseInputJson("{}".into()),
        }));
        events.push(Ok(StreamEvent::ContentBlockStop { index: idx }));
    }
    events.push(Ok(StreamEvent::MessageDelta {
        stop_reason: Some(StopReason::ToolUse),
        usage_delta: Some(Usage::default()),
    }));
    events.push(Ok(StreamEvent::MessageStop));
    events
}

/// Stream events for a turn that produces an `EndTurn` with no content.
/// Used to terminate the run after the tool-call turn.
fn end_turn_events() -> Vec<caliban_provider::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "msg_end".into(),
            model: "mock-model".into(),
        }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

// ---------------------------------------------------------------------------
// Smoke test
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_tool_one_turn_still_works() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[("t1", "sleepy_a")]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(SleepyTool::new("sleepy_a", Duration::from_millis(5))));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .build()
        .unwrap();

    let mut stream =
        agent.stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    let mut tool_call_ends = 0;
    while let Some(ev) = stream.next().await {
        if let TurnEvent::ToolCallEnd { .. } = ev.unwrap() {
            tool_call_ends += 1;
        }
    }
    assert_eq!(tool_call_ends, 1);
}
```

- [ ] **Step 2: Verify the smoke test passes**

Run: `cargo test -p caliban-agent-core --test parallel_tools one_tool_one_turn_still_works`
Expected: PASS.

If `Message::user_text` doesn't exist, replace with an explicit construction. Check the crate exports:

```bash
grep -n "user_text\|fn user\b" crates/caliban-provider/src/message.rs
```

Use whichever helper exists (likely `Message::user_text`, defined on `caliban_provider::Message`). The token-aggregation test uses the same one — see `crates/caliban-agent-core/tests/token_aggregation.rs` for reference.

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-agent-core/tests/parallel_tools.rs
git commit -m "$(cat <<'EOF'
test(agent-core): parallel-tools test infrastructure + smoke test

SleepyTool (delays then returns), TrackingTool (peak concurrent counter),
DenyingHooks, plus parallel_tool_turn() helper that emits N tool_use
blocks at distinct content-block indices via MockProvider.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Wall-time parallel-wins test

**Files:**
- Modify: `crates/caliban-agent-core/tests/parallel_tools.rs`

- [ ] **Step 1: Append the test**

Add at the bottom of `crates/caliban-agent-core/tests/parallel_tools.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_is_faster_than_serial() {
    // Three tools, each sleeping 100ms.
    // Parallel (limit=3): ~100-150ms.
    // Serial: ~300-400ms.
    fn build() -> (Arc<MockProvider>, ToolRegistry) {
        let mp = Arc::new(MockProvider::new());
        mp.enqueue_stream(parallel_tool_turn(&[
            ("t1", "sleepy_a"),
            ("t2", "sleepy_b"),
            ("t3", "sleepy_c"),
        ]));
        mp.enqueue_stream(end_turn_events());
        let mut registry = ToolRegistry::default();
        let d = Duration::from_millis(100);
        registry.register(Arc::new(SleepyTool::new("sleepy_a", d)));
        registry.register(Arc::new(SleepyTool::new("sleepy_b", d)));
        registry.register(Arc::new(SleepyTool::new("sleepy_c", d)));
        (mp, registry)
    }

    async fn run_with(parallel: bool) -> Duration {
        let (mp, registry) = build();
        let agent = Agent::builder()
            .provider(mp as Arc<dyn Provider + Send + Sync>)
            .tools(registry)
            .model("mock-model")
            .max_tokens(64)
            .parallel_tools(parallel)
            .parallel_tool_limit(NonZeroUsize::new(3).unwrap())
            .build()
            .unwrap();
        let start = Instant::now();
        let mut s = agent.stream_until_done(
            vec![Message::user_text("hi")],
            CancellationToken::new(),
        );
        while let Some(ev) = s.next().await {
            ev.unwrap();
        }
        start.elapsed()
    }

    let parallel_wall = run_with(true).await;
    let serial_wall = run_with(false).await;

    // Parallel should be well under the 300ms serial baseline.
    assert!(
        parallel_wall < Duration::from_millis(200),
        "parallel wall {parallel_wall:?} should be < 200ms (3 × 100ms in parallel)"
    );
    // Serial must take at least ~3× tool latency.
    assert!(
        serial_wall >= Duration::from_millis(280),
        "serial wall {serial_wall:?} should be >= 280ms (3 × 100ms serially)"
    );
    // And parallel should be at least 2× faster than serial.
    assert!(
        parallel_wall.as_millis() * 2 < serial_wall.as_millis(),
        "parallel {parallel_wall:?} should be at least 2× faster than serial {serial_wall:?}"
    );
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p caliban-agent-core --test parallel_tools parallel_is_faster_than_serial`
Expected: PASS. Parallel ~120ms, serial ~310ms.

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-agent-core/tests/parallel_tools.rs
git commit -m "$(cat <<'EOF'
test(agent-core): parallel dispatch beats serial on wall-time

Three 100ms SleepyTool calls in one turn. Parallel completes in < 200ms;
serial baseline ≥ 280ms; parallel must be at least 2× faster.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: History order preserved + completion order in events

**Files:**
- Modify: `crates/caliban-agent-core/tests/parallel_tools.rs`

- [ ] **Step 1: Append the test**

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn history_in_assistant_order_events_in_completion_order() {
    // Three tools where C finishes first (5ms), then B (25ms), then A (50ms).
    // Assistant-message order: A, B, C.
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[
        ("ta", "a"),
        ("tb", "b"),
        ("tc", "c"),
    ]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(SleepyTool::new("a", Duration::from_millis(80))));
    registry.register(Arc::new(SleepyTool::new("b", Duration::from_millis(40))));
    registry.register(Arc::new(SleepyTool::new("c", Duration::from_millis(5))));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .parallel_tools(true)
        .parallel_tool_limit(NonZeroUsize::new(3).unwrap())
        .build()
        .unwrap();

    let mut stream =
        agent.stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());

    let mut event_order: Vec<String> = Vec::new();
    let mut final_messages: Vec<Message> = Vec::new();
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            TurnEvent::ToolCallEnd { tool_use_id, .. } => event_order.push(tool_use_id),
            TurnEvent::RunEnd { final_messages: fm, .. } => final_messages = fm,
            _ => {}
        }
    }

    // Completion order: c, b, a (shortest sleep first).
    assert_eq!(event_order, vec!["tc", "tb", "ta"], "ToolCallEnd must arrive in completion order");

    // History: locate the tool-results message and assert order ta, tb, tc.
    let tool_results_msg = final_messages
        .iter()
        .find(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult(_)))
        })
        .expect("tool-results message present in history");
    let history_ids: Vec<&str> = tool_results_msg
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult(tr) => Some(tr.tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(history_ids, vec!["ta", "tb", "tc"], "history must be in assistant-message order");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p caliban-agent-core --test parallel_tools history_in_assistant_order_events_in_completion_order`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-agent-core/tests/parallel_tools.rs
git commit -m "$(cat <<'EOF'
test(agent-core): completion-order events, history preserves assistant order

3 tools with descending delays (a=80ms, b=40ms, c=5ms). ToolCallEnd
events fire c,b,a; persisted tool_results message stays a,b,c so replay
remains deterministic.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Deny preserves history slot

**Files:**
- Modify: `crates/caliban-agent-core/tests/parallel_tools.rs`

- [ ] **Step 1: Append the test**

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn denied_tool_keeps_its_history_slot() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[
        ("ta", "a"),
        ("tb", "b"),
        ("tc", "c"),
    ]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(SleepyTool::new("a", Duration::from_millis(20))));
    registry.register(Arc::new(SleepyTool::new("b", Duration::from_millis(20))));
    registry.register(Arc::new(SleepyTool::new("c", Duration::from_millis(20))));

    let hooks = Arc::new(DenyingHooks {
        deny_names: vec!["b".to_string()],
    });

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .hooks(hooks)
        .model("mock-model")
        .max_tokens(64)
        .parallel_tools(true)
        .parallel_tool_limit(NonZeroUsize::new(3).unwrap())
        .build()
        .unwrap();

    let mut stream =
        agent.stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());

    let mut final_messages: Vec<Message> = Vec::new();
    let mut denied_seen = false;
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            TurnEvent::ToolCallEnd {
                tool_use_id,
                is_error,
                content,
                ..
            } if tool_use_id == "tb" => {
                denied_seen = true;
                assert!(is_error, "denied tool's ToolCallEnd must have is_error=true");
                let text = match &content[0] {
                    ContentBlock::Text(t) => t.text.clone(),
                    _ => panic!("expected text block in denial"),
                };
                assert!(
                    text.contains("denied"),
                    "denial content should mention denial; got {text:?}"
                );
            }
            TurnEvent::RunEnd { final_messages: fm, .. } => final_messages = fm,
            _ => {}
        }
    }
    assert!(denied_seen);

    let tool_results_msg = final_messages
        .iter()
        .find(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult(_)))
        })
        .expect("tool-results message present");
    let history_ids: Vec<&str> = tool_results_msg
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult(tr) => Some(tr.tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        history_ids,
        vec!["ta", "tb", "tc"],
        "denied tool must keep its slot in assistant-message order"
    );
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p caliban-agent-core --test parallel_tools denied_tool_keeps_its_history_slot`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-agent-core/tests/parallel_tools.rs
git commit -m "$(cat <<'EOF'
test(agent-core): denial preserves the tool's slot in history

DenyingHooks denies tool b. Tools a and c invoke in parallel; b's slot
in the tool_results message is the synthesized denial block. History
order remains a,b,c.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Cancellation drains in-flight

**Files:**
- Modify: `crates/caliban-agent-core/tests/parallel_tools.rs`

- [ ] **Step 1: Append the test**

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_drains_in_flight() {
    // Three tools sleeping 200ms each. Cancel after 20ms. All three must
    // observe the cancel and the run must terminate with Cancelled.
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[
        ("t1", "long_a"),
        ("t2", "long_b"),
        ("t3", "long_c"),
    ]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    let d = Duration::from_millis(200);
    registry.register(Arc::new(SleepyTool::new("long_a", d)));
    registry.register(Arc::new(SleepyTool::new("long_b", d)));
    registry.register(Arc::new(SleepyTool::new("long_c", d)));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .parallel_tools(true)
        .parallel_tool_limit(NonZeroUsize::new(3).unwrap())
        .build()
        .unwrap();

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_clone.cancel();
    });

    let start = Instant::now();
    let mut s = agent.stream_until_done(vec![Message::user_text("hi")], cancel);
    let mut stop_condition: Option<StopCondition> = None;
    while let Some(ev) = s.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev.unwrap() {
            stop_condition = Some(stopped_for);
        }
    }
    let elapsed = start.elapsed();

    assert!(
        matches!(stop_condition, Some(StopCondition::Cancelled)),
        "run must terminate with StopCondition::Cancelled; got {stop_condition:?}"
    );
    // Tools observe cancel quickly; the run should finish well before the
    // tool's full 200ms sleep on each. 150ms cap gives ample slack.
    assert!(
        elapsed < Duration::from_millis(150),
        "cancellation should propagate quickly; elapsed = {elapsed:?}"
    );
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p caliban-agent-core --test parallel_tools cancellation_drains_in_flight`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-agent-core/tests/parallel_tools.rs
git commit -m "$(cat <<'EOF'
test(agent-core): cancel drains in-flight tools and propagates

Three 200ms tools, cancel fired at 20ms. All tools observe the token,
run terminates with StopCondition::Cancelled, and total wall is < 150ms
(well under the 600ms a sequential cancel-then-drain would take).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Limit honored + serial-equivalence

**Files:**
- Modify: `crates/caliban-agent-core/tests/parallel_tools.rs`

- [ ] **Step 1: Append both tests**

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn semaphore_limit_caps_concurrency() {
    // Five tools, limit=2. Peak concurrent must be exactly 2.
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[
        ("t1", "track_1"),
        ("t2", "track_2"),
        ("t3", "track_3"),
        ("t4", "track_4"),
        ("t5", "track_5"),
    ]));
    mp.enqueue_stream(end_turn_events());

    let state = Arc::new(Mutex::new((0_usize, 0_usize)));
    let mut registry = ToolRegistry::default();
    let d = Duration::from_millis(40);
    for i in 1..=5 {
        registry.register(Arc::new(TrackingTool::new(
            &format!("track_{i}"),
            Arc::clone(&state),
            d,
        )));
    }

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .parallel_tools(true)
        .parallel_tool_limit(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();

    let mut s = agent.stream_until_done(
        vec![Message::user_text("hi")],
        CancellationToken::new(),
    );
    while let Some(ev) = s.next().await {
        ev.unwrap();
    }

    let peak = state.lock().unwrap().1;
    assert_eq!(peak, 2, "with limit=2, peak concurrent must be exactly 2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_tools_false_is_serial() {
    // Same setup as above but parallel_tools=false → peak must be 1.
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[
        ("t1", "track_1"),
        ("t2", "track_2"),
        ("t3", "track_3"),
    ]));
    mp.enqueue_stream(end_turn_events());

    let state = Arc::new(Mutex::new((0_usize, 0_usize)));
    let mut registry = ToolRegistry::default();
    let d = Duration::from_millis(30);
    for i in 1..=3 {
        registry.register(Arc::new(TrackingTool::new(
            &format!("track_{i}"),
            Arc::clone(&state),
            d,
        )));
    }

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .parallel_tools(false)
        // limit is ignored when parallel_tools=false; set high to prove that.
        .parallel_tool_limit(NonZeroUsize::new(8).unwrap())
        .build()
        .unwrap();

    let mut s = agent.stream_until_done(
        vec![Message::user_text("hi")],
        CancellationToken::new(),
    );
    let mut event_order: Vec<String> = Vec::new();
    while let Some(ev) = s.next().await {
        if let TurnEvent::ToolCallEnd { tool_use_id, .. } = ev.unwrap() {
            event_order.push(tool_use_id);
        }
    }

    let peak = state.lock().unwrap().1;
    assert_eq!(peak, 1, "with parallel_tools=false, peak concurrent must be 1");
    // Serial mode: tools execute in assistant-message order, so events
    // also stream in that order.
    assert_eq!(event_order, vec!["t1", "t2", "t3"]);
}
```

- [ ] **Step 2: Run both tests**

Run: `cargo test -p caliban-agent-core --test parallel_tools semaphore_limit_caps_concurrency parallel_tools_false_is_serial`
Expected: Both PASS.

- [ ] **Step 3: Run the entire parallel_tools test file to catch any cross-test interaction**

Run: `cargo test -p caliban-agent-core --test parallel_tools`
Expected: All 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/caliban-agent-core/tests/parallel_tools.rs
git commit -m "$(cat <<'EOF'
test(agent-core): semaphore caps concurrency; parallel_tools=false is serial

TrackingTool exposes a peak-concurrent counter through a shared Mutex.
With limit=2 and 5 tools, peak is exactly 2. With parallel_tools=false
and a 3-tool turn, peak is 1 and ToolCallEnd events stream in
assistant-message order (preserving today's behavior).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Hooks::after_tool doc-comment update

**Files:**
- Modify: `crates/caliban-agent-core/src/hooks.rs`

- [ ] **Step 1: Update the `after_tool` doc-comment**

Find the `after_tool` trait method (around line 67) and replace its doc-comment:

```rust
    /// Called after each tool invocation (or denial) with the result.
    ///
    /// **Ordering note:** Under parallel tool dispatch (the default), this
    /// hook fires once per tool but **not** in assistant-message order —
    /// it fires in completion order. Each call carries the tool's
    /// `tool_use_id` and `tool_name` in [`ToolCtx`] so implementors can
    /// correlate. For denials (returned by [`Hooks::before_tool`]), this
    /// hook still fires once with `Err(ToolError::Execution(...))`.
    async fn after_tool(
        &self,
        _ctx: &ToolCtx<'_>,
        _result: &std::result::Result<Vec<ContentBlock>, ToolError>,
    ) -> Result<()> {
        Ok(())
    }
```

- [ ] **Step 2: Confirm rustdoc still builds**

Run: `cargo doc -p caliban-agent-core --no-deps 2>&1 | tail -20`
Expected: No warnings about broken intra-doc links from the updated comment.

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-agent-core/src/hooks.rs
git commit -m "$(cat <<'EOF'
docs(agent-core): clarify after_tool fires in completion order under parallel dispatch

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: CLI flags in main.rs

**Files:**
- Modify: `caliban/src/main.rs`

- [ ] **Step 1: Add the import for `NonZeroUsize`**

Find the existing `use std::path::PathBuf;` block (around line 11) and add:

```rust
use std::num::NonZeroUsize;
```

- [ ] **Step 2: Add the two CLI flags**

In the `pub(crate) struct Args` block, after the existing `no_prompt_cache` field (around line 137), add:

```rust
    /// Disable parallel tool execution (run tool_use blocks serially).
    #[arg(long, env = "CALIBAN_NO_PARALLEL_TOOLS")]
    pub(crate) no_parallel_tools: bool,

    /// Max concurrent tool invocations per turn. Defaults to CPU cores - 1 (min 1).
    #[arg(long, value_name = "N", env = "CALIBAN_PARALLEL_TOOL_LIMIT")]
    pub(crate) parallel_tool_limit: Option<NonZeroUsize>,
```

- [ ] **Step 3: Wire the flags into the AgentBuilder**

Find the builder construction (around line 390) and chain the new methods after `.prompt_cache(...)`:

```rust
    let mut builder = Agent::builder()
        .provider(provider)
        .tools(registry)
        .model(model.clone())
        .max_tokens(args.max_tokens)
        .max_turns(args.max_turns)
        .prompt_cache(!args.no_prompt_cache)
        .parallel_tools(!args.no_parallel_tools);
    if let Some(limit) = args.parallel_tool_limit {
        builder = builder.parallel_tool_limit(limit);
    }
    if let Some(t) = args.temperature {
        builder = builder.temperature(t);
    }
```

- [ ] **Step 4: Build the binary**

Run: `cargo build -p caliban`
Expected: Clean build.

- [ ] **Step 5: Help-text smoke test**

Run: `cargo run -p caliban --quiet -- --help 2>&1 | grep -E "parallel|prompt-cache"`
Expected: Output includes `--no-parallel-tools`, `--parallel-tool-limit <N>`, and the existing `--no-prompt-cache`.

- [ ] **Step 6: Commit**

```bash
git add caliban/src/main.rs
git commit -m "$(cat <<'EOF'
feat(cli): --no-parallel-tools and --parallel-tool-limit flags

Env vars CALIBAN_NO_PARALLEL_TOOLS and CALIBAN_PARALLEL_TOOL_LIMIT
mirror the flags. Plumbed into AgentBuilder::parallel_tools and
::parallel_tool_limit.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Full-suite verification

**Files:**
- None modified.

- [ ] **Step 1: Workspace test**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 2: Workspace clippy (matches CI's default profile)**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: No warnings.

- [ ] **Step 3: ci-cloud clippy (matches feature-gated transports built in ci-cloud)**

Run: `cargo clippy --workspace --all-targets --features caliban-provider-anthropic/bedrock,caliban-provider-anthropic/vertex,caliban-provider-openai/azure,caliban-provider-google/vertex -- -D warnings`
Expected: No warnings.

- [ ] **Step 4: Open PR**

```bash
git push -u origin jf/feat/parallel-tools
gh pr create --title "feat: parallel tool execution per turn" --body "$(cat <<'EOF'
## Summary

- Dispatch a single assistant turn's `tool_use` blocks concurrently via `FuturesUnordered` + `tokio::sync::Semaphore`.
- Default ON; cap defaults to `available_parallelism().get() - 1` (min 1). Opt out with `--no-parallel-tools`; override the cap with `--parallel-tool-limit N`.
- `ToolCallEnd` events stream in **completion order** (best TUI liveness). Stored history preserves **assistant-message order** so replay/serialization stay deterministic.
- `before_tool` remains serial (the deny gate). `after_tool` now fires in completion order — documented in the trait.
- New `caliban::tools` tracing event captures dispatched/denied counts and total wall time per turn.

Spec: `docs/superpowers/specs/2026-05-23-parallel-tools-design.md`
Plan: `docs/superpowers/plans/2026-05-23-parallel-tools.md`

## Test plan

- [x] 5 new unit tests for the Agent builder fields + default cap helper
- [x] 6 new integration tests in `crates/caliban-agent-core/tests/parallel_tools.rs`:
  - smoke (1 tool still works)
  - parallel wall-time beats serial by >= 2× (3 × 100ms tools)
  - `ToolCallEnd` arrives in completion order; history stays in assistant order
  - denied tool keeps its slot in the persisted tool-results message
  - cancellation drains in-flight tools quickly (< 150ms vs 600ms sequential)
  - `parallel_tool_limit=2` caps peak concurrent at 2; `parallel_tools(false)` caps at 1 and preserves event order
- [x] `cargo test --workspace` passes
- [x] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [x] ci-cloud clippy (feature-gated transports) clean
- [ ] Manual: run an interactive session with multi-tool turns and confirm the TUI shows tools completing inline as they finish (best-effort — needs an API key + workload that triggers parallel tool_use blocks).

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-review notes

Spec coverage check (each section of `docs/superpowers/specs/2026-05-23-parallel-tools-design.md` → task):

- "Default behavior" (default-on, cap = cores - 1) → Task 1 (Steps 3, 5)
- "Agent surface" (builder methods) → Task 1 (Steps 4, 6, 7)
- "Dispatch loop" (3-phase) → Task 2
- "Cancellation" (drain semantics) → Task 2 + Task 7 (test)
- "Tracing" (`caliban::tools` event) → Task 2 (Step 4)
- "CLI plumbing" → Task 10
- "Testing" (6 listed tests) → Tasks 3-8 (6 tests: smoke, wall-time, ordering, deny, cancel, limit + serial equivalence)
- "Hook ordering" doc update → Task 9
- "Risks" (concurrent FS writes, fan-out, span interleaving) → no code change required; documented in PR body via spec link

No spec requirement is unmapped.

Type/name consistency check:

- `DispatchPlan::Allowed { original_index, id, name, input }` defined in Task 2 Step 2; used in Task 2 Step 4. ✓
- `DispatchPlan::Denied { original_index, result }` defined Task 2 Step 2; used Step 4. ✓
- `default_parallel_tool_limit` defined Task 1 Step 3; called from `Default for AgentBuilder` (Task 1 Step 5) and re-exported (Task 1 Step 8). ✓
- `parallel_tools` / `parallel_tool_limit` field names consistent across `Agent`, `AgentBuilder`, builder methods, tests, and CLI plumbing. ✓
- `caliban::tools` tracing target consistent between Task 2 spec and code. ✓
