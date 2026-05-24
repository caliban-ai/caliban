# Performance Baseline — Design

**Date:** 2026-05-23
**Status:** Approved
**Target branch:** `jf/feat/perf/baseline`

## Goal

Land a coherent "performance baseline" PR that closes the highest-value gaps from a recent performance audit. The audit found that most of caliban's transport and streaming layer is already correct (shared `reqwest::Client`, end-to-end streaming, exponential-backoff retries with jitter, per-request timeouts). The gaps are concentrated in five areas:

1. Anthropic prompt caching is wired through the IR + adapter but never enabled at any construction site.
2. OpenAI's automatic prompt cache hits are extracted into the usage struct but not surfaced to the user.
3. `reqwest` is built without HTTP/2 or async DNS.
4. There is no time-to-first-token measurement, so future optimizations have no baseline.
5. Two `std::fs` calls run on the async runtime during startup.

This PR addresses all five.

## Non-goals

- Parallel tool execution (ADR 0009 deferred this; needs its own design).
- Pre-dispatch of tool calls mid-stream (modest complexity; saves the model-done → tool-start gap but tightly couples to parallel execution).
- Provider trait redesign (`impl Stream` vs `Pin<Box<dyn Stream>>`) — would break dyn-compatibility with `Arc<dyn Provider>`.
- Hedged requests, circuit breakers (defer until a router exists).
- Google Gemini context caching — requires implementing the separate `cachedContents` resource API. Not a "simple addition."
- `simd-json` (SSE chunks are small; parser is not a measured bottleneck).

## Scope summary

| # | Item | Layer | LOC estimate |
|---|---|---|---|
| 1 | Anthropic `cache_control: Ephemeral` on system + last tool def | agent-core + IR | ~80 |
| 2 | TUI usage line shows `cache_read` + `cache_creation` tokens | tui + transcript | ~30 |
| 3 | HTTP/2 enabled on workspace `reqwest` | Cargo.toml | ~1 |
| 4 | `hickory-dns` enabled on workspace `reqwest` | Cargo.toml | ~1 |
| 5 | TTFT + TBT capture, tracing event + TUI footer | agent-core + tui | ~120 |
| 6 | `tokio::fs` replaces `std::fs` in `main.rs` startup | caliban bin | ~10 |
| 7 | Gemini caching deferred — note in PR description + ADR stub | docs | ~0 |

Total: ~240 LOC plus tests.

## Architecture

### 1. Anthropic prompt caching

**Wire-format reminder.** Anthropic's `cache_control` is set on a specific *block* (text block, tool definition, image block, tool result). Setting it on a block marks "cache everything in the request up to and including this block." The token-cost surcharge for cache creation is ~25%; cache reads cost ~10% of normal input. Break-even is ~1 cache hit per cache-write.

**Default behavior.** Caching is **default-on**. The user opts out with `--no-prompt-cache` (or `CALIBAN_NO_PROMPT_CACHE=1`). Rationale: the TUI is the dominant use case and is always interactive; cache hits start at turn 2.

**Caching policy for v1.** Set `cache_control: Ephemeral` on:

- The **last `TextBlock` of the system message**, if a system message exists.
- The **last `ToolDef` in the tools array**, if any tools are registered.

This caches the system prompt and the entire tool registry. Both are stable across a session, so subsequent turns hit the cache for both. Conversation messages are NOT marked in v1 (more nuanced; defer).

**Where the change lives.** The agent's turn loop (`crates/caliban-agent-core/src/stream.rs`) builds `CompletionRequest` per turn from `messages` + `tools` from the registry. Add an `Agent` field `prompt_cache: bool` (builder method `.prompt_cache(bool)`) and a small helper:

```rust
fn apply_prompt_cache(messages: &mut [Message], tools: &mut [ToolDef]) {
    // Mark last text block of system message (if any).
    if let Some(sys) = messages.iter_mut().find(|m| m.role == Role::System)
        && let Some(ContentBlock::Text(last)) = sys.content.iter_mut().rev().find(|b| matches!(b, ContentBlock::Text(_)))
    {
        last.cache_control = Some(CacheControl::Ephemeral);
    }
    // Mark last tool def (if any).
    if let Some(last) = tools.last_mut() {
        last.cache_control = Some(CacheControl::Ephemeral);
    }
}
```

Called inside the turn-build path before the request goes to the provider. Only invoked when `self.prompt_cache` is true.

**CLI plumbing.** New `Args` field on the binary:

```rust
/// Disable Anthropic prompt caching (default: enabled).
#[arg(long)]
pub(crate) no_prompt_cache: bool,
```

Read via `CALIBAN_NO_PROMPT_CACHE` env var also. Threaded into `Agent::builder().prompt_cache(!args.no_prompt_cache)`.

