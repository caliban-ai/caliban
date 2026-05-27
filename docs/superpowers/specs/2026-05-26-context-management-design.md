# Context-window management — Design

**Date:** 2026-05-26
**Author:** john.ford2002@gmail.com
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** *(none yet — propose 0042 if this lands)*
**Origin:** `docs/TODO.md` findings on autocompact threshold, microcompact (supersession), tool-result size cap with persist-to-disk, and per-message conversation cache marker.

## Goal

caliban already has compaction *machinery* (`NoopCompactor`, `DropOldestCompactor`, `SummarizingCompactor` behind the `Compactor` trait) but the loop never *uses* it proactively, leaves stale tool results in place across turns, has no system-wide cap on tool-result size, and caches only the static prefix — the conversation history is re-billed every turn. Together, those gaps mean long sessions silently approach the context limit and pay extra cost the whole way there.

After this lands:

1. **Autocompact** fires when token usage crosses a configurable threshold (default 0.75), with backoff after consecutive failures.
2. **Microcompact** runs every turn — cheap, LLM-free — and replaces superseded tool results with a one-line placeholder.
3. **Tool-result size cap** writes overflow to disk, replaces the inline block with a `[truncated: N chars, full at <path>]` preview, and applies uniformly to MCP + built-ins.
4. **Per-message cache marker** on the last user message means turn N+1 reuses turn N's prefix on Anthropic — turning the cache_read curve from flat to linear-with-history.

## Non-goals

- **No new pre-turn hook surface.** Auto-compaction reuses `before_turn` / `pre_compact` / `post_compact`.
- **No supersession across tools.** Microcompact's predicate is intentionally per-tool (Read same-path, Grep same-args) — Bash and most others are never superseded.
- **No global LLM-driven compaction cadence change.** Spec A handles reactive compaction (after a 413); this spec handles the proactive side and the cheap microcompact pass.
- **No persistent overflow store.** Overflow files live under `~/Library/Caches/caliban/tool-overflows/<session>/<tool_use_id>.txt` and are cleaned up on session end. They are not durable across runs.
- **No cache-marker changes for non-Anthropic providers.** `CacheControl` already serializes to `None` for them; the new marker is wire-noop everywhere except Anthropic.

## Architecture

```
caliban-agent-core
  config.rs
    AgentConfig {
      auto_compact_threshold: Option<f32>      ← Some(0.75) default
      micro_compact_enabled: bool              ← true default
      tool_result_cap_chars: usize             ← 50_000 default
      min_cache_block_tokens: usize            ← 1024 default
    }

  compact.rs (existing)
    + MicroCompactor                            ← NEW strategy
        impl Compactor (cheap, no LLM)

  stream/mod.rs (turn loop)
    pre-turn:
      ┌─ run microcompact (every turn, fast)
      ├─ run autocompact if utilization > threshold
      └─ existing pre_compact / post_compact hooks fire for both

  post_process.rs (existing module)
    + cap_tool_results(blocks, session_id, cap, overflow_dir)
        for each ToolResult block:
          if len > cap:
            write full content → <overflow_dir>/<session>/<tool_use_id>.txt
            replace block content with:
              "[truncated: <N> chars, full at <path>;\nhead 2KB:\n<head>\n…\ntail 2KB:\n<tail>]"

  cache.rs (existing)
    apply_prompt_cache(messages, tools)
      existing: mark last system text + last tool
      + NEW: mark last block of last user message, gated by
             min_cache_block_tokens (skip tiny messages — caching them
             wastes a breakpoint and adds overhead)

caliban-telemetry
  CompactionMetrics
    auto_compact_triggered     (counter)
    auto_compact_disabled      (counter; bumps when consecutive_failures cap hit)
    micro_compact_tokens_freed (histogram)
    tool_result_cap_overflows  (counter, with tool name attribute)
    conversation_cache_marked  (counter)
```

`MicroCompactor` and `cap_tool_results` are independent units — either can ship without the other.

