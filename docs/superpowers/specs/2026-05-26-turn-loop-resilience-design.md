# Turn-loop resilience — Design

**Date:** 2026-05-26
**Author:** john.ford2002@gmail.com
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** *(none yet — propose 0041 if this lands)*
**Origin:** `docs/TODO.md` findings on MaxTokens hang, stream idle, refusal silence, reactive compaction, stalled-tokens UI, hook death-spiral, and after_turn redirect (from the 2026-05-25 ADR conformance audit + lmstudio probe).

## Goal

Make caliban's agent loop *survive* the unhappy paths every long-running session eventually hits. Today a single dropped TCP socket, a reasoning-heavy gpt-5 turn, a 413, or a model refusal all surface as either a silent hang or an indistinguishable "ended OK" RunEnd. After this lands:

1. **MaxTokens** turns recover via budget escalation, then meta-continuation, then surrender — never an invisible halt.
2. **Stream death** (silent TCP drop mid-SSE) aborts within ~90 s with a warning and a clean error, not a forever-hang.
3. **Refusals / content-filter** halts emit a synthetic assistant message and a distinct `StopCondition`.
4. **Prompt-too-long (HTTP 413 / `ContextTooLong`)** triggers one reactive compaction + retry before surrendering.
5. **TUI** shows a "no tokens for Ns" hint after 3 s of stream silence so users can distinguish "still thinking" from "network died".
6. **Failure hooks** can't drive a death-spiral: `after_run`/`after_turn` are gated on success; a sibling `after_run_failure` runs on terminal errors.
7. **`after_turn`** can request continuation via a typed `TurnDecision`, enabling stop-hook style "you didn't actually write the file" patterns.

## Non-goals

- **No new compaction strategy.** Reactive compaction reuses the existing `Compactor` trait. (Strategy expansion lives in Spec B.)
- **No provider-specific retry policies beyond MaxTokens & stream idle.** Other 5xx / rate-limit paths already have provider-level handling; this spec only adds the two missing ones.
- **No new permission flow.** Stage B's meta-continuation injects a system-controlled message, not a user-controlled one — no `PermissionRequest` event fires.
- **No headless-only changes.** All behavior is shared between TUI and `--print`; only the spinner/text rendering is TUI-only.
- **No backwards-incompatible breaks on `Hooks`.** The trait gains methods with default `Continue`/no-op impls — existing impls compile unchanged.

## Architecture

```
caliban-agent-core
  stream/mod.rs (turn loop)
    inner_loop_state
      turn_index
      max_tokens_recovery_count    ← NEW (per-run, capped)
      meta_continuation_count       ← NEW (per-run, capped at 3)
      attempted_reactive_compact    ← NEW (per-run, one-shot)
    ┌─ recover_max_tokens(turn_stop_reason, …)
    │     Stage A: re-issue with ESCALATED_MAX_TOKENS (16_384) once
    │     Stage B: append meta user msg, continue, capped × 3
    │     Stage C: yield StopCondition::MaxTokensExhausted
    ├─ recover_context_too_long(err, …)
    │     compactor.compact(history, caps)
    │     re-issue once; on second hit → ProviderError surrenders
    ├─ surface_refusal / surface_content_filter
    │     synthesize assistant TextBlock("Model declined to respond.")
    │     yield StopCondition::Refusal(reason) | ContentFilter(reason)
    └─ failure-aware hook dispatch
          if stopped_for.is_failure() → hooks.after_run_failure(…)
          else                         → hooks.after_run(…)

caliban-provider (and per-provider stream_parse.rs)
  stream::WatchedStream<S>             ← NEW wrapper
    poll_next:
      tokio::time::timeout(idle, inner.next())
        Ok(chunk) → reset, yield
        Err(_)    → first timeout → emit warning event, double budget
                    second timeout → abort, ProviderError::StreamIdle
  each provider's stream_parse:
    replace `while let Some(item) = sse.next().await`
      with    WatchedStream::new(sse, idle).await

caliban (TUI)
  tui::app::App
    last_delta_at: Instant            ← NEW
    has_active_tools: bool            ← already tracked, just read it
  tui::events.rs
    on AssistantTextDelta / ToolCallInputDelta → app.last_delta_at = now
    on ToolUseStart                            → also reset
  tui::render.rs
    spinner cell:
      let idle = now - app.last_delta_at;
      if idle > 3s && !app.has_active_tools
        → dim/warm color + " (no tokens for {idle:.0}s)"

caliban-agent-core
  hooks.rs
    trait Hooks {
      // existing
      async fn after_run(&self, ctx, &RunHookOutcome)         → unchanged signature
      async fn after_turn(&self, ctx, &TurnOutcome)           → NEW return: Result<TurnDecision>
      // new
      async fn after_run_failure(&self, ctx, &RunHookOutcome) → default Ok(())
      async fn after_turn_failure(&self, ctx, &TurnOutcome)   → default Ok(())
    }
    enum TurnDecision { Continue, ContinueWith(Vec<Message>), Stop }
```

