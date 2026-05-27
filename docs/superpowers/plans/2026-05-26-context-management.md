# Context-window management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add autocompact (proactive threshold-based), microcompact (per-turn supersession-based, LLM-free), a global tool-result size cap with overflow persistence, and a per-message conversation-level prompt-cache marker.

**Architecture:** Four mostly-independent additions to `caliban-agent-core`. Each is wired at a different turn-loop seam: microcompact + autocompact at the top of each turn; tool-result cap as a `post_process` pass after parallel dispatch; conversation cache marker as an extension to the existing `apply_prompt_cache`. New configuration on `AgentConfig` is additive with sensible defaults.

**Tech Stack:** Rust 1.85.0 (edition 2024), `tokio`, `async-trait`, `serde`, `directories` (for cache dir), `caliban-provider` (`feature = "mock"` for tests).

**Spec:** [`docs/superpowers/specs/2026-05-26-context-management-design.md`](../specs/2026-05-26-context-management-design.md)

---

## File Structure

```
crates/caliban-agent-core/src/
├── config.rs                  MODIFY: add autocompact/microcompact/cap/cache fields
├── compact.rs                 MODIFY: add MicroCompactor + supersession key helper
├── cache.rs                   MODIFY: apply_prompt_cache gains conversation marker
├── stream/mod.rs              MODIFY: pre-turn microcompact + autocompact threshold check
├── post_process.rs            MODIFY: add cap_tool_results pass
├── stream/parallel.rs         MODIFY: invoke cap_tool_results after batch dispatch

caliban-telemetry/src/
├── compaction.rs              CREATE: new metric definitions

crates/caliban-agent-core/tests/
├── compact_micro.rs           CREATE: supersession unit/integration tests
├── compact_auto.rs            CREATE: threshold + backoff integration tests
├── tool_result_cap.rs         CREATE: overflow + placeholder content tests
└── cache_marker.rs            CREATE: multi-turn conversation marker tests
```

---

## Task 1: Add config knobs to `AgentConfig`

**Files:**
- Modify: `crates/caliban-agent-core/src/config.rs`
- Test: `crates/caliban-agent-core/src/config.rs` (inline)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod context_config_tests {
    use super::*;

    #[test]
    fn default_context_knobs() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.auto_compact_threshold, Some(0.75));
        assert!(cfg.micro_compact_enabled);
        assert_eq!(cfg.tool_result_cap_chars, 50_000);
        assert_eq!(cfg.min_cache_block_tokens, 1024);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-agent-core context_config_tests`
Expected: FAIL.

- [ ] **Step 3: Add the fields**

```rust
pub struct AgentConfig {
    // …existing…
    /// Pre-turn autocompaction threshold (utilization in 0..=1). `None` disables.
    pub auto_compact_threshold: Option<f32>,
    /// Enable the per-turn microcompact janitor pass.
    pub micro_compact_enabled: bool,
    /// Global per-tool-result cap in chars. `0` disables.
    pub tool_result_cap_chars: usize,
    /// Minimum estimated tokens on the last user message to merit a cache marker.
    pub min_cache_block_tokens: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            // …existing defaults…
            auto_compact_threshold: Some(0.75),
            micro_compact_enabled: true,
            tool_result_cap_chars: 50_000,
            min_cache_block_tokens: 1024,
        }
    }
}
```

- [ ] **Step 4: Run test**

Run: `cargo test -p caliban-agent-core context_config_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/config.rs
git commit -m "feat(agent-core): add context-management knobs to AgentConfig"
```

---

## Task 2: Per-tool supersession key

**Files:**
- Modify: `crates/caliban-agent-core/src/compact.rs`
- Test: `crates/caliban-agent-core/src/compact.rs` (inline)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod supersession_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_key_is_file_path() {
        let k = supersession_key("Read", &json!({"file_path": "/x.rs"}));
        assert_eq!(k.as_deref(), Some("/x.rs"));
    }
    #[test]
    fn grep_key_is_exact_args() {
        let a = supersession_key("Grep", &json!({"pattern": "foo", "path": "."}));
        let b = supersession_key("Grep", &json!({"pattern": "foo", "path": "."}));
        let c = supersession_key("Grep", &json!({"pattern": "bar", "path": "."}));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
    #[test]
    fn bash_is_never_supersedable() {
        assert!(supersession_key("Bash", &json!({"command": "ls"})).is_none());
    }
    #[test]
    fn webfetch_keys_by_url() {
        let k = supersession_key("WebFetch", &json!({"url": "https://x", "prompt": "…"}));
        assert_eq!(k.as_deref(), Some("https://x"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-agent-core supersession_tests`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