**Cross-provider safety.** OpenAI, Gemini, Ollama IR converters either ignore `cache_control` or already do today (it's `Option<>` with default-skip-serialize in their wire formats). The Anthropic adapter is the only one that maps it to a wire field.

**Tests.** Two unit tests in `crates/caliban-agent-core` (or wherever `apply_prompt_cache` lives):
- Empty system + empty tools → no mutation, no panic.
- One system message with two text blocks + three tools → last text block of system is marked; last tool is marked; nothing else is.

Plus one integration test asserting that the Anthropic wire JSON contains `"cache_control":{"type":"ephemeral"}` on the expected positions when `prompt_cache: true`, and contains no `cache_control` keys when false.

### 2. OpenAI cache hits visible in TUI

**Already known.** OpenAI prompt caching is automatic for prompts ≥1024 tokens; no API parameter. The OpenAI adapter (`crates/caliban-provider-openai/src/ir_convert.rs:314-333` and `stream_parse.rs:194-198`) already extracts `prompt_tokens_details.cached_tokens` into the IR's `Usage::cache_read_input_tokens`. Same field is populated by the Anthropic adapter from `usage.cache_read_input_tokens`.

**The miss.** The TUI's `UsageSummary` transcript line and the `caliban-sessions` `--quiet` mode footer show `X↑ Y↓ tokens` and never mention cache hits.

**Change.**

- Extend `TranscriptLine::UsageSummary` to carry `cache_read: Option<u32>` and `cache_creation: Option<u32>` (already on the IR `Usage`).
- The render arm in `caliban/src/tui.rs` produces:
  - When `cache_read > 0` (any provider): `[caliban: N turns · X↑ (R cached) Y↓ tokens]`
  - When `cache_creation > 0` AND `cache_read == 0` (first turn of an Anthropic session): `[caliban: N turns · X↑ (C cache write) Y↓ tokens]`
  - Both nonzero: `[caliban: N turns · X↑ (R cached, C write) Y↓ tokens]`
  - Neither: unchanged from today.
- The session save line (printed by `caliban-sessions` via stderr in non-TUI mode) follows the same format.

**Tracing emission.** At end-of-turn, emit `tracing::info!(target: "caliban::cache", read = R, creation = C, "prompt cache stats")` when either is > 0. Useful for log analysis.

**Tests.** A snapshot-shaped test on the rendering helper covering each cache-state combination.

### 3. HTTP/2

One-line change to the workspace `reqwest` feature list:

```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream", "http2"] }
```

reqwest negotiates HTTP/2 via ALPN when the server supports it; no per-request code change needed. Effect: connection multiplexing reduces handshake overhead on cold connections, sets us up for parallel tool calls later.

### 4. hickory-dns

Same line, append `"hickory-dns"`. reqwest then uses the async hickory resolver with built-in caching instead of `getaddrinfo` per connection. Effect: cold DNS lookups go from 10–50 ms to negligible; warm lookups skip the syscall entirely.

**Risk.** hickory pulls in extra crates (~5). Build time increases marginally. Worth it.

### 5. TTFT / TBT

**What's measured.**

- **TTFT** (time-to-first-token): from "request sent to provider" → "first `StreamEvent::Delta` arrives." Measured per turn.
- **TBT** (time-between-tokens): mean inter-arrival time across all deltas in a turn. Single scalar per turn, computed as `(t_last_delta - t_first_delta) / (n_deltas - 1)` if `n_deltas >= 2`, else `None`.

**Capture site.** Inside the agent's `stream_until_done` loop in `crates/caliban-agent-core/src/stream.rs`. Add a small helper:

```rust
struct TurnTiming {
    request_sent_at: Instant,
    first_delta_at: Option<Instant>,
    last_delta_at: Option<Instant>,
    delta_count: u32,
}

impl TurnTiming {
    fn observe_delta(&mut self) {
        let now = Instant::now();
        self.first_delta_at.get_or_insert(now);
        self.last_delta_at = Some(now);
        self.delta_count += 1;
    }
    fn ttft(&self) -> Option<Duration> {
        self.first_delta_at.map(|t| t.saturating_duration_since(self.request_sent_at))
    }
    fn tbt(&self) -> Option<Duration> {
        match (self.first_delta_at, self.last_delta_at, self.delta_count) {
            (Some(f), Some(l), n) if n >= 2 => Some(l.saturating_duration_since(f) / (n - 1)),
            _ => None,
        }
    }
}
```

**Plumbing.** A new `TurnEvent::TurnEnd` variant payload carries `ttft: Option<Duration>` + `tbt: Option<Duration>` alongside the existing fields (turn_count, usage). Today's `TurnEvent::TurnEnd` already exists — extend it.

**Tracing.** Per-turn `tracing::info!(target: "caliban::timing", ttft_ms = ?, tbt_ms = ?, "turn timing")` emitted at TurnEnd.

**TUI display.** `TranscriptLine::UsageSummary` gains `last_turn_ttft_ms: Option<u64>` (the most-recent turn's TTFT; per-turn is the most actionable view, and average can be added later if needed). Rendered as `[caliban: N turns · ... · TTFT 412ms]` when present.

**Tests.** `TurnTiming` is a pure data struct; standard arithmetic tests cover all branches.

### 6. tokio::fs in startup

`caliban/src/main.rs:319` and `:321` both currently call sync `std::fs` from inside `async fn main`. The path is debug-log file setup. Switch to `tokio::fs::create_dir_all(&parent).await?` and `tokio::fs::OpenOptions::new()...open(&path).await?`. These are one-shot startup operations, not hot path — change is purely about hygiene (don't park the async runtime on a sync syscall, even briefly).

### 7. Gemini caching deferred

Note in PR description. No code change. A short TODO comment goes in `crates/caliban-provider-google/src/ir_convert.rs` near the hardcoded `cache_creation_input_tokens: None` lines:

```rust
// Gemini's context caching uses a separate `cachedContents` API resource
// rather than per-block markers. Not implemented; revisit when needed.
```

## Data flow

```
[user submits prompt]
        |
        v
Agent::builder().prompt_cache(!args.no_prompt_cache).build()
        |
        v
agent.stream_until_done(messages, cancel)
        |
        v
turn_loop:
  - build CompletionRequest from messages + tools
  - if self.prompt_cache: apply_prompt_cache(&mut messages, &mut tools)
  - timing.request_sent_at = Instant::now()
  - provider.complete(req).await -> Stream<StreamEvent>
  - for each event:
      - on first Delta: timing.observe_delta() (sets first_delta_at)
      - on subsequent Deltas: timing.observe_delta() (updates last_delta_at + count)
      - emit TurnEvent::AssistantTextDelta / ToolCallStart / ... to consumer
  - on stream end:
      - emit TurnEvent::TurnEnd { ttft, tbt, usage_with_cache_tokens, ... }
      - tracing::info!(target: "caliban::timing", ttft_ms, tbt_ms, ...)
      - tracing::info!(target: "caliban::cache", read, creation, ...) when nonzero
        |
        v
TUI consumes TurnEnd → updates UsageSummary transcript line with cache + TTFT fields
```

## Configuration surface

| Flag | Env var | Default | Effect |
|---|---|---|---|
| `--no-prompt-cache` | `CALIBAN_NO_PROMPT_CACHE` | false (caching on) | Skip `cache_control` markers on system + tools |

No other new flags. HTTP/2 and hickory-dns are unconditional (compile-time features).

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Prompt caching surcharge hurts one-shot users | `--no-prompt-cache` documented in README; the rotation cost is small (~25% on first turn input only) |
| HTTP/2 negotiation incompatibility with some proxy/transparent middlebox | reqwest falls back to HTTP/1.1 via ALPN cleanly; no action needed |
| hickory-dns adds build time | Acceptable; the deps are well-established |
| TTFT measurement perturbs the hot loop | `Instant::now()` is ~20ns on Linux; negligible against network RTT |
| Cache hits when content hash collides (cache reused from prior session unexpectedly) | Anthropic's cache key includes the full prompt prefix; no risk of leakage. Marking system + tools doesn't expose anything not already in the request |

## Success criteria

- `cargo test --workspace` passes including new unit + integration tests.
- A turn under Anthropic with `--no-prompt-cache` produces wire JSON with zero `cache_control` keys (golden-file test or wiremock assertion).
- A turn under Anthropic with default settings (no flag) produces wire JSON with `cache_control: {"type":"ephemeral"}` on:
  - the last `text` block in the system message
  - the last entry in the `tools` array
- TUI `UsageSummary` line (rendered once per run at `RunEnd`) shows `(N cached)` when cache_read > 0 (Anthropic on second turn onward, or OpenAI on prompts ≥1024 tokens).
- TUI `UsageSummary` line shows `TTFT XXXms` reflecting the most recent turn of the run when at least one delta was received.
- `tracing::info!(target: "caliban::timing", ...)` events fire once per turn (visible with `--debug`).
- `cargo build --workspace` succeeds with hickory + http2 features enabled.
- ADR or note documents the Gemini caching deferral.

## Out-of-scope follow-up work

Listed for context, NOT for this PR:

- Per-conversation cache_control on the last user message (more aggressive caching, more nuance — separate PR).
- Aggregate TTFT/TBT statistics in the TUI (min/max/avg across a run).
- Pre-dispatch tool calls mid-stream.
- Parallel tool execution (ADR 0009 follow-up).
- Hot-path tracing spans with sampling.
- Gemini `cachedContents` API integration.
- Circuit breakers + hedged requests (await router subsystem).