## Data model deltas

### `StopCondition` (`crates/caliban-agent-core/src/stream/mod.rs:170`)

```rust
pub enum StopCondition {
    EndOfTurn,
    MaxTurnsReached(u32),
    Cancelled,
    ProviderError(String),
    HookDenied(String),
    CompactionFailed(String),
    // ── new ───────────────────────────────────────────────
    /// MaxTokens was hit and Stage A + Stage B recovery both surrendered.
    MaxTokensExhausted,
    /// Provider returned `stop_reason: Refusal`; synthetic message
    /// already in `final_messages`.
    Refusal(String),
    /// Provider returned `stop_reason: ContentFilter`; synthetic message
    /// already in `final_messages`.
    ContentFilter(String),
    /// SSE/HTTP stream went silent past the idle timeout; both warning
    /// and abort were emitted.
    StreamIdle(std::time::Duration),
}

impl StopCondition {
    /// True for stop conditions that indicate failure, not natural completion.
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::ProviderError(_) | Self::HookDenied(_) | Self::CompactionFailed(_)
                | Self::MaxTokensExhausted | Self::Refusal(_) | Self::ContentFilter(_)
                | Self::StreamIdle(_)
        )
    }
}
```

### `TurnDecision` (new, hooks.rs)

```rust
/// Returned by `after_turn` to influence the loop without breaking it.
#[derive(Debug, Clone)]
pub enum TurnDecision {
    /// Default — proceed as the loop would otherwise (continue if there
    /// were `ToolUse` results, otherwise stop).
    Continue,
    /// Append these messages to history and force another turn iteration,
    /// regardless of stop_reason. Capped at `MAX_FORCED_CONTINUATIONS = 3`.
    ContinueWith(Vec<Message>),
    /// Hard-stop the run immediately. Becomes `StopCondition::HookDenied("after_turn: Stop")`.
    Stop,
}
```

### Provider error use

`crates/caliban-provider/src/error.rs:23-30` already defines:

```rust
ContextTooLong { max_tokens: u32, requested_tokens: u32 },
```

That's the detection point — no new variant needed. Spec A *uses* this; Spec B's reactive-compaction handler turns it into a recovery instead of surrender.

### `AgentConfig` additions (`caliban-agent-core/src/config.rs`)

```rust
pub struct AgentConfig {
    // existing fields…
    /// Stage A escalated max_tokens budget. Default: 16_384.
    pub escalated_max_tokens: u32,
    /// Stage B meta-continuation cap. Default: 3.
    pub max_meta_continuations: u8,
    /// Stream idle timeout (ms). Default: 90_000. Overridable via env.
    pub stream_idle_timeout_ms: u32,
}
```

Env overrides:
- `CALIBAN_STREAM_IDLE_TIMEOUT_MS=90000`
- `CALIBAN_MAX_TOKENS_RECOVERY=true` (gate Stage A; default on)
- `CALIBAN_META_CONTINUATION_MAX=3`