## Data model deltas

### `AutoCompactTracking` (new, per-run state)

Lives on the inner-loop state object created at the top of `stream_until_done_with_settings`:

```rust
#[derive(Debug, Default)]
struct AutoCompactTracking {
    last_attempt_turn: Option<u32>,
    consecutive_failures: u8,
    disabled: bool,        // sticky: once true, stays true for the run
}

const MAX_CONSECUTIVE_FAILURES: u8 = 2;
```

The tracker is per-run (lives in the `try_stream!` closure), not per-agent — restarting the run resets it.

### `MicroCompactor` strategy

```rust
/// Walks the message history and replaces superseded ToolResult blocks
/// with a one-line placeholder. "Superseded" means: same tool name +
/// same supersession key, AND a more recent invocation exists.
pub struct MicroCompactor {
    placeholder_fmt: fn(tool: &str, key: &str) -> String,
}

impl MicroCompactor {
    pub fn new() -> Self {
        Self { placeholder_fmt: |t, k| format!("[superseded: {t}({k})]") }
    }
}

/// Per-tool supersession predicate.
fn supersession_key(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "Read"       => input.get("file_path").and_then(|v| v.as_str()).map(String::from),
        "Grep"       => Some(input.to_string()),   // exact-args match
        "Glob"       => Some(input.to_string()),
        "WebFetch"   => input.get("url").and_then(|v| v.as_str()).map(String::from),
        _            => None,                       // not supersedable
    }
}
```

The strategy is LLM-free: it walks `messages` once, builds a `HashMap<(String, String), tool_use_id>` of the latest invocation per `(tool, key)`, then sweeps again and replaces older `ToolResult` blocks whose corresponding `ToolUse` was beaten. Cost: O(n) walk per turn.

### Tool-result cap

A `cap_tool_results` pass added to `crates/caliban-agent-core/src/post_process.rs`:

```rust
pub struct ToolResultCap {
    pub max_chars: usize,
    pub overflow_dir: PathBuf,    // ~/Library/Caches/caliban/tool-overflows
    pub session_id: String,
}

impl ToolResultCap {
    /// Called after parallel tool dispatch (see stream/parallel.rs) on
    /// the batch of newly-produced ToolResult blocks. Mutates in place.
    pub async fn cap(&self, blocks: &mut Vec<ContentBlock>) -> std::io::Result<usize> {
        // returns count of blocks that overflowed
    }
}
```

The cap is enforced **uniformly** across:
- Built-in tools (already have per-tool caps; those become *soft* limits — informational, while the global cap is the hard ceiling).
- MCP tools (currently uncapped — biggest beneficiary).
- Agent-tool results.

Per-tool overrides land later via a `max_result_chars_hint` field on the `Tool` descriptor (in `caliban-provider::tool::Tool`). v1 ships with a single global constant.

### Cache marker extension

`cache.rs::apply_prompt_cache` adds a third marker:

```rust
pub(crate) fn apply_prompt_cache(
    messages: &mut [Message],
    tools: &mut [Tool],
    min_cache_block_tokens: usize,
) {
    // existing: mark last system text + last tool (unchanged)
    …

    // NEW: mark last block of last user message, if it's big enough.
    if let Some(last_user_idx) = messages
        .iter()
        .rposition(|m| m.role == Role::User)
    {
        let user = &mut messages[last_user_idx];
        let user_tokens = estimate_message_tokens(user);
        if user_tokens >= min_cache_block_tokens {
            if let Some(last_block) = user.content.last_mut() {
                set_cache_control(last_block, CacheControl::Ephemeral);
            }
        }
    }
}
```

This uses 3 of the 4 cache breakpoints Anthropic allows. Marker placement is idempotent — re-running `apply_prompt_cache` on already-marked messages keeps the same shape.

## Flows

### Autocompact (pre-turn)

