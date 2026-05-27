# Turn-loop resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make caliban's agent loop survive MaxTokens, dropped SSE connections, refusals, content-filter halts, prompt-too-long errors, and hooks that would otherwise drive death-spirals.

**Architecture:** Extends `StopCondition` with 4 new variants; adds Stage A (budget escalation) + Stage B (meta-continuation) recovery for MaxTokens; wraps every provider's SSE stream in `WatchedStream` for an idle watchdog; routes refusal/content-filter through a synthetic-message path; adds reactive compaction triggered by `ProviderError::ContextTooLong`; gates `after_run`/`after_turn` on failure to call sibling `*_failure` hooks; gives `after_turn` a `TurnDecision` return so hooks can request continuation.

**Tech Stack:** Rust 1.85.0 (edition 2024), `tokio`, `async-trait`, `futures`, `tracing`, `arc-swap`, `caliban-provider` (`feature = "mock"` for tests).

**Spec:** [`docs/superpowers/specs/2026-05-26-turn-loop-resilience-design.md`](../specs/2026-05-26-turn-loop-resilience-design.md)

---

## File Structure

```
crates/caliban-agent-core/src/
├── stream/mod.rs              MODIFY: StopCondition variants, recovery branching
├── stream/recovery.rs         CREATE: MaxTokens + reactive-compact + refusal helpers
├── hooks.rs                   MODIFY: after_turn → Result<TurnDecision>, add after_*_failure
├── config.rs                  MODIFY: AgentConfig recovery knobs + Stage A/B caps

crates/caliban-provider/src/
├── stream.rs                  MODIFY: add WatchedStream<S>

crates/caliban-provider-anthropic/src/stream_parse.rs   MODIFY: wrap SSE in WatchedStream
crates/caliban-provider-openai/src/stream_parse.rs      MODIFY: wrap SSE in WatchedStream
crates/caliban-provider-google/src/stream_parse.rs      MODIFY: wrap SSE in WatchedStream
crates/caliban-provider-ollama/src/stream_parse.rs      MODIFY: wrap SSE in WatchedStream

caliban/src/tui/
├── app.rs                     MODIFY: add last_delta_at: Instant
├── events.rs                  MODIFY: update last_delta_at on deltas/tool starts; new StopCondition arms
├── render.rs                  MODIFY: spinner stalled-style branch

crates/caliban-agent-core/tests/
├── recovery_max_tokens.rs     CREATE: Stage A + B + C integration tests
├── recovery_refusal.rs        CREATE: refusal/content-filter surfacing tests
├── recovery_context.rs        CREATE: reactive-compact integration test
├── recovery_stream_idle.rs    CREATE: WatchedStream behavior + provider integration
├── hook_failure.rs            CREATE: after_run_failure / death-spiral guard
└── hook_turn_decision.rs      CREATE: TurnDecision::ContinueWith + cap
```

---

## Task 1: Add new `StopCondition` variants

**Files:**
- Modify: `crates/caliban-agent-core/src/stream/mod.rs` (around line 170)
- Test: `crates/caliban-agent-core/src/stream/mod.rs` (inline test module)

- [ ] **Step 1: Write the failing test**

Append to the existing inline tests in `stream/mod.rs` (or create one if none):

```rust
#[cfg(test)]
mod stop_condition_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn is_failure_classifies_correctly() {
        assert!(!StopCondition::EndOfTurn.is_failure());
        assert!(!StopCondition::MaxTurnsReached(5).is_failure());
        assert!(!StopCondition::Cancelled.is_failure());
        assert!(StopCondition::ProviderError("x".into()).is_failure());
        assert!(StopCondition::HookDenied("x".into()).is_failure());
        assert!(StopCondition::CompactionFailed("x".into()).is_failure());
        assert!(StopCondition::MaxTokensExhausted.is_failure());
        assert!(StopCondition::Refusal("x".into()).is_failure());
        assert!(StopCondition::ContentFilter("x".into()).is_failure());
        assert!(StopCondition::StreamIdle(Duration::from_secs(90)).is_failure());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-agent-core stop_condition_tests`
Expected: FAIL — `MaxTokensExhausted`, `Refusal`, `ContentFilter`, `StreamIdle` and `is_failure` are unknown.

- [ ] **Step 3: Add the variants and `is_failure`**

In `crates/caliban-agent-core/src/stream/mod.rs`, replace the existing `StopCondition` enum and add the impl:

```rust
#[derive(Debug, Clone)]
pub enum StopCondition {
    EndOfTurn,
    MaxTurnsReached(u32),
    Cancelled,
    ProviderError(String),
    HookDenied(String),
    CompactionFailed(String),
    /// MaxTokens hit and Stage A + Stage B recovery both surrendered.
    MaxTokensExhausted,
    /// `stop_reason: Refusal` from the provider; synthetic message already in `final_messages`.
    Refusal(String),
    /// `stop_reason: ContentFilter` from the provider; synthetic message already in `final_messages`.
    ContentFilter(String),
    /// SSE/HTTP stream went silent past the idle timeout.
    StreamIdle(std::time::Duration),
}

impl StopCondition {
    /// True for stop conditions that indicate failure, not natural completion.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::ProviderError(_)
                | Self::HookDenied(_)
                | Self::CompactionFailed(_)
                | Self::MaxTokensExhausted
                | Self::Refusal(_)
                | Self::ContentFilter(_)
                | Self::StreamIdle(_)
        )
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban-agent-core stop_condition_tests`
Expected: PASS.

Also: `cargo build -p caliban-agent-core` to confirm no callers break (existing `match stopped_for` arms have catch-all wildcards).

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/stream/mod.rs
git commit -m "feat(agent-core): add MaxTokensExhausted/Refusal/ContentFilter/StreamIdle StopConditions"
```

---

## Task 2: Add recovery config knobs to `AgentConfig`

**Files:**
- Modify: `crates/caliban-agent-core/src/config.rs`
- Test: `crates/caliban-agent-core/src/config.rs` (inline)

- [ ] **Step 1: Write the failing test**

Append to `crates/caliban-agent-core/src/config.rs`:

```rust
#[cfg(test)]
mod recovery_config_tests {
    use super::*;