## Recovery flows in detail

### MaxTokens — Stage A → B → C

```
turn ends with stop_reason == MaxTokens, tool_results empty
│
├─ if !max_tokens_recovery_attempted_this_turn && config.max_tokens_recovery:
│     // Stage A
│     override_max_tokens_for_next_request = ESCALATED_MAX_TOKENS
│     max_tokens_recovery_attempted_this_turn = true
│     re-issue current turn's request    ← does NOT count as a new turn
│
├─ else if meta_continuation_count < config.max_meta_continuations:
│     // Stage B
│     history.push(Message::user_text(META_CONTINUATION_PROMPT));
│     meta_continuation_count += 1
│     continue 'outer                    ← counts as the next turn
│
└─ else:
      // Stage C
      stopped_for = StopCondition::MaxTokensExhausted
      break 'outer
```

`META_CONTINUATION_PROMPT` (single const, ~`crates/caliban-agent-core/src/recovery.rs`):

```
Output token limit hit. Resume directly — no apology, no recap. Pick up
mid-thought. Break remaining work into smaller pieces.
```

### Stream idle watchdog

`crates/caliban-provider/src/stream.rs` gets `WatchedStream<S: Stream>`. Each provider's `stream_parse.rs` wraps its SSE reader:

```rust
// before
while let Some(item) = sse.next().await { handle(item)? }

// after
let mut watched = WatchedStream::new(sse, idle_timeout);
while let Some(item) = watched.next().await? { handle(item)? }
```

`WatchedStream::next` returns:
- `Ok(Some(item))` on normal data; resets the timer.
- `Ok(None)` on EOF.
- `Err(ProviderError::StreamIdle(elapsed))` after second timeout (after the warning).

A `tracing::warn!(target: "caliban::stream", elapsed_ms, "stream idle")` fires at the half-time warning so debug.log post-mortems are obvious.

### Refusal / ContentFilter surfacing

`StopReason::Refusal` already parses from each provider. In the post-turn branch (`stream/mod.rs:903`):

```rust
match turn_stop_reason {
    StopReason::ToolUse => { /* continue */ }
    StopReason::MaxTokens => { /* recovery flow above */ }
    StopReason::Refusal => {
        let synth = "Model declined to respond.";
        history.push(Message::assistant_text(synth));
        stopped_for = StopCondition::Refusal(synth.into());
        break 'outer;
    }
    StopReason::ContentFilter => {
        let synth = "Response blocked by content filter.";
        history.push(Message::assistant_text(synth));
        stopped_for = StopCondition::ContentFilter(synth.into());
        break 'outer;
    }
    _ => { stopped_for = StopCondition::EndOfTurn; break 'outer; }
}
```

The text strings are deliberately terse and model-neutral so 3P providers don't get Anthropic-flavored copy.

### Reactive compaction (ContextTooLong)

Sits inside the per-turn request issuance, **not** the existing pre-turn `compact()` step. Pseudocode:

```rust
async fn issue_with_reactive_compact(…) -> Result<TurnResponse, ProviderError> {
    match provider.stream(req).await {
        Err(ProviderError::ContextTooLong { .. }) if !attempted_reactive_compact => {
            if let Some(new) = compactor.compact(&history, &caps).await? {
                history = new;
                attempted_reactive_compact = true;
                req = rebuild_request(&history, …);
                provider.stream(req).await         // retry once
            } else {
                Err(ProviderError::ContextTooLong { … })
            }
        }
        other => other,
    }
}
```

`attempted_reactive_compact` is **per-run**, not per-turn — once compaction has been used reactively in a run, a second 413 surrenders to `StopCondition::ProviderError(…)` rather than spinning. The autocompact tracker (Spec B) holds the proactive side; this side handles the surprise case.

### Stalled-tokens TUI signal