```
'outer turn loop, top of each iteration:
  1. estimate_tokens(&history) → tokens_used
  2. capacity = caps.max_input_tokens
  3. utilization = tokens_used as f32 / capacity as f32
  4. if !tracking.disabled && utilization > config.auto_compact_threshold:
       pre_compact hook fires
       compactor.compact(&history, &caps).await:
         Ok(Some(new))  → history = new; tracking.consecutive_failures = 0
                          post_compact hook fires
         Ok(None)       → no-op (compactor declined)
         Err(_)         → tracking.consecutive_failures += 1
                          if tracking.consecutive_failures >= MAX_CONSECUTIVE_FAILURES:
                            tracking.disabled = true
                            tracing::warn!("autocompact disabled after repeated failures")
       tracking.last_attempt_turn = Some(turn_index)
  5. continue normal turn
```

`auto_compact_threshold = None` disables autocompact entirely (same as setting threshold to `> 1.0`). Env override: `CALIBAN_AUTO_COMPACT_THRESHOLD=0.75`.

### Microcompact (pre-turn, before autocompact)

Runs *every* turn when `config.micro_compact_enabled`:

```
1. let freed_before = estimate_tokens(&history)
2. MicroCompactor.compact(&history, &caps).await
3. let freed_after  = estimate_tokens(&history)
4. metric: micro_compact_tokens_freed += (before - after)
```

Microcompact is fire-and-forget — `Ok(None)` is the common case (nothing was superseded). It does **not** call the `pre_compact` / `post_compact` hooks; those are for LLM-driven compaction so observers can react to "the model just summarized your history" (a meaningful event). Microcompact is a janitor pass — opaque and routine.

Order matters: microcompact first, then autocompact threshold check. Microcompact often frees enough that autocompact doesn't have to fire.

### Tool-result cap

Called from `crates/caliban-agent-core/src/stream/parallel.rs` after dispatch returns its batch:

```rust
// after dispatch
let mut tool_result_blocks = collect_results(handles).await;
self.tool_result_cap.cap(&mut tool_result_blocks).await?;
history.push(Message::user(tool_result_blocks));
```

Overflow file format (`<overflow_dir>/<session>/<tool_use_id>.txt`):

```
[caliban tool overflow]
tool_use_id: <id>
tool_name: <name>
session_id: <session>
timestamp: <ISO8601>
size_chars: <N>

<full output…>
```

Replacement block content (deterministic, ~4KB max):

```
[truncated: 87523 chars, full content at ~/Library/Caches/caliban/tool-overflows/abcd/xyz.txt]

--- head 2KB ---
<first 2048 chars>
--- tail 2KB ---
<last 2048 chars>
```

The model can re-read the full content with `Read{file_path: "/Users/.../tool-overflows/abcd/xyz.txt"}` — the existing `Read` tool already handles this, no extra plumbing.

### Cache marker

Already covered above. New unit test asserts:
- Exactly one Ephemeral marker on last user message block when its estimated tokens >= 1024.
- Zero markers on user messages when they're below the threshold.
- System + tool markers are still placed as before.
- Multi-turn assistant ↔ user back-and-forth: only the *last* user message is marked, never an interior one.

## Configuration surface

`AgentConfig` (`crates/caliban-agent-core/src/config.rs`) gains:

```rust
pub auto_compact_threshold: Option<f32>,    // None disables; Some(0.75) default
pub micro_compact_enabled: bool,             // true default
pub tool_result_cap_chars: usize,            // 50_000 default; 0 = disabled
pub min_cache_block_tokens: usize,           // 1024 default
```

Env overrides (parsed in `caliban-settings` and shimmed into `AgentConfig`):
- `CALIBAN_AUTO_COMPACT_THRESHOLD`: `0..=1.0` or `disabled`
- `CALIBAN_MICRO_COMPACT`: `true` / `false`
- `CALIBAN_TOOL_RESULT_CAP_CHARS`: integer
- `CALIBAN_MIN_CACHE_BLOCK_TOKENS`: integer