    #[test]
    fn default_recovery_knobs() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.escalated_max_tokens, 16_384);
        assert_eq!(cfg.max_meta_continuations, 3);
        assert_eq!(cfg.stream_idle_timeout_ms, 90_000);
        assert!(cfg.max_tokens_recovery);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-agent-core recovery_config_tests`
Expected: FAIL — fields don't exist.

- [ ] **Step 3: Add the fields**

In `AgentConfig` struct and its `Default` impl:

```rust
pub struct AgentConfig {
    // …existing fields…
    /// Stage A escalated max_tokens budget (used once per MaxTokens hit).
    pub escalated_max_tokens: u32,
    /// Stage B meta-continuation cap (per-run).
    pub max_meta_continuations: u8,
    /// Stream idle timeout (ms). 0 disables the watchdog.
    pub stream_idle_timeout_ms: u32,
    /// Master switch for MaxTokens recovery (Stage A + B).
    pub max_tokens_recovery: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            // …existing defaults…
            escalated_max_tokens: 16_384,
            max_meta_continuations: 3,
            stream_idle_timeout_ms: 90_000,
            max_tokens_recovery: true,
        }
    }
}
```

- [ ] **Step 4: Run test**

Run: `cargo test -p caliban-agent-core recovery_config_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/config.rs
git commit -m "feat(agent-core): add recovery knobs to AgentConfig (Stage A/B + stream idle)"
```

---

## Task 3: Carve out a `recovery` submodule with the meta-continuation const

**Files:**
- Create: `crates/caliban-agent-core/src/stream/recovery.rs`
- Modify: `crates/caliban-agent-core/src/stream/mod.rs` (add `mod recovery;`)
- Test: `crates/caliban-agent-core/src/stream/recovery.rs` (inline)

- [ ] **Step 1: Write the file with the const + a passing smoke test**

```rust
//! Recovery flows for the turn loop:
//! - MaxTokens Stage A (budget escalation) + Stage B (meta-continuation).
//! - Reactive compaction on `ContextTooLong`.
//! - Refusal / ContentFilter synthetic-message surfacing.

/// Stage B meta-continuation prompt. Kept terse and model-neutral so 3P
/// providers don't get Anthropic-flavored copy.
pub(crate) const META_CONTINUATION_PROMPT: &str =
    "Output token limit hit. Resume directly \u{2014} no apology, no recap. \
     Pick up mid-thought. Break remaining work into smaller pieces.";

/// Synthetic message text for `stop_reason: Refusal`.
pub(crate) const REFUSAL_SYNTHETIC: &str = "Model declined to respond.";

/// Synthetic message text for `stop_reason: ContentFilter`.
pub(crate) const CONTENT_FILTER_SYNTHETIC: &str = "Response blocked by content filter.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_non_empty() {
        assert!(!META_CONTINUATION_PROMPT.is_empty());
        assert!(!REFUSAL_SYNTHETIC.is_empty());
        assert!(!CONTENT_FILTER_SYNTHETIC.is_empty());
    }
}
```

In `crates/caliban-agent-core/src/stream/mod.rs`, add near the top of the module:

```rust
mod recovery;
```

- [ ] **Step 2: Run test**

Run: `cargo test -p caliban-agent-core recovery`
Expected: PASS (`constants_are_non_empty`).

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-agent-core/src/stream/recovery.rs crates/caliban-agent-core/src/stream/mod.rs
git commit -m "feat(agent-core): add stream::recovery submodule with meta-continuation const"
```

---

## Task 4: MaxTokens Stage A — budget escalation

**Files:**
- Modify: `crates/caliban-agent-core/src/stream/mod.rs` (the post-turn branch around line 903 + the request issuance site)
- Test: `crates/caliban-agent-core/tests/recovery_max_tokens.rs` (new)

- [ ] **Step 1: Write the failing test**

Create `crates/caliban-agent-core/tests/recovery_max_tokens.rs`:

```rust
//! Stage A: one-shot budget escalation when a turn ends in `MaxTokens` with no tool_use.

#![cfg(feature = "mock")]

use caliban_agent_core::{Agent, AgentConfig};
use caliban_provider::{Message, MockProvider, StopReason};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use futures::StreamExt as _;

#[tokio::test]
async fn stage_a_escalates_then_succeeds() {
    let provider = MockProvider::builder()
        // turn 1: stop_reason MaxTokens, output 1024 tokens, no tool_use
        .with_response_max_tokens(1024)
        // turn 2 (after Stage A retry with escalated budget): natural EndTurn
        .with_response_end_turn("All done.")
        .build();

    let cfg = AgentConfig {
        max_tokens: 1024,
        max_tokens_recovery: true,
        escalated_max_tokens: 16_384,
        ..Default::default()
    };
    let agent = Arc::new(Agent::new(Arc::new(provider), cfg).expect("agent"));

    let mut stream = agent.stream_until_done(
        vec![Message::user_text("write a haiku")],
        CancellationToken::new(),
    );

    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let caliban_agent_core::TurnEvent::RunEnd { stopped_for, .. } = ev {
            last_stop = Some(stopped_for);
        }
    }
    assert!(matches!(last_stop, Some(caliban_agent_core::StopCondition::EndOfTurn)));
    // Stage A retried the same request once → 2 provider calls.
    // Implementation detail asserted via the MockProvider call count helper:
    // (left as a TODO_in_mock_provider; replace once the helper exists)
}
```