State: `App.last_delta_at: Instant` (init to `Instant::now()`). Updated in `tui/events.rs` on:
- `Event::AssistantTextDelta`
- `Event::ToolCallInputDelta`
- `Event::ToolUseStart` (reset, since tool dispatch is its own form of progress)

Render (in `tui/render.rs` spinner cell):

```rust
let elapsed = now.duration_since(app.last_delta_at);
let stalled = elapsed > Duration::from_secs(3) && !app.has_active_tools;
let style = if stalled { Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM) }
            else        { default_style };
let suffix = if stalled && elapsed > Duration::from_secs(10) {
    format!(" (no tokens for {}s)", elapsed.as_secs())
} else { String::new() };
```

Threshold and color are not configurable in v1 (YAGNI); env var can be added later if the constants prove annoying.

### Hook death-spiral guard

`stream/mod.rs:911-933` currently invokes `hooks.after_run` unconditionally. Change:

```rust
let outcome = RunHookOutcome { …, success: !stopped_for.is_failure() };
if stopped_for.is_failure() {
    if let Err(e) = self.hooks.after_run_failure(&run_ctx, &outcome).await {
        tracing::warn!(error = %e, "after_run_failure hook error (non-fatal)");
    }
} else {
    if let Err(e) = self.hooks.after_run(&run_ctx, &outcome).await {
        tracing::warn!(error = %e, "after_run hook error (non-fatal)");
    }
}
```

Same split applies for `after_turn` (currently called *before* the continue/halt decision; needs to be moved or duplicated to maintain ordering — see plan).

### `after_turn` redirect

Trait change:

```rust
async fn after_turn(&self, _ctx: &TurnCtx<'_>, _outcome: &crate::TurnOutcome)
    -> Result<TurnDecision> { Ok(TurnDecision::Continue) }
```

Loop integration (after the existing continue/halt decision but **before** `break 'outer`):

```rust
let decision = self.hooks.after_turn(&turn_ctx, &turn_outcome).await
    .unwrap_or(TurnDecision::Continue);

match decision {
    TurnDecision::Continue => { /* keep existing decision */ }
    TurnDecision::ContinueWith(msgs) if forced_continuations < MAX_FORCED_CONTINUATIONS => {
        history.extend(msgs);
        forced_continuations += 1;
        continue 'outer;            // re-enter regardless of stop_reason
    }
    TurnDecision::ContinueWith(_) => {
        tracing::warn!("after_turn ContinueWith ignored (forced_continuations cap hit)");
    }
    TurnDecision::Stop => {
        stopped_for = StopCondition::HookDenied("after_turn: Stop".into());
        break 'outer;
    }
}
```

`MAX_FORCED_CONTINUATIONS = 3` is a const. The cap prevents runaway hooks from spinning a session forever.

## Error handling

| Failure mode | Today | After |
|---|---|---|
| gpt-5 burns budget on reasoning | Silent halt, `RunEnd { EndOfTurn }` | Stage A escalates → Stage B meta-prompt → Stage C surrenders with `MaxTokensExhausted` |
| SSE drops mid-response | Forever-hangs | After 45 s warning + 90 s abort → `StopCondition::StreamIdle(90s)` |
| `stop_reason: Refusal` | Silent halt | Synthetic assistant message + `StopCondition::Refusal(...)` |
| `stop_reason: ContentFilter` | Silent halt | Synthetic assistant message + `StopCondition::ContentFilter(...)` |
| HTTP 413 / `ContextTooLong` | Run surrenders immediately | Once-per-run reactive compact → retry |
| Hook spirals on error | Easy | `after_run_failure` is the only call path on failure; users must opt in to mutate state |
| Hook wants to redirect | Impossible | `after_turn` returns `ContinueWith(msgs)`, capped at 3 |

## Testing strategy

Each recovery flow gets a `MockProvider`-driven integration test (the `caliban-provider` crate already has `feature = "mock"`):