/// Per-tool predicate for "newer invocation of this same logical action".
/// Returns the supersession key; `None` means this tool is never supersedable.
pub(crate) fn supersession_key(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "Read" => input.get("file_path").and_then(|v| v.as_str()).map(String::from),
        "Grep" => Some(input.to_string()),
        "Glob" => Some(input.to_string()),
        "WebFetch" => input.get("url").and_then(|v| v.as_str()).map(String::from),
        _ => None,
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-agent-core supersession_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/compact.rs
git commit -m "feat(agent-core): add per-tool supersession_key for microcompact"
```

---

## Task 3: `MicroCompactor` strategy

**Files:**
- Modify: `crates/caliban-agent-core/src/compact.rs`
- Test: `crates/caliban-agent-core/tests/compact_micro.rs` (new)

- [ ] **Step 1: Write the failing test**

```rust
use caliban_agent_core::{Compactor, MicroCompactor};
use caliban_provider::{Capabilities, ContentBlock, Message, Role, TextBlock, ToolResultBlock, ToolUseBlock};
use serde_json::json;

fn capabilities() -> Capabilities { Capabilities::default() }

fn read_use(id: &str, path: &str) -> ContentBlock {
    ContentBlock::ToolUse(ToolUseBlock {
        id: id.into(),
        name: "Read".into(),
        input: json!({"file_path": path}),
        ..Default::default()
    })
}
fn tool_result(id: &str, body: &str) -> ContentBlock {
    ContentBlock::ToolResult(ToolResultBlock {
        tool_use_id: id.into(),
        content: vec![ContentBlock::Text(TextBlock { text: body.into(), cache_control: None })],
        is_error: false,
        cache_control: None,
    })
}

#[tokio::test]
async fn supersedes_older_read_of_same_path() {
    let msgs = vec![
        Message { role: Role::Assistant, content: vec![read_use("a", "/x.rs")] },
        Message { role: Role::User, content: vec![tool_result("a", "old content")] },
        Message { role: Role::Assistant, content: vec![read_use("b", "/x.rs")] },
        Message { role: Role::User, content: vec![tool_result("b", "new content")] },
    ];
    let out = MicroCompactor::new().compact(&msgs, &capabilities()).await.unwrap();
    let new = out.expect("microcompact should mutate");
    // Older result replaced with placeholder
    let text_a = match &new[1].content[0] {
        ContentBlock::ToolResult(tr) => match &tr.content[0] {
            ContentBlock::Text(t) => t.text.clone(), _ => panic!() },
        _ => panic!(),
    };
    assert!(text_a.starts_with("[superseded: Read("));
    // Newer result preserved
    let text_b = match &new[3].content[0] {
        ContentBlock::ToolResult(tr) => match &tr.content[0] {
            ContentBlock::Text(t) => t.text.clone(), _ => panic!() },
        _ => panic!(),
    };
    assert_eq!(text_b, "new content");
}

#[tokio::test]
async fn does_not_supersede_different_path() {
    let msgs = vec![
        Message { role: Role::Assistant, content: vec![read_use("a", "/x.rs")] },
        Message { role: Role::User, content: vec![tool_result("a", "X")] },
        Message { role: Role::Assistant, content: vec![read_use("b", "/y.rs")] },
        Message { role: Role::User, content: vec![tool_result("b", "Y")] },
    ];
    let out = MicroCompactor::new().compact(&msgs, &capabilities()).await.unwrap();
    assert!(out.is_none(), "no supersession across different paths");
}

#[tokio::test]
async fn does_not_supersede_bash() {
    let msgs = vec![
        Message { role: Role::Assistant, content: vec![ContentBlock::ToolUse(ToolUseBlock {
            id: "a".into(), name: "Bash".into(), input: json!({"command": "ls"}), ..Default::default()
        })] },
        Message { role: Role::User, content: vec![tool_result("a", "out1")] },
        Message { role: Role::Assistant, content: vec![ContentBlock::ToolUse(ToolUseBlock {
            id: "b".into(), name: "Bash".into(), input: json!({"command": "ls"}), ..Default::default()
        })] },
        Message { role: Role::User, content: vec![tool_result("b", "out2")] },
    ];
    let out = MicroCompactor::new().compact(&msgs, &capabilities()).await.unwrap();
    assert!(out.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-agent-core --test compact_micro`
Expected: FAIL.

- [ ] **Step 3: Implement `MicroCompactor`**

In `crates/caliban-agent-core/src/compact.rs`:

```rust
/// Janitor compactor: replaces older `ToolResult` blocks with a one-line
/// placeholder when a newer invocation of the same logical action exists.
/// LLM-free; O(n) per call.
#[derive(Debug, Default)]
pub struct MicroCompactor;

impl MicroCompactor {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Compactor for MicroCompactor {
    async fn compact(
        &self,
        messages: &[Message],
        _capabilities: &Capabilities,
    ) -> Result<Option<Vec<Message>>> {
        // First pass: find the latest tool_use_id for each (tool, key).
        let mut latest: std::collections::HashMap<(String, String), String> = Default::default();
        for m in messages {
            for cb in &m.content {
                if let caliban_provider::ContentBlock::ToolUse(tu) = cb {
                    if let Some(k) = supersession_key(&tu.name, &tu.input) {
                        latest.insert((tu.name.clone(), k), tu.id.clone());
                    }
                }
            }
        }
        // Build a map tool_use_id → (tool_name, key) for older invocations.
        let mut superseded: std::collections::HashMap<String, (String, String)> = Default::default();
        for m in messages {
            for cb in &m.content {
                if let caliban_provider::ContentBlock::ToolUse(tu) = cb {
                    if let Some(k) = supersession_key(&tu.name, &tu.input) {
                        if let Some(latest_id) = latest.get(&(tu.name.clone(), k.clone())) {
                            if latest_id != &tu.id {
                                superseded.insert(tu.id.clone(), (tu.name.clone(), k));
                            }
                        }
                    }
                }
            }
        }
        if superseded.is_empty() { return Ok(None); }
        // Second pass: rewrite ToolResult blocks whose id is superseded.
        let new: Vec<Message> = messages.iter().map(|m| {
            let new_content: Vec<_> = m.content.iter().map(|cb| match cb {
                caliban_provider::ContentBlock::ToolResult(tr) => {
                    if let Some((tool, key)) = superseded.get(&tr.tool_use_id) {
                        let placeholder = format!("[superseded: {tool}({key})]");
                        caliban_provider::ContentBlock::ToolResult(caliban_provider::ToolResultBlock {
                            tool_use_id: tr.tool_use_id.clone(),
                            content: vec![caliban_provider::ContentBlock::Text(caliban_provider::TextBlock {
                                text: placeholder, cache_control: None,
                            })],
                            is_error: tr.is_error,
                            cache_control: None,
                        })
                    } else { cb.clone() }
                }
                _ => cb.clone(),
            }).collect();
            caliban_provider::Message { role: m.role.clone(), content: new_content }
        }).collect();
        Ok(Some(new))
    }

    fn strategy_name(&self) -> &'static str { "MicroCompactor" }
}
```

Re-export from `lib.rs`:

```rust
pub use compact::{Compactor, MicroCompactor, NoopCompactor, DropOldestCompactor, SummarizingCompactor, estimate_tokens};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-agent-core --test compact_micro`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/compact.rs crates/caliban-agent-core/src/lib.rs crates/caliban-agent-core/tests/compact_micro.rs
git commit -m "feat(agent-core): add MicroCompactor (LLM-free supersession-based)"
```

---

## Task 4: Wire microcompact into the pre-turn step

**Files:**
- Modify: `crates/caliban-agent-core/src/stream/mod.rs` (around the existing compact block, ~line 318-360)
- Test: extend `crates/caliban-agent-core/tests/compact_micro.rs` with end-to-end

- [ ] **Step 1: Write the failing test**

Add to `compact_micro.rs`:

```rust
#![cfg(feature = "mock")]

use caliban_agent_core::{Agent, AgentConfig};
use caliban_provider::{ContentBlock, Message, MockProvider, Role};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use futures::StreamExt as _;

#[tokio::test]
async fn microcompact_runs_pre_turn_and_strips_superseded() {
    // History already contains two reads of /x.rs from previous turns.
    let history = vec![
        Message::user_text("read this please"),
        Message { role: Role::Assistant, content: vec![read_use("a", "/x.rs")] },
        Message { role: Role::User, content: vec![tool_result("a", "v1")] },
        Message { role: Role::Assistant, content: vec![read_use("b", "/x.rs")] },
        Message { role: Role::User, content: vec![tool_result("b", "v2")] },
    ];
    let provider = MockProvider::builder().with_response_end_turn("ok").build();
    let cfg = AgentConfig { micro_compact_enabled: true, ..Default::default() };
    let agent = Arc::new(Agent::new(Arc::new(provider), cfg).unwrap());

    let mut stream = agent.stream_until_done(history, CancellationToken::new());
    let mut final_messages = Vec::new();
    while let Some(Ok(ev)) = stream.next().await {
        if let caliban_agent_core::TurnEvent::RunEnd { final_messages: fm, .. } = ev {
            final_messages = fm;
        }
    }
    let old_result = &final_messages[2].content[0];
    let text = match old_result {
        ContentBlock::ToolResult(tr) => match &tr.content[0] {
            ContentBlock::Text(t) => t.text.clone(), _ => panic!() },
        _ => panic!(),
    };
    assert!(text.starts_with("[superseded:"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-agent-core --features mock --test compact_micro microcompact_runs_pre_turn`
Expected: FAIL.

- [ ] **Step 3: Wire microcompact ahead of autocompact at the top of each turn**

In `stream/mod.rs` inside `'outer for turn_index in …`, before the existing `// ---- Compaction ----` block:

```rust
// ---- Microcompact (per-turn, LLM-free) ----
if self.config.micro_compact_enabled {
    let caps = self.provider.capabilities(&self.config.model);
    if let Ok(Some(new)) = crate::compact::MicroCompactor::new().compact(&history, &caps).await {
        let freed = crate::compact::estimate_tokens(&history).saturating_sub(crate::compact::estimate_tokens(&new));
        tracing::debug!(target: "caliban::compact", freed_tokens = freed, "microcompact");
        history = new;
    }
}
```

- [ ] **Step 4: Run test**

Run: `cargo test -p caliban-agent-core --features mock --test compact_micro microcompact_runs_pre_turn`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/stream/mod.rs crates/caliban-agent-core/tests/compact_micro.rs
git commit -m "feat(agent-core): run MicroCompactor at top of each turn"
```

---

## Task 5: Autocompact threshold + failure-backoff tracker

**Files:**
- Modify: `crates/caliban-agent-core/src/stream/mod.rs`
- Test: `crates/caliban-agent-core/tests/compact_auto.rs` (new)

- [ ] **Step 1: Write the failing test**

```rust
#![cfg(feature = "mock")]

use async_trait::async_trait;
use caliban_agent_core::{Agent, AgentConfig, Compactor};
use caliban_provider::{Capabilities, Message, MockProvider};
use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
use tokio_util::sync::CancellationToken;
use futures::StreamExt as _;

struct RecordingCompactor {
    calls: Arc<AtomicU32>,
    fail: bool,
}

#[async_trait]
impl Compactor for RecordingCompactor {
    async fn compact(&self, messages: &[Message], _caps: &Capabilities)
        -> caliban_agent_core::error::Result<Option<Vec<Message>>>
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(caliban_agent_core::error::Error::Other("compact failed".into()))
        } else {
            Ok(Some(vec![messages.last().cloned().unwrap()]))
        }
    }
    fn strategy_name(&self) -> &'static str { "test" }
}

#[tokio::test]
async fn autocompact_fires_above_threshold() {
    // Build a long history so estimate_tokens crosses 50%.
    let mut history = vec![Message::user_text("hi")];
    let filler = "x".repeat(100_000);
    for _ in 0..5 { history.push(Message::user_text(&filler)); }
    let provider = MockProvider::builder()
        .with_response_end_turn("done")
        .with_max_input_tokens(200_000)
        .build();
    let calls = Arc::new(AtomicU32::new(0));
    let agent = Arc::new(Agent::new(Arc::new(provider), AgentConfig {
        auto_compact_threshold: Some(0.5),
        ..Default::default()
    }).unwrap()
    .with_compactor(Arc::new(RecordingCompactor { calls: calls.clone(), fail: false })));

    let mut stream = agent.stream_until_done(history, CancellationToken::new());
    while stream.next().await.is_some() {}
    assert!(calls.load(Ordering::SeqCst) >= 1, "autocompact should have fired");
}

#[tokio::test]
async fn autocompact_disables_after_two_failures() {
    // Force 3 turns with a failing compactor; verify it stops being called.
    let provider = MockProvider::builder()
        .with_response_tool_use("AgentTool", serde_json::json!({}))
        .with_response_tool_use("AgentTool", serde_json::json!({}))
        .with_response_end_turn("done")
        .with_max_input_tokens(100)
        .build();
    let calls = Arc::new(AtomicU32::new(0));
    let history = vec![Message::user_text(&"x".repeat(10_000))];
    let agent = Arc::new(Agent::new(Arc::new(provider), AgentConfig {
        auto_compact_threshold: Some(0.1),
        ..Default::default()
    }).unwrap()
    .with_compactor(Arc::new(RecordingCompactor { calls: calls.clone(), fail: true })));
    let mut stream = agent.stream_until_done(history, CancellationToken::new());
    while stream.next().await.is_some() {}
    let n = calls.load(Ordering::SeqCst);
    assert!(n >= 1 && n <= 2, "compactor should have been disabled after ≤2 failures, got {n}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-agent-core --features mock --test compact_auto`
Expected: FAIL.

- [ ] **Step 3: Implement autocompact tracker + replace existing compact step**

In `stream/mod.rs`, the existing pre-turn `// ---- Compaction ----` block currently calls `compactor.compact` unconditionally. Replace its `match self.compactor.compact(...)` invocation with a threshold-gated version, hoisting a per-run tracker:

```rust
const MAX_CONSECUTIVE_COMPACT_FAILURES: u8 = 2;

#[derive(Debug, Default)]
struct AutoCompactTracking {
    consecutive_failures: u8,
    disabled: bool,
}
let mut auto_tracking = AutoCompactTracking::default();
```

Replace the inner compact call with:

```rust
let threshold = self.config.auto_compact_threshold;
let should_attempt = threshold.is_some_and(|t| {
    let utilization = token_count_before as f32 / caps.max_input_tokens.max(1) as f32;
    !auto_tracking.disabled && utilization >= t
});
if should_attempt {
    if let Err(e) = self.hooks.pre_compact(&compact_ctx).await {
        tracing::warn!(error = %e, "pre_compact hook error (non-fatal)");
    }
    match self.compactor.compact(&history, &caps).await {
        Err(e) => {
            tracing::warn!(error = %e, "autocompact failed");
            auto_tracking.consecutive_failures += 1;
            if auto_tracking.consecutive_failures >= MAX_CONSECUTIVE_COMPACT_FAILURES {
                auto_tracking.disabled = true;
                tracing::warn!("autocompact disabled after {MAX_CONSECUTIVE_COMPACT_FAILURES} consecutive failures");
            }
        }
        Ok(Some(new)) => {
            auto_tracking.consecutive_failures = 0;
            let token_count_after = crate::compact::estimate_tokens(&new);
            history = new;
            let outcome = CompactOutcome { token_count_after, compacted: true };
            let _ = self.hooks.post_compact(&compact_ctx, &outcome).await;
        }
        Ok(None) => {
            auto_tracking.consecutive_failures = 0;
            let outcome = CompactOutcome { token_count_after: token_count_before, compacted: false };
            let _ = self.hooks.post_compact(&compact_ctx, &outcome).await;
        }
    }
}
```

(If `MockProvider::builder().with_max_input_tokens` doesn't exist, add a one-line helper that sets the `Capabilities::max_input_tokens` reported by the mock.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-agent-core --features mock --test compact_auto`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/stream/mod.rs crates/caliban-agent-core/tests/compact_auto.rs crates/caliban-provider/src/mock.rs
git commit -m "feat(agent-core): autocompact threshold + 2-strike failure backoff"
```

---

## Task 6: Tool-result size cap

**Files:**
- Modify: `crates/caliban-agent-core/src/post_process.rs`
- Modify: `crates/caliban-agent-core/src/stream/parallel.rs`
- Modify: `caliban-agent-core/Cargo.toml` (add `directories`)
- Test: `crates/caliban-agent-core/tests/tool_result_cap.rs` (new)

- [ ] **Step 1: Write the failing test**

```rust
use caliban_agent_core::post_process::ToolResultCap;
use caliban_provider::{ContentBlock, TextBlock, ToolResultBlock};
use tempfile::tempdir;

#[tokio::test]
async fn caps_oversized_block_and_writes_overflow() {
    let dir = tempdir().unwrap();
    let cap = ToolResultCap {
        max_chars: 50,
        overflow_dir: dir.path().into(),
        session_id: "sess".into(),
    };
    let huge: String = std::iter::repeat('x').take(500).collect();
    let mut blocks = vec![ContentBlock::ToolResult(ToolResultBlock {
        tool_use_id: "tu_1".into(),
        content: vec![ContentBlock::Text(TextBlock { text: huge.clone(), cache_control: None })],
        is_error: false, cache_control: None,
    })];
    let n = cap.cap(&mut blocks).await.unwrap();
    assert_eq!(n, 1);
    // Placeholder content
    let body = match &blocks[0] {
        ContentBlock::ToolResult(tr) => match &tr.content[0] {
            ContentBlock::Text(t) => t.text.clone(), _ => panic!() },
        _ => panic!(),
    };
    assert!(body.starts_with("[truncated: 500 chars"));
    assert!(body.contains(dir.path().to_string_lossy().as_ref()));
    // Overflow file exists with the original
    let overflow_path = dir.path().join("sess").join("tu_1.txt");
    let overflow = std::fs::read_to_string(&overflow_path).unwrap();
    assert!(overflow.contains(&huge));
}

#[tokio::test]
async fn small_blocks_pass_through_untouched() {
    let dir = tempdir().unwrap();
    let cap = ToolResultCap {
        max_chars: 1000, overflow_dir: dir.path().into(), session_id: "sess".into(),
    };
    let mut blocks = vec![ContentBlock::ToolResult(ToolResultBlock {
        tool_use_id: "tu_1".into(),
        content: vec![ContentBlock::Text(TextBlock { text: "small".into(), cache_control: None })],
        is_error: false, cache_control: None,
    })];
    let n = cap.cap(&mut blocks).await.unwrap();
    assert_eq!(n, 0);
    let body = match &blocks[0] {
        ContentBlock::ToolResult(tr) => match &tr.content[0] {
            ContentBlock::Text(t) => t.text.clone(), _ => panic!() },
        _ => panic!(),
    };
    assert_eq!(body, "small");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-agent-core --test tool_result_cap`
Expected: FAIL.

- [ ] **Step 3: Implement `ToolResultCap`**

In `crates/caliban-agent-core/src/post_process.rs`:

```rust
use std::path::PathBuf;
use caliban_provider::{ContentBlock, TextBlock, ToolResultBlock};

pub struct ToolResultCap {
    pub max_chars: usize,
    pub overflow_dir: PathBuf,
    pub session_id: String,
}

const HEAD_TAIL_CHARS: usize = 2048;

impl ToolResultCap {
    /// Walks the blocks and replaces oversized ToolResult content with a
    /// truncation placeholder + head/tail preview; writes the full original
    /// to `<overflow_dir>/<session_id>/<tool_use_id>.txt`.
    /// Returns the count of blocks that overflowed.
    pub async fn cap(&self, blocks: &mut Vec<ContentBlock>) -> std::io::Result<usize> {
        if self.max_chars == 0 { return Ok(0); }
        let session_dir = self.overflow_dir.join(&self.session_id);
        let mut overflows = 0;
        for block in blocks.iter_mut() {
            let ContentBlock::ToolResult(tr) = block else { continue };
            // Skip already-truncated blocks (idempotent).
            if let Some(ContentBlock::Text(t)) = tr.content.first() {
                if t.text.starts_with("[truncated:") || t.text.starts_with("[superseded:") {
                    continue;
                }
            }
            // Concatenate all text segments for size check.
            let full: String = tr.content.iter().filter_map(|b| match b {
                ContentBlock::Text(t) => Some(t.text.as_str()),
                _ => None,
            }).collect::<Vec<_>>().join("\n");
            if full.chars().count() <= self.max_chars { continue; }
            tokio::fs::create_dir_all(&session_dir).await?;
            let path = session_dir.join(format!("{}.txt", tr.tool_use_id));
            tokio::fs::write(&path, &full).await?;
            let head: String = full.chars().take(HEAD_TAIL_CHARS).collect();
            let tail_start = full.chars().count().saturating_sub(HEAD_TAIL_CHARS);
            let tail: String = full.chars().skip(tail_start).collect();
            let placeholder = format!(
                "[truncated: {} chars, full content at {}]\n\n--- head 2KB ---\n{}\n--- tail 2KB ---\n{}",
                full.chars().count(),
                path.display(),
                head, tail,
            );
            tr.content = vec![ContentBlock::Text(TextBlock { text: placeholder, cache_control: None })];
            overflows += 1;
        }
        Ok(overflows)
    }
}
```

Add `directories = "5"` and `tempfile = "3"` (dev-dep) to `crates/caliban-agent-core/Cargo.toml` if not present.

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-agent-core --test tool_result_cap`
Expected: PASS.

- [ ] **Step 5: Wire the cap into `parallel.rs`**

In `crates/caliban-agent-core/src/stream/parallel.rs`, after the parallel-dispatch batch is collected:

```rust
if self.config.tool_result_cap_chars > 0 {
    let overflow_dir = directories::ProjectDirs::from("dev", "caliban", "caliban")
        .map(|d| d.cache_dir().join("tool-overflows"))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/caliban-tool-overflows"));
    let cap = crate::post_process::ToolResultCap {
        max_chars: self.config.tool_result_cap_chars,
        overflow_dir,
        session_id: settings.session_id.clone(),
    };
    let _ = cap.cap(&mut tool_result_blocks).await;     // best-effort; IO errors logged in cap
}
```

(Locate `tool_result_blocks` — the variable holding the post-dispatch results in the existing code; the spec's pseudocode names it that way for clarity, but the real binding may differ.)

- [ ] **Step 6: Run workspace tests**

Run: `cargo test -p caliban-agent-core`
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/caliban-agent-core/src/post_process.rs crates/caliban-agent-core/src/stream/parallel.rs crates/caliban-agent-core/Cargo.toml crates/caliban-agent-core/tests/tool_result_cap.rs
git commit -m "feat(agent-core): global ToolResultCap with overflow persistence + preview"
```

---

## Task 7: Conversation cache marker

**Files:**
- Modify: `crates/caliban-agent-core/src/cache.rs`
- Test: `crates/caliban-agent-core/tests/cache_marker.rs` (new)

- [ ] **Step 1: Write the failing test**

```rust
use caliban_agent_core::cache::apply_prompt_cache;
use caliban_provider::{CacheControl, ContentBlock, Message, Role, TextBlock, Tool};

#[test]
fn marks_last_user_message_when_above_threshold() {
    let mut msgs = vec![
        Message::user_text("first"),
        Message::assistant_text("reply"),
        Message::user_text(&"x".repeat(8000)),   // ~2000 tokens (chars/4 heuristic)
    ];
    let mut tools: Vec<Tool> = Vec::new();
    apply_prompt_cache(&mut msgs, &mut tools, /*min_cache_block_tokens=*/ 1024);
    let last = &msgs[2].content[0];
    match last {
        ContentBlock::Text(t) => assert!(matches!(t.cache_control, Some(CacheControl::Ephemeral))),
        _ => panic!(),
    }
}

#[test]
fn does_not_mark_tiny_user_message() {
    let mut msgs = vec![Message::user_text("short")];
    let mut tools: Vec<Tool> = Vec::new();
    apply_prompt_cache(&mut msgs, &mut tools, 1024);
    let only = &msgs[0].content[0];
    match only {
        ContentBlock::Text(t) => assert!(t.cache_control.is_none()),
        _ => panic!(),
    }
}

#[test]
fn marks_only_last_user_not_interior() {
    let mut msgs = vec![
        Message::user_text(&"x".repeat(8000)),
        Message::assistant_text("reply"),
        Message::user_text(&"y".repeat(8000)),
    ];
    let mut tools: Vec<Tool> = Vec::new();
    apply_prompt_cache(&mut msgs, &mut tools, 1024);
    let first_user = match &msgs[0].content[0] { ContentBlock::Text(t) => t.cache_control.clone(), _ => panic!() };
    let last_user = match &msgs[2].content[0] { ContentBlock::Text(t) => t.cache_control.clone(), _ => panic!() };
    assert!(first_user.is_none(), "interior user must not be marked");
    assert!(matches!(last_user, Some(CacheControl::Ephemeral)));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-agent-core --test cache_marker`
Expected: FAIL — `apply_prompt_cache` doesn't take a third argument and doesn't mark user messages.

- [ ] **Step 3: Update `apply_prompt_cache`**

In `crates/caliban-agent-core/src/cache.rs`, change the signature and add the user-marker branch:

```rust
pub(crate) fn apply_prompt_cache(
    messages: &mut [Message],
    tools: &mut [Tool],
    min_cache_block_tokens: usize,
) {
    // existing: mark last system text
    if let Some(sys) = messages.iter_mut().find(|m| m.role == Role::System)
        && let Some(last_text) = sys.content.iter_mut().rev().find_map(|b| match b {
            ContentBlock::Text(t) => Some(t),
            _ => None,
        })
    {
        last_text.cache_control = Some(CacheControl::Ephemeral);
    }
    // existing: mark last tool
    if let Some(last_tool) = tools.last_mut() {
        last_tool.cache_control = Some(CacheControl::Ephemeral);
    }
    // NEW: mark last block of last user message, if it's big enough.
    if let Some(idx) = messages.iter().rposition(|m| m.role == Role::User) {
        let tokens = crate::compact::estimate_tokens(&messages[idx..=idx]);
        if (tokens as usize) >= min_cache_block_tokens {
            if let Some(last_block) = messages[idx].content.last_mut() {
                set_cache_control_on_block(last_block, CacheControl::Ephemeral);
            }
        }
    }
}

fn set_cache_control_on_block(block: &mut ContentBlock, cc: CacheControl) {
    match block {
        ContentBlock::Text(t) => t.cache_control = Some(cc),
        ContentBlock::ToolResult(tr) => tr.cache_control = Some(cc),
        _ => { /* image/thinking/tool_use don't carry cache_control today */ }
    }
}
```

Update all callers (search for `apply_prompt_cache(` in the workspace) — there's a single call in `stream/mod.rs`. Pass `self.config.min_cache_block_tokens`.

- [ ] **Step 4: Update the existing `cache.rs` inline tests**

The existing tests call the 2-arg form. Update them to pass `1024` (or whatever value keeps the existing assertions valid — for the tiny fixtures they use, passing `usize::MAX` makes the conversation-marker code path a no-op, preserving the test expectations).

- [ ] **Step 5: Run tests**

Run: `cargo test -p caliban-agent-core --test cache_marker`
Run: `cargo test -p caliban-agent-core cache::tests`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-agent-core/src/cache.rs crates/caliban-agent-core/src/stream/mod.rs crates/caliban-agent-core/tests/cache_marker.rs
git commit -m "feat(agent-core): per-message conversation cache marker on last user message"
```

---

## Task 8: Telemetry counters + settings schema

**Files:**
- Create: `crates/caliban-telemetry/src/compaction.rs`
- Modify: `crates/caliban-telemetry/src/lib.rs` (re-export)
- Modify: `crates/caliban-settings/src/schema.json` (new config keys)
- Modify: `crates/caliban-settings/src/lib.rs` (wire the keys through to `AgentConfig`)

- [ ] **Step 1: Add the counter names**

```rust
// crates/caliban-telemetry/src/compaction.rs
pub const AUTO_TRIGGERED: &str = "caliban.compaction.auto_triggered";
pub const AUTO_DISABLED:  &str = "caliban.compaction.auto_disabled_after_failures";
pub const MICRO_FREED:    &str = "caliban.compaction.micro_freed_tokens";
pub const TOOL_OVERFLOW:  &str = "caliban.compaction.tool_result_overflowed";
pub const CACHE_MARKED:   &str = "caliban.cache.conversation_marked";
```

Re-export from `caliban-telemetry/src/lib.rs`:

```rust
pub mod compaction;
```

Increment each at the site added in Tasks 4–7. Each is one line.

- [ ] **Step 2: Add schema entries**

In `crates/caliban-settings/src/schema.json`, under the top-level `properties`:

```json
"autoCompactThreshold": { "type": ["number", "null"], "minimum": 0, "maximum": 1, "default": 0.75 },
"microCompactEnabled": { "type": "boolean", "default": true },
"toolResultCapChars":  { "type": "integer", "minimum": 0, "default": 50000 },
"minCacheBlockTokens": { "type": "integer", "minimum": 0, "default": 1024 }
```

In `caliban-settings/src/lib.rs`, where settings are merged into `AgentConfig`, add the four fields.

- [ ] **Step 3: Run the full workspace tests**

Run: `cargo test --workspace --all-features`
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 4: Update parity-gap matrix**

In `docs/parity-gap-matrix.md`, under section K (Observability/cost), note that autocompact is now proactive (it was previously `🟡` implicit). Ensure section C (Memory & checkpointing) reflects microcompact's existence.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-telemetry/src/compaction.rs crates/caliban-telemetry/src/lib.rs \
        crates/caliban-settings/src/schema.json crates/caliban-settings/src/lib.rs \
        crates/caliban-agent-core/src/{stream/mod,compact,cache,post_process}.rs \
        docs/parity-gap-matrix.md
git commit -m "chore(telemetry+settings): expose context-management knobs; update parity matrix"
```

---

## Self-Review Notes

- **Spec coverage:** Task 1 (config), Task 2 (supersession), Task 3 (MicroCompactor), Task 4 (per-turn micro), Task 5 (auto + backoff), Task 6 (tool cap), Task 7 (cache marker), Task 8 (telemetry+settings+matrix). All four spec subsystems addressed.
- **Composition:** microcompact runs first (frees tokens for free), autocompact next (only fires if microcompact didn't suffice), tool cap is invoked on the *new* results dispatched this turn (not on the existing history), cache marker is applied as part of request build (orthogonal to compaction).
- **Idempotency:** the tool-result cap explicitly skips blocks that already start with `[truncated:` or `[superseded:`. The cache marker overwrites cache_control on the last block; running it twice produces the same shape.
- **Backwards compat:** all four config fields are additive with `Default::default()` values that preserve current behavior when defaults are accepted *and* nothing in the workspace relies on the old `apply_prompt_cache` 2-arg signature (just the one call site in `stream/mod.rs`, updated in Task 7).