(Test references `MockProvider::builder().with_response_max_tokens(…)`. If that helper doesn't exist yet, add it in `crates/caliban-provider/src/mock.rs` as part of Step 3 of this task.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-agent-core --features mock --test recovery_max_tokens stage_a_escalates`
Expected: FAIL — either the helper or the recovery branch is missing.

- [ ] **Step 3: Implement Stage A**

In `crates/caliban-agent-core/src/stream/mod.rs`, before the `'outer for turn_index in 0..max_turns` loop, add per-run state:

```rust
let mut stage_a_attempted_this_turn = false;
let mut override_max_tokens_for_request: Option<u32> = None;
```

Then refactor the inner request build to read `override_max_tokens_for_request` instead of `self.config.max_tokens` directly (one substitution at the request-building site).

In the post-turn branch (`stream/mod.rs:903`), replace the bare `if turn_stop_reason != StopReason::ToolUse { … break }` with:

```rust
if turn_stop_reason == StopReason::ToolUse {
    // existing continue-the-loop path
    stage_a_attempted_this_turn = false;       // reset for next turn
    override_max_tokens_for_request = None;
    continue 'outer;
}

if turn_stop_reason == StopReason::MaxTokens
    && self.config.max_tokens_recovery
    && !stage_a_attempted_this_turn
{
    tracing::warn!(
        target: "caliban::recovery",
        from = self.config.max_tokens,
        to = self.config.escalated_max_tokens,
        "recovery.max_tokens.stage_a"
    );
    stage_a_attempted_this_turn = true;
    override_max_tokens_for_request = Some(self.config.escalated_max_tokens);
    // Re-enter the turn body without bumping turn_index.
    // Easiest: pull the turn body into a local async closure invoked via a `'inner` loop, OR
    //   set a `redo_turn = true` flag and use a `loop { … if !redo_turn { break } }` wrap.
    // See implementation note below.
    continue_inner_request_retry = true;
    continue;
}
```

**Implementation note for re-entering the turn body without consuming `turn_index`:** wrap the turn body in `loop { … if !redo_turn { break } redo_turn = false; }` *inside* the `'outer for` iteration. The outer counter advances only when the inner loop breaks.

For the rest: handle the non-tool, non-recoverable `MaxTokens` case (turning into `MaxTokensExhausted` after Stage A surrenders) — defer to Task 5.

- [ ] **Step 4: Run test**

Run: `cargo test -p caliban-agent-core --features mock --test recovery_max_tokens stage_a_escalates`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/stream/mod.rs \
        crates/caliban-agent-core/tests/recovery_max_tokens.rs \
        crates/caliban-provider/src/mock.rs
git commit -m "feat(agent-core): Stage A MaxTokens budget escalation"
```

---

## Task 5: MaxTokens Stage B — meta-continuation + Stage C surrender

**Files:**
- Modify: `crates/caliban-agent-core/src/stream/mod.rs`
- Test: extend `crates/caliban-agent-core/tests/recovery_max_tokens.rs`

- [ ] **Step 1: Add the failing tests**

Append:

```rust
#[tokio::test]
async fn stage_b_injects_meta_then_continues() {
    let provider = MockProvider::builder()
        // turn 1: MaxTokens (Stage A escalates)
        .with_response_max_tokens(1024)
        // turn 1 retry: still MaxTokens → Stage B fires, inject meta msg
        .with_response_max_tokens(16_384)
        // turn 2: EndTurn after the meta msg
        .with_response_end_turn("Resumed.")
        .build();

    let cfg = AgentConfig {
        max_tokens: 1024,
        max_meta_continuations: 3,
        ..Default::default()
    };
    let agent = Arc::new(Agent::new(Arc::new(provider), cfg).unwrap());
    let mut stream = agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut final_history = Vec::new();
    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let caliban_agent_core::TurnEvent::RunEnd { final_messages, stopped_for, .. } = ev {
            final_history = final_messages;
            last_stop = Some(stopped_for);
        }
    }
    assert!(matches!(last_stop, Some(caliban_agent_core::StopCondition::EndOfTurn)));
    let injected = final_history.iter().filter(|m| {
        m.role == caliban_provider::Role::User
            && m.content.iter().any(|b| matches!(b, caliban_provider::ContentBlock::Text(t)
                if t.text.starts_with("Output token limit hit")))
    }).count();
    assert_eq!(injected, 1, "exactly one meta-continuation message injected");
}

#[tokio::test]
async fn stage_c_surrenders_after_cap() {
    let mut builder = MockProvider::builder();
    // Stage A + Stage B × 3 = 4 retries; all return MaxTokens.
    for _ in 0..5 {
        builder = builder.with_response_max_tokens(16_384);
    }
    let provider = builder.build();

    let cfg = AgentConfig {
        max_tokens: 1024,
        max_meta_continuations: 3,
        ..Default::default()
    };
    let agent = Arc::new(Agent::new(Arc::new(provider), cfg).unwrap());
    let mut stream = agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let caliban_agent_core::TurnEvent::RunEnd { stopped_for, .. } = ev {
            last_stop = Some(stopped_for);
        }
    }
    assert!(matches!(last_stop, Some(caliban_agent_core::StopCondition::MaxTokensExhausted)));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-agent-core --features mock --test recovery_max_tokens stage_b`
Expected: FAIL — Stage B not implemented yet.

- [ ] **Step 3: Implement Stage B + Stage C**

In `stream/mod.rs`, extend the MaxTokens branch from Task 4:

```rust
if turn_stop_reason == StopReason::MaxTokens && self.config.max_tokens_recovery {
    if !stage_a_attempted_this_turn {
        // Stage A path (from Task 4) — unchanged.
        stage_a_attempted_this_turn = true;
        override_max_tokens_for_request = Some(self.config.escalated_max_tokens);
        redo_turn = true;
        continue;
    }
    if meta_continuation_count < self.config.max_meta_continuations {
        tracing::warn!(
            target: "caliban::recovery",
            meta_continuation = meta_continuation_count + 1,
            "recovery.max_tokens.stage_b"
        );
        history.push(Message::user_text(crate::stream::recovery::META_CONTINUATION_PROMPT));
        meta_continuation_count += 1;
        stage_a_attempted_this_turn = false;
        override_max_tokens_for_request = None;
        continue 'outer;       // next turn iteration
    }
    // Stage C
    tracing::error!(target: "caliban::recovery", "recovery.max_tokens.stage_c");
    stopped_for = StopCondition::MaxTokensExhausted;
    break 'outer;
}
```

Hoist `meta_continuation_count: u8 = 0` and `redo_turn: bool = false` to the per-run scope.

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-agent-core --features mock --test recovery_max_tokens`
Expected: all three tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/stream/mod.rs crates/caliban-agent-core/tests/recovery_max_tokens.rs
git commit -m "feat(agent-core): MaxTokens Stage B meta-continuation + Stage C surrender"
```

---

## Task 6: Refusal + ContentFilter surfacing

**Files:**
- Modify: `crates/caliban-agent-core/src/stream/mod.rs`
- Test: `crates/caliban-agent-core/tests/recovery_refusal.rs` (new)

- [ ] **Step 1: Write the failing test**

```rust
#![cfg(feature = "mock")]