1. **MaxTokens Stage A:** mock returns `stop_reason: MaxTokens, output_tokens: cap` on turn 1, normal `EndTurn` on the retry with escalated budget. Assert `total_usage` includes both calls and `stopped_for == EndOfTurn`.
2. **MaxTokens Stage B:** mock returns `MaxTokens` on every call; Stage A fires once, Stage B fires `max_meta_continuations` times, then surrender. Assert `stopped_for == MaxTokensExhausted` and `history` contains exactly `max_meta_continuations` injected user messages.
3. **Stream idle:** mock provider's stream sleeps past `idle_timeout`. Assert one warning event, abort, `stopped_for == StreamIdle(_)`.
4. **Refusal:** mock returns `stop_reason: Refusal`. Assert history has a synthetic assistant message and `stopped_for == Refusal(_)`.
5. **Reactive compact:** mock fails turn 1 with `ContextTooLong`, succeeds turn 2 after `Compactor::compact` ran. Use a `RecordingCompactor` to assert exactly one call.
6. **Hook death-spiral:** install a `Hooks` impl whose `after_run` appends a sentinel message; trigger a `ProviderError` run; assert the sentinel did **not** land (`after_run_failure` ran instead).
7. **TurnDecision::ContinueWith cap:** install a `Hooks` impl that always returns `ContinueWith(vec![msg])`; assert exactly `MAX_FORCED_CONTINUATIONS` injections then surrender.

TUI stall test: render-snapshot at `last_delta_at = now - 5s` with `has_active_tools = false` → snapshot includes the stalled style + suffix.

## Telemetry

- New tracing events on each recovery transition, all under `target: caliban::recovery`:
  - `recovery.max_tokens.stage_a`
  - `recovery.max_tokens.stage_b` (with `meta_continuation_count`)
  - `recovery.max_tokens.stage_c`
  - `recovery.stream_idle.warning` / `recovery.stream_idle.abort`
  - `recovery.reactive_compact.fired` (with tokens-before/after)
  - `recovery.refusal` / `recovery.content_filter`
- Metric counters (in `caliban-telemetry`): `caliban.recovery.max_tokens_recovered`, `caliban.recovery.stream_idle_aborted`, `caliban.recovery.reactive_compacted`, `caliban.recovery.refusals_surfaced`.

These feed the same OTLP exporter as the existing metrics — no new exporter wiring.

## Migration notes

- **`after_turn` signature change** is the only soft breaking change. Custom `Hooks` impls in the workspace (`caliban-checkpoint`, the head's `CompositeHooks`) need to update return types from `Result<()>` to `Result<TurnDecision>`. Default body is `Ok(TurnDecision::Continue)`. No external `Hooks` impls exist yet (caliban is pre-1.0).
- **`StopCondition` new variants** are non-breaking for trait code (the enum isn't `#[non_exhaustive]`, but all matches in this workspace already use a default arm; we'll add explicit arms in `tui/events.rs` for the new variants).
- **`StreamIdle` may surface on flaky networks** that previously hung — net improvement, but operators should know the timeout exists. Documented in `--help` and `docs/parity-gap-matrix.md`.

## Open questions

1. **Stage A escalation budget.** 16_384 is the Claude Code default; for gpt-5 the practical reasoning ceiling is closer to 64K. Use 16_384 in v1, revisit when adding a `effort: max` reasoning preset (Spec C).
2. **Should `StreamIdle` retry non-streaming?** Claude Code does. Initial proposal: yes, behind a `CALIBAN_STREAM_IDLE_RETRY_NONSTREAMING=true` env; default off in v1 because it complicates the per-provider request shape.
3. **Failure-hook ordering.** When both `after_turn_failure` and `after_run_failure` would fire (e.g., the failing turn was also the last turn), do we fire both or skip `after_turn_failure` since `after_run_failure` covers it? Proposal: fire both; document the contract.

These are non-blocking — pick conservative defaults and revisit if they bite.