settings.json schema additions land in `caliban-settings/src/schema.json` and the merge layer; existing schema-validation warns-but-doesn't-abort behavior is preserved.

## Testing strategy

1. **Autocompact threshold:** `MockProvider` returns predictable usage; `RecordingCompactor` tracks calls. Set threshold to 0.5; build a history that crosses 50%; assert exactly one `compact` call per crossing turn. Verify the `consecutive_failures` backoff by failing `compact` twice and asserting `disabled = true` afterwards.
2. **Microcompact supersession:** build a history with two `Read{path: A}` results, assert the older becomes `[superseded: Read(A)]`. Two `Read{path: A}` interleaved with `Bash` calls: `Bash` is untouched, only the older `Read` is replaced. Cross-tool: `Read{A}` and `Grep{A}` do **not** supersede each other (different tool names).
3. **Tool-result cap:** synthesize a `ToolResult` block of 100 KB; cap to 50 KB; assert replacement content size <= 5 KB, overflow file exists, `[truncated: …]` placeholder present, head/tail strings match the original head/tail. Re-run the same cap on already-truncated blocks: no double-truncation, no second overflow file.
4. **Cache marker:** unit tests in `cache.rs` extending the existing suite. Multi-turn fixture; verify exactly one user marker plus existing system+tool markers. Tiny-user-message fixture (< `min_cache_block_tokens`): zero user markers.
5. **End-to-end:** `MockProvider` reports `cache_read_input_tokens` that climbs across turns when markers are present and stays flat when they're not. Assert the climbing case.

## Telemetry

Emitted by `caliban-telemetry::CompactionMetrics`:

- `caliban.compaction.auto_triggered` (counter, attrs: `strategy`, `tokens_before`, `tokens_after`)
- `caliban.compaction.auto_disabled_after_failures` (counter)
- `caliban.compaction.micro_freed_tokens` (histogram)
- `caliban.compaction.tool_result_overflowed` (counter, attrs: `tool_name`, `original_chars`)
- `caliban.cache.conversation_marked` (counter, attrs: `user_tokens`)

Existing `caliban::cache: prompt cache stats cache_read=…` log lines will show the conversation-marker benefit immediately: in a 20-turn session against Sonnet 4.6, expect `cache_read_input_tokens` to climb steadily instead of staying flat at the system-prefix size.

## Migration notes

- **Pure additive.** All new fields on `AgentConfig` have `Default` impls; existing callers compile unchanged.
- **`apply_prompt_cache` signature change.** Adds `min_cache_block_tokens: usize` parameter. All callers live in this workspace (`stream/mod.rs`, `cache.rs` tests). Mechanical update.
- **Per-tool soft limits stay.** The existing per-tool caps (`shell/bash.rs:STDOUT_CAP`, etc.) remain — they tune *per-tool* behavior (e.g. line-based truncation) before the global cap sees the block. Removing them is a follow-up cleanup PR after the global cap proves stable.
- **Overflow dir on Linux/Windows:** uses `directories::ProjectDirs::cache_dir()` so paths are correct per-OS. The example paths above show macOS for brevity.

## Open questions

1. **Should microcompact run after each tool dispatch, or only at turn boundary?** Turn boundary in v1 (cheaper, single pass). If users complain about mid-turn token spikes, re-evaluate.
2. **Should the overflow file be model-readable via a special re-fetch tool, or just Read?** Read in v1 — the path is right there in the placeholder, the model can just call `Read{path: <p>}`. Adding a dedicated tool is YAGNI.
3. **Cache marker on assistant messages too?** Anthropic allows it; would add another breakpoint. Skip in v1 — the system+tools+user-message trio is the documented sweet spot and we want headroom for future experiments.
4. **Settings rename: `auto_compact_threshold` vs `autocompactThreshold` vs `auto-compact-threshold`?** Match the existing `caliban-settings` style (snake_case in TOML, camelCase in JSON via serde).