use caliban_agent_core::{Agent, AgentConfig, StopCondition, TurnEvent};
use caliban_provider::{ContentBlock, Message, MockProvider, Role, StopReason};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use futures::StreamExt as _;

#[tokio::test]
async fn refusal_emits_synthetic_message_and_distinct_stop() {
    let provider = MockProvider::builder()
        .with_response_stop_reason(StopReason::Refusal, "")
        .build();
    let agent = Arc::new(Agent::new(Arc::new(provider), AgentConfig::default()).unwrap());
    let mut stream = agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut final_history = Vec::new();
    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { final_messages, stopped_for, .. } = ev {
            final_history = final_messages;
            last_stop = Some(stopped_for);
        }
    }
    assert!(matches!(last_stop, Some(StopCondition::Refusal(_))));
    let last = final_history.last().expect("at least one message");
    assert_eq!(last.role, Role::Assistant);
    assert!(matches!(&last.content[0], ContentBlock::Text(t) if t.text == "Model declined to respond."));
}

#[tokio::test]
async fn content_filter_emits_synthetic_and_distinct_stop() {
    let provider = MockProvider::builder()
        .with_response_stop_reason(StopReason::ContentFilter, "")
        .build();
    let agent = Arc::new(Agent::new(Arc::new(provider), AgentConfig::default()).unwrap());
    let mut stream = agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());
    let mut last_stop = None;
    let mut final_history = Vec::new();
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { final_messages, stopped_for, .. } = ev {
            final_history = final_messages;
            last_stop = Some(stopped_for);
        }
    }
    assert!(matches!(last_stop, Some(StopCondition::ContentFilter(_))));
    let last = final_history.last().unwrap();
    assert!(matches!(&last.content[0], ContentBlock::Text(t) if t.text == "Response blocked by content filter."));
}
```

(Add `MockProvider::builder().with_response_stop_reason(StopReason, &str)` if it doesn't exist; one-line helper alongside the existing builder.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-agent-core --features mock --test recovery_refusal`
Expected: FAIL.

- [ ] **Step 3: Implement the branches**

In `stream/mod.rs` post-turn, extend the match begun in Tasks 4–5:

```rust
match turn_stop_reason {
    StopReason::ToolUse => { /* … existing continue path */ }
    StopReason::MaxTokens => { /* Task 5 */ }
    StopReason::Refusal => {
        tracing::warn!(target: "caliban::recovery", "recovery.refusal");
        history.push(Message::assistant_text(crate::stream::recovery::REFUSAL_SYNTHETIC));
        stopped_for = StopCondition::Refusal(crate::stream::recovery::REFUSAL_SYNTHETIC.into());
        break 'outer;
    }
    StopReason::ContentFilter => {
        tracing::warn!(target: "caliban::recovery", "recovery.content_filter");
        history.push(Message::assistant_text(crate::stream::recovery::CONTENT_FILTER_SYNTHETIC));
        stopped_for = StopCondition::ContentFilter(crate::stream::recovery::CONTENT_FILTER_SYNTHETIC.into());
        break 'outer;
    }
    _ => {
        stopped_for = StopCondition::EndOfTurn;
        break 'outer;
    }
}
```

(If `Message::assistant_text` doesn't exist yet, add it as a one-liner constructor next to `Message::user_text` in `caliban-provider`.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-agent-core --features mock --test recovery_refusal`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/stream/mod.rs \
        crates/caliban-agent-core/tests/recovery_refusal.rs \
        crates/caliban-provider/src/message.rs
git commit -m "feat(agent-core): surface Refusal/ContentFilter with synthetic message + distinct StopCondition"
```

---

## Task 7: Reactive compaction on `ContextTooLong`

**Files:**
- Modify: `crates/caliban-agent-core/src/stream/mod.rs`
- Modify: `crates/caliban-agent-core/src/stream/recovery.rs` (add helper)
- Test: `crates/caliban-agent-core/tests/recovery_context.rs` (new)

- [ ] **Step 1: Write the failing test**

```rust
#![cfg(feature = "mock")]

use caliban_agent_core::{Agent, AgentConfig, Compactor, StopCondition, TurnEvent};
use caliban_provider::{Capabilities, Error as ProviderError, Message, MockProvider};
use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
use tokio_util::sync::CancellationToken;
use futures::StreamExt as _;
use async_trait::async_trait;

struct RecordingCompactor { calls: Arc<AtomicU32> }

#[async_trait]
impl Compactor for RecordingCompactor {
    async fn compact(
        &self,
        messages: &[Message],
        _caps: &Capabilities,
    ) -> caliban_agent_core::error::Result<Option<Vec<Message>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Drop everything but the last user message to simulate a real reduction.
        let last_user = messages.iter().rev().find(|m| m.role == caliban_provider::Role::User).cloned();
        Ok(last_user.map(|m| vec![m]))
    }
    fn strategy_name(&self) -> &'static str { "test-recording" }
}

#[tokio::test]
async fn reactive_compacts_then_retries_once() {
    let provider = MockProvider::builder()
        .with_error_once(ProviderError::ContextTooLong { max_tokens: 200_000, requested_tokens: 210_000 })
        .with_response_end_turn("ok")
        .build();
    let calls = Arc::new(AtomicU32::new(0));
    let agent = Arc::new(
        Agent::new(Arc::new(provider), AgentConfig::default())
            .unwrap()
            .with_compactor(Arc::new(RecordingCompactor { calls: calls.clone() }))
    );

    let mut stream = agent.stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev { last_stop = Some(stopped_for); }
    }
    assert!(matches!(last_stop, Some(StopCondition::EndOfTurn)));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "compactor fired exactly once");
}

#[tokio::test]
async fn second_context_too_long_surrenders() {
    let provider = MockProvider::builder()
        .with_error_once(ProviderError::ContextTooLong { max_tokens: 200_000, requested_tokens: 210_000 })
        .with_error_once(ProviderError::ContextTooLong { max_tokens: 200_000, requested_tokens: 210_000 })
        .build();
    let agent = Arc::new(
        Agent::new(Arc::new(provider), AgentConfig::default())
            .unwrap()
            .with_compactor(Arc::new(RecordingCompactor { calls: Arc::new(AtomicU32::new(0)) }))
    );

    let mut stream = agent.stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev { last_stop = Some(stopped_for); }
    }
    assert!(matches!(last_stop, Some(StopCondition::ProviderError(_))));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-agent-core --features mock --test recovery_context`
Expected: FAIL.

- [ ] **Step 3: Implement the recovery branch**

At the per-run scope in `stream_until_done_with_settings`:

```rust
let mut attempted_reactive_compact = false;
```

Where the loop currently catches a provider error (search for `StopCondition::ProviderError` in the error-handling arm of the inner request issuance), wrap with:

```rust
match provider_result {
    Err(ProviderError::ContextTooLong { .. }) if !attempted_reactive_compact => {
        tracing::warn!(target: "caliban::recovery", "recovery.reactive_compact.fired");
        attempted_reactive_compact = true;
        let caps = self.provider.capabilities(&self.config.model);
        match self.compactor.compact(&history, &caps).await {
            Ok(Some(new)) => {
                history = new;
                redo_turn = true;
                continue;     // re-enter the turn without bumping turn_index
            }
            Ok(None) | Err(_) => {
                stopped_for = StopCondition::ProviderError("context too long; compactor declined".into());
                break 'outer;
            }
        }
    }
    other => { /* existing handling */ }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-agent-core --features mock --test recovery_context`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/stream/mod.rs crates/caliban-agent-core/tests/recovery_context.rs
git commit -m "feat(agent-core): reactive compaction on ContextTooLong"
```

---

## Task 8: `WatchedStream` wrapper

**Files:**
- Modify: `crates/caliban-provider/src/stream.rs`
- Test: `crates/caliban-provider/src/stream.rs` (inline)

- [ ] **Step 1: Write the failing test**

Append to `crates/caliban-provider/src/stream.rs`:

```rust
#[cfg(test)]
mod watched_tests {
    use super::*;
    use futures::stream::{self, StreamExt as _};
    use std::time::Duration;

    #[tokio::test]
    async fn passes_through_normal_data() {
        let inner = stream::iter(vec![Ok(StreamEvent::Done), Ok(StreamEvent::Done)]);
        let mut w = WatchedStream::new(inner, Duration::from_secs(1));
        let mut seen = 0;
        while let Some(item) = w.next().await { item.unwrap(); seen += 1; }
        assert_eq!(seen, 2);
    }

    #[tokio::test]
    async fn aborts_after_idle_timeout() {
        let inner = stream::pending::<Result<StreamEvent>>();
        let mut w = WatchedStream::new(inner, Duration::from_millis(20));
        let r = w.next().await.expect("Some(_)");
        assert!(matches!(r, Err(Error::StreamIdle(_))));
    }
}
```

Add a new `Error::StreamIdle(Duration)` variant to `crates/caliban-provider/src/error.rs`:

```rust
/// The streaming response went silent past the idle timeout.
#[error("stream idle for {0:?}")]
StreamIdle(std::time::Duration),
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-provider watched_tests`
Expected: FAIL — `WatchedStream` doesn't exist.

- [ ] **Step 3: Implement `WatchedStream`**

In `crates/caliban-provider/src/stream.rs`:

```rust
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

pin_project_lite::pin_project! {
    /// Wraps a `MessageStream` and aborts if no chunk arrives within `idle`.
    /// Emits a `tracing::warn` at half-time and `Err(Error::StreamIdle)` on full timeout.
    pub struct WatchedStream<S> {
        #[pin]
        inner: S,
        idle: Duration,
        last_chunk_at: Instant,
        warned: bool,
    }
}

impl<S> WatchedStream<S> {
    pub fn new(inner: S, idle: Duration) -> Self {
        Self { inner, idle, last_chunk_at: Instant::now(), warned: false }
    }
}

impl<S> Stream for WatchedStream<S>
where
    S: Stream<Item = Result<StreamEvent>>,
{
    type Item = Result<StreamEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(item)) => {
                *this.last_chunk_at = Instant::now();
                *this.warned = false;
                Poll::Ready(Some(item))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => {
                let elapsed = this.last_chunk_at.elapsed();
                if elapsed >= *this.idle {
                    tracing::error!(target: "caliban::stream", elapsed_ms = elapsed.as_millis() as u64, "recovery.stream_idle.abort");
                    return Poll::Ready(Some(Err(Error::StreamIdle(elapsed))));
                }
                if !*this.warned && elapsed >= *this.idle / 2 {
                    *this.warned = true;
                    tracing::warn!(target: "caliban::stream", elapsed_ms = elapsed.as_millis() as u64, "recovery.stream_idle.warning");
                }
                // Schedule a wakeup at the remaining time so we can fire abort even if `inner` stays Pending.
                let remaining = *this.idle - elapsed;
                let waker = cx.waker().clone();
                tokio::spawn(async move {
                    tokio::time::sleep(remaining + Duration::from_millis(1)).await;
                    waker.wake();
                });
                Poll::Pending
            }
        }
    }
}
```

Add `pin-project-lite = "0.2"` to `crates/caliban-provider/Cargo.toml` if not already present.

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-provider watched_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-provider/src/stream.rs crates/caliban-provider/src/error.rs crates/caliban-provider/Cargo.toml
git commit -m "feat(provider): add WatchedStream<S> stream-idle watchdog with half-time warning"
```

---

## Task 9: Wire `WatchedStream` into each provider's stream_parse

**Files:**
- Modify: `crates/caliban-provider-anthropic/src/stream_parse.rs`
- Modify: `crates/caliban-provider-openai/src/stream_parse.rs`
- Modify: `crates/caliban-provider-google/src/stream_parse.rs`
- Modify: `crates/caliban-provider-ollama/src/stream_parse.rs`
- Test: `crates/caliban-agent-core/tests/recovery_stream_idle.rs` (new, end-to-end)

- [ ] **Step 1: Write the end-to-end failing test**

```rust
#![cfg(feature = "mock")]

use caliban_agent_core::{Agent, AgentConfig, StopCondition, TurnEvent};
use caliban_provider::{Message, MockProvider};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use futures::StreamExt as _;

#[tokio::test]
async fn stream_idle_aborts_run() {
    let provider = MockProvider::builder().with_silent_stream(Duration::from_secs(10)).build();
    let cfg = AgentConfig { stream_idle_timeout_ms: 200, ..Default::default() };
    let agent = Arc::new(Agent::new(Arc::new(provider), cfg).unwrap());
    let mut stream = agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev { last_stop = Some(stopped_for); }
    }
    assert!(matches!(last_stop, Some(StopCondition::StreamIdle(_))));
}
```

Add `MockProvider::builder().with_silent_stream(Duration)` — a one-line method that returns a `Stream` that stays `Pending` for `duration` then yields `Done`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-agent-core --features mock --test recovery_stream_idle`
Expected: FAIL (`MockProvider` doesn't wrap, `Error::StreamIdle` not mapped to `StopCondition::StreamIdle`).

- [ ] **Step 3: Map `Error::StreamIdle` → `StopCondition::StreamIdle` in the agent loop**

In `stream/mod.rs`, where provider errors are converted to `StopCondition`:

```rust
match e {
    ProviderError::ContextTooLong { .. } => { /* Task 7 */ }
    ProviderError::StreamIdle(d) => {
        stopped_for = StopCondition::StreamIdle(d);
        break 'outer;
    }
    other => {
        stopped_for = StopCondition::ProviderError(other.to_string());
        break 'outer;
    }
}
```

- [ ] **Step 4: Wrap each provider's SSE consumer**

In each `stream_parse.rs`, replace the bare `while let Some(item) = sse.next().await` with:

```rust
let idle_ms = std::env::var("CALIBAN_STREAM_IDLE_TIMEOUT_MS").ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(90_000);
let mut watched = caliban_provider::stream::WatchedStream::new(sse, std::time::Duration::from_millis(idle_ms));
while let Some(item) = watched.next().await {
    let item = item?;     // propagates Err(Error::StreamIdle(_))
    // …existing per-event handling…
}
```

Repeat in all four provider crates. Each touch is the same 4-5 line change.

- [ ] **Step 5: Run test**

Run: `cargo test -p caliban-agent-core --features mock --test recovery_stream_idle`
Expected: PASS.

Also: `cargo test --workspace` to confirm no regression in provider crates' own tests.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-provider-*/src/stream_parse.rs crates/caliban-agent-core/src/stream/mod.rs \
        crates/caliban-agent-core/tests/recovery_stream_idle.rs crates/caliban-provider/src/mock.rs
git commit -m "feat(providers): wrap SSE consumers in WatchedStream; map StreamIdle to StopCondition"
```

---

## Task 10: Failure-aware hook dispatch (death-spiral guard)

**Files:**
- Modify: `crates/caliban-agent-core/src/hooks.rs`
- Modify: `crates/caliban-agent-core/src/stream/mod.rs`
- Test: `crates/caliban-agent-core/tests/hook_failure.rs` (new)

- [ ] **Step 1: Write the failing test**

```rust
#![cfg(feature = "mock")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use futures::StreamExt as _;

use caliban_agent_core::{
    Agent, AgentConfig, Hooks, RunCtx, RunHookOutcome, StopCondition, TurnEvent,
};
use caliban_provider::{Error as ProviderError, Message, MockProvider};

struct CountingHooks {
    after_run_called: Arc<AtomicU32>,
    after_run_failure_called: Arc<AtomicU32>,
}

#[async_trait]
impl Hooks for CountingHooks {
    async fn after_run(&self, _: &RunCtx<'_>, _: &RunHookOutcome) -> anyhow::Result<()> {
        self.after_run_called.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn after_run_failure(&self, _: &RunCtx<'_>, _: &RunHookOutcome) -> anyhow::Result<()> {
        self.after_run_failure_called.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn provider_error_runs_after_run_failure_not_after_run() {
    let provider = MockProvider::builder()
        .with_error_once(ProviderError::Auth("nope".into()))
        .build();
    let hooks = Arc::new(CountingHooks {
        after_run_called: Arc::new(AtomicU32::new(0)),
        after_run_failure_called: Arc::new(AtomicU32::new(0)),
    });
    let agent = Arc::new(Agent::new(Arc::new(provider), AgentConfig::default()).unwrap()
        .with_hooks(hooks.clone()));
    let mut stream = agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());
    while stream.next().await.is_some() {}
    assert_eq!(hooks.after_run_called.load(Ordering::SeqCst), 0);
    assert_eq!(hooks.after_run_failure_called.load(Ordering::SeqCst), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-agent-core --features mock --test hook_failure`
Expected: FAIL — `after_run_failure` doesn't exist on `Hooks`.

- [ ] **Step 3: Add `after_run_failure` / `after_turn_failure` to the trait**

In `crates/caliban-agent-core/src/hooks.rs`, after `after_run`:

```rust
/// Called instead of [`Hooks::after_run`] when the run ended in failure.
/// Default is a no-op; observability for failure modes; should NOT mutate
/// session state (avoid death-spirals).
async fn after_run_failure(
    &self,
    _ctx: &RunCtx<'_>,
    _outcome: &RunHookOutcome,
) -> Result<()> { Ok(()) }

/// Called instead of [`Hooks::after_turn`] when the turn ended in failure.
async fn after_turn_failure(
    &self,
    _ctx: &TurnCtx<'_>,
    _outcome: &crate::TurnOutcome,
) -> Result<()> { Ok(()) }
```

- [ ] **Step 4: Branch the dispatch site**

In `stream/mod.rs:911-933`, replace the unconditional `after_run` with:

```rust
let outcome = RunHookOutcome {
    turn_count: turns_completed,
    input_tokens: total_usage.input_tokens,
    output_tokens: total_usage.output_tokens,
    success: !stopped_for.is_failure(),
};
let run_ctx = RunCtx {
    session_id: &settings.session_id,
    workspace_root: &settings.workspace_root,
    user_message: user_msg_owned.as_ref(),
    prompt_index: settings.prompt_index,
    cancel: cancel.clone(),
};
let dispatch = if stopped_for.is_failure() {
    self.hooks.after_run_failure(&run_ctx, &outcome).await
} else {
    self.hooks.after_run(&run_ctx, &outcome).await
};
if let Err(e) = dispatch {
    tracing::warn!(error = %e, "after_run* hook error (non-fatal)");
}
```

Apply the same pattern to `after_turn` (look for its call site; today it's invoked inside the turn body before the continue/halt decision — keep the position, branch on `TurnOutcome::is_failure()` if such helper exists, otherwise pass the same `stopped_for.is_failure()` check via a closure).

- [ ] **Step 5: Run test**

Run: `cargo test -p caliban-agent-core --features mock --test hook_failure`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-agent-core/src/hooks.rs crates/caliban-agent-core/src/stream/mod.rs \
        crates/caliban-agent-core/tests/hook_failure.rs
git commit -m "feat(agent-core): split after_run/after_turn into success vs failure paths"
```

---

## Task 11: `TurnDecision` return on `after_turn`

**Files:**
- Modify: `crates/caliban-agent-core/src/hooks.rs`
- Modify: `crates/caliban-agent-core/src/stream/mod.rs`
- Test: `crates/caliban-agent-core/tests/hook_turn_decision.rs` (new)

- [ ] **Step 1: Write the failing test**

```rust
#![cfg(feature = "mock")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use futures::StreamExt as _;

use caliban_agent_core::{
    Agent, AgentConfig, Hooks, TurnCtx, TurnDecision, TurnOutcome, TurnEvent,
};
use caliban_provider::{Message, MockProvider};

struct ForceContinueHooks { count: Arc<AtomicU32> }

#[async_trait]
impl Hooks for ForceContinueHooks {
    async fn after_turn(
        &self,
        _ctx: &TurnCtx<'_>,
        _outcome: &TurnOutcome,
    ) -> anyhow::Result<TurnDecision> {
        let n = self.count.fetch_add(1, Ordering::SeqCst);
        if n < 5 {
            Ok(TurnDecision::ContinueWith(vec![Message::user_text("keep going")]))
        } else {
            Ok(TurnDecision::Continue)
        }
    }
}

#[tokio::test]
async fn continue_with_capped_at_three() {
    let mut builder = MockProvider::builder();
    for _ in 0..10 { builder = builder.with_response_end_turn("done"); }
    let provider = builder.build();
    let hooks = Arc::new(ForceContinueHooks { count: Arc::new(AtomicU32::new(0)) });
    let agent = Arc::new(Agent::new(Arc::new(provider), AgentConfig::default()).unwrap()
        .with_hooks(hooks.clone()));
    let mut stream = agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());
    let mut final_history = Vec::new();
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { final_messages, .. } = ev { final_history = final_messages; }
    }
    let injected = final_history.iter()
        .filter(|m| m.role == caliban_provider::Role::User
                 && m.content.iter().any(|b| matches!(b, caliban_provider::ContentBlock::Text(t) if t.text == "keep going")))
        .count();
    assert_eq!(injected, 3, "ContinueWith capped at MAX_FORCED_CONTINUATIONS=3");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-agent-core --features mock --test hook_turn_decision`
Expected: FAIL — `TurnDecision` doesn't exist.

- [ ] **Step 3: Implement `TurnDecision` + cap**

In `crates/caliban-agent-core/src/hooks.rs`:

```rust
/// Outcome of `after_turn`. Default impls return `Continue`.
#[derive(Debug, Clone)]
pub enum TurnDecision {
    Continue,
    ContinueWith(Vec<caliban_provider::Message>),
    Stop,
}

// Change the default impl of after_turn:
async fn after_turn(
    &self,
    _ctx: &TurnCtx<'_>,
    _outcome: &crate::TurnOutcome,
) -> Result<TurnDecision> { Ok(TurnDecision::Continue) }
```

Re-export `TurnDecision` from `caliban-agent-core::lib.rs`.

In `stream/mod.rs`, near the existing `after_turn` call site, add the per-run counter and decision handling:

```rust
const MAX_FORCED_CONTINUATIONS: u8 = 3;
let mut forced_continuations: u8 = 0;

// …after the continue/halt decision is computed but before the actual break:
let decision = match self.hooks.after_turn(&turn_ctx, &turn_outcome).await {
    Ok(d) => d,
    Err(e) => {
        tracing::warn!(error = %e, "after_turn hook error (non-fatal)");
        TurnDecision::Continue
    }
};

match decision {
    TurnDecision::Continue => { /* fall through to existing logic */ }
    TurnDecision::ContinueWith(msgs) if forced_continuations < MAX_FORCED_CONTINUATIONS => {
        history.extend(msgs);
        forced_continuations += 1;
        continue 'outer;
    }
    TurnDecision::ContinueWith(_) => {
        tracing::warn!(forced_continuations, "after_turn ContinueWith ignored (cap reached)");
    }
    TurnDecision::Stop => {
        stopped_for = StopCondition::HookDenied("after_turn: Stop".into());
        break 'outer;
    }
}
```

Update other `Hooks` impls in the workspace (`caliban-checkpoint`'s `CheckpointHook`, `caliban` bin's `CompositeHooks` if it defines `after_turn`) to match the new return type — typically a one-line `Ok(TurnDecision::Continue)` swap.

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-agent-core --features mock --test hook_turn_decision`
Expected: PASS.

Run: `cargo test --workspace` to confirm the trait change didn't break other crates.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/hooks.rs crates/caliban-agent-core/src/stream/mod.rs \
        crates/caliban-agent-core/src/lib.rs crates/caliban-agent-core/tests/hook_turn_decision.rs \
        crates/caliban-checkpoint/src/*.rs caliban/src/**/*.rs
git commit -m "feat(agent-core): after_turn returns TurnDecision with ContinueWith(cap=3)"
```

---

## Task 12: TUI stalled-tokens signal

**Files:**
- Modify: `caliban/src/tui/app.rs` (add `last_delta_at`)
- Modify: `caliban/src/tui/events.rs` (update on deltas / tool starts; new StopCondition match arms)
- Modify: `caliban/src/tui/render.rs` (stalled-style branch)
- Test: `caliban/src/tui/render.rs` (inline snapshot test)

- [ ] **Step 1: Write the failing test**

In `caliban/src/tui/render.rs` add an inline test (or extend the existing module):

```rust
#[cfg(test)]
mod stalled_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn render_spinner_stalled_when_idle_over_3s_no_tools() {
        let now = Instant::now();
        let last_delta = now - Duration::from_secs(12);
        let label = format_spinner_cell(/*active_tools=*/ false, last_delta, now);
        assert!(label.contains("no tokens for 12s"));
    }

    #[test]
    fn render_spinner_normal_under_3s() {
        let now = Instant::now();
        let last_delta = now - Duration::from_secs(1);
        let label = format_spinner_cell(false, last_delta, now);
        assert!(!label.contains("no tokens"));
    }

    #[test]
    fn render_spinner_normal_when_tools_active() {
        let now = Instant::now();
        let last_delta = now - Duration::from_secs(30);
        let label = format_spinner_cell(/*active_tools=*/ true, last_delta, now);
        assert!(!label.contains("no tokens"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban stalled_tests`
Expected: FAIL — `format_spinner_cell` doesn't exist.

- [ ] **Step 3: Add `last_delta_at` to `App` and wire events**

In `caliban/src/tui/app.rs`:

```rust
pub(crate) last_delta_at: std::time::Instant,
```

Initialize to `Instant::now()` in the constructor.

In `caliban/src/tui/events.rs`, in the handlers for `AssistantTextDelta`, `ToolCallInputDelta`, and `ToolUseStart`:

```rust
app.last_delta_at = std::time::Instant::now();
```

Add match arms in `events.rs` wherever a `StopCondition` is pattern-matched (look for `StopCondition::ProviderError` and add adjacent arms for the new variants):

```rust
StopCondition::MaxTokensExhausted => format_status("max tokens exhausted — try `/effort low` next time"),
StopCondition::Refusal(msg) | StopCondition::ContentFilter(msg) => format_status(msg),
StopCondition::StreamIdle(d) => format_status(&format!("stream idle for {}s", d.as_secs())),
```

In `caliban/src/tui/render.rs`, extract a `format_spinner_cell` helper:

```rust
pub(crate) fn format_spinner_cell(active_tools: bool, last_delta_at: std::time::Instant, now: std::time::Instant) -> String {
    let elapsed = now.duration_since(last_delta_at);
    if !active_tools && elapsed >= std::time::Duration::from_secs(3) {
        let secs = elapsed.as_secs();
        if secs >= 10 {
            return format!("Thinking… (no tokens for {secs}s)");
        }
        return "Thinking…".to_string();
    }
    "Thinking…".to_string()
}
```

And in the actual render path:

```rust
let label = format_spinner_cell(app.has_active_tools, app.last_delta_at, Instant::now());
let style = if !app.has_active_tools && app.last_delta_at.elapsed() >= Duration::from_secs(3) {
    Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM)
} else {
    Style::default()
};
```

(If `app.has_active_tools` doesn't already exist, derive it from the in-flight tool dispatch state already tracked on `App`.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban stalled_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/app.rs caliban/src/tui/events.rs caliban/src/tui/render.rs
git commit -m "feat(tui): stalled-tokens spinner hint after 3s of stream silence"
```

---

## Task 13: Workspace sanity + commit final telemetry counters

**Files:**
- Modify: `crates/caliban-telemetry/src/metrics.rs` (add the new counter names)
- Run: full workspace test pass

- [ ] **Step 1: Add the new metric counters**

In `crates/caliban-telemetry/src/metrics.rs`, alongside existing metric definitions:

```rust
pub const RECOVERY_MAX_TOKENS_RECOVERED: &str = "caliban.recovery.max_tokens_recovered";
pub const RECOVERY_STREAM_IDLE_ABORTED:  &str = "caliban.recovery.stream_idle_aborted";
pub const RECOVERY_REACTIVE_COMPACTED:   &str = "caliban.recovery.reactive_compacted";
pub const RECOVERY_REFUSALS_SURFACED:    &str = "caliban.recovery.refusals_surfaced";
```

Then increment them at the corresponding recovery sites added in Tasks 5, 6, 7, 9. (Each call is one line of `MetricEmitter::emit_counter(NAME, attrs)`.)

- [ ] **Step 2: Run the full workspace test pass**

Run: `cargo test --workspace --all-features`
Expected: all tests pass.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings (or only pre-existing ones unrelated to this PR).

- [ ] **Step 3: Update the parity-gap matrix**

In `docs/parity-gap-matrix.md`, the implicit "turn loop resilience" gaps aren't currently tracked as a section. Add a brief mention under section K (Observability) or a new mini-section, ticking the items as ✅ for this PR.

- [ ] **Step 4: Commit**

```bash
git add crates/caliban-telemetry/src/metrics.rs docs/parity-gap-matrix.md
git commit -m "chore(telemetry): metric counters for recovery flows; update parity matrix"
```

---

## Self-Review Notes

- **Spec coverage:** Task 1 (StopCondition), Task 2 (config knobs), Task 3 (recovery module), Task 4 (Stage A), Task 5 (Stage B/C), Task 6 (Refusal/ContentFilter), Task 7 (reactive compact), Task 8 (`WatchedStream`), Task 9 (provider wiring), Task 10 (failure hooks), Task 11 (`TurnDecision`), Task 12 (TUI stall), Task 13 (telemetry + matrix). Every spec requirement traces to a task.
- **No backwards-incompatible surprises:** `Hooks` default impls preserve all existing behavior. Provider `Error` enum is additive. `StopCondition` is additive — existing `match` arms have catch-all wildcards in this workspace.
- **Cap values are constants in code**, not magic numbers in the plan: `MAX_FORCED_CONTINUATIONS=3`, `max_meta_continuations=3`, `escalated_max_tokens=16_384`, `stream_idle_timeout_ms=90_000`, `min_cache_block_tokens=1024` (Spec B).
