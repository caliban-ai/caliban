# Prefill-aware stream watchdog + stream-path total-timeout exemption

**Date:** 2026-07-03
**Tickets:** [#263](https://github.com/caliban-ai/caliban/issues/263) (idle watchdog too aggressive for slow prefill), [#254](https://github.com/caliban-ai/caliban/issues/254) (total per-request timeout kills healthy slow streams)
**Status:** Approved (design)

## Problem

Two related timeout mechanisms bias against slow local models:

1. **Idle watchdog (#263).** `WatchedStream` (`crates/caliban-provider/src/stream.rs`) aborts with
   `Error::StreamIdle` when no chunk arrives within `stream_idle_timeout_ms` (default 90s). It measures
   idle time from stream construction and cannot tell a slow **prefill** (no first token yet — legitimate
   for a large-context turn on a slow/emulated local model) from a **mid-content stall**. Late-turn
   local-model runs trip the 90s window during prefill even though the model is actively working.
   Evidence: 3/25 instances in the 27b containerized SWE-bench eval (2026-06-25) killed by
   `stream idle for 90s`, all late-turn (35–49).

2. **Total per-request timeout (#254).** The provider builds one reqwest client for both `send` and
   `stream`, using reqwest's **total** `.timeout()` (ollama default 300s). For a streaming turn this caps
   the entire turn's generation regardless of whether tokens are flowing, killing a healthy-but-slow
   stream mid-response with `operation timed out`.

These are two halves of the same "timeouts are anti-local-model" problem. #254's proposed fix (drop the
total timeout, rely on the idle watchdog) would leave #263's prefill cases failing — and may surface them
*more* once the total timeout stops pre-empting. So both are fixed together here.

## Non-goals

- Per-provider timeout configuration (ollama vs anthropic vs openai getting distinct values). Deferred to
  epic [#259](https://github.com/caliban-ai/caliban/issues/259)'s config surface. This PR uses global
  config + an ollama-specific env override.
- Retrying mid-stream interruptions (that is #245, closed) or changing the non-streaming `send` path's
  bounded total timeout (kept as-is).

## Architecture

Two independent seams, one PR:

- **Seam A — app-level idle detection** (`WatchedStream`, agent-core): make the watchdog *phase-aware* so
  a slow prefill gets a generous budget while mid-stream stalls keep the tight window.
- **Seam B — transport-level total timeout** (provider clients): stop applying reqwest's *total*
  `.timeout()` to the streaming path; keep it on the non-streaming `send` path.

They compose: after Seam B the transport only catches **connect** failures, and `WatchedStream` becomes
the **sole** post-header stall detector. This is why we do **not** use reqwest `.read_timeout()` on the
stream client — it is a single fixed value that cannot express the prefill/content distinction, so it would
duplicate (and fight) the smarter app-level watchdog.

## Components

### Component 1 — `WatchedStream` two-phase budget

File: `crates/caliban-provider/src/stream.rs`

- New fields: `prefill: Duration`, `first_chunk_seen: bool`. Constructor becomes
  `new(inner, idle, prefill)`.
- In `poll_next`:
  - On the first `Poll::Ready(Some(item))`, set `first_chunk_seen = true` (in addition to the existing
    `last_chunk_at` reset).
  - On `Poll::Pending`, the active idle limit is `if first_chunk_seen { idle } else { prefill }`. The
    half-time warning and the abort both compare `last_chunk_at.elapsed()` against the **active** limit.
- Error type is unchanged: `Error::StreamIdle(Duration)`. No new variant → no ripple through
  `TransportErrorClass`, `StopCondition`, or the TUI surface. Observability instead gains a
  `phase = "prefill" | "content"` field on the `recovery.stream_idle.warning` / `.abort` tracing events so
  logs and eval artifacts can distinguish the two.
- Wakeup-timer re-arm logic (the single resettable `tokio::time::Sleep` from #117) is preserved; only the
  deadline it targets changes with the active limit.

### Component 2 — config knobs

File: `crates/caliban-agent-core/src/agent.rs`

- Add `pub stream_prefill_timeout_ms: u32`, default **300_000** (300s). Frontier models prefill in
  milliseconds and never approach it, so a single generous global default is safe for all providers.
- Keep `stream_idle_timeout_ms: u32` default `90_000`.
- Semantics: the watchdog gate is unchanged (`stream_idle_timeout_ms > 0` enables it). The prefill limit
  resolves to `stream_idle_timeout_ms` when `stream_prefill_timeout_ms == 0`, preserving today's behavior
  for anyone who zeroes it.

### Component 3 — wiring the budgets into the stream

File: `crates/caliban-agent-core/src/stream/mod.rs` (~line 1146)

- Compute `prefill = if cfg.stream_prefill_timeout_ms > 0 { prefill_ms } else { idle_ms }` and pass both
  `idle` and `prefill` into `WatchedStream::new`.

### Component 4 — Settings plumbing (TOML knob)

File: `crates/caliban-settings/src/settings.rs`

- Add `Option<u32>` fields `stream_idle_timeout_ms` and `stream_prefill_timeout_ms` to `Settings`.
  (`stream_idle_timeout_ms` has no TOML knob today — programmatic only — so this delivers the
  "widen the window without a rebuild" acceptance criterion for both budgets.)
- Add an `apply_stream_watchdog(&self, cfg: &mut AgentConfig)` helper mirroring the existing
  `apply_context_management` pattern (only fields explicitly set override the defaults).
- **Wire the call in `build_agent`.** The existing `apply_context_management` has a documented history
  (PR #60) of the helper being added but the call site forgotten; a test guards against that here.

### Component 5 — stream-path total-timeout exemption (Seam B)

Files: `crates/caliban-common/src/http.rs`, `crates/caliban-provider-openai/src/transport/direct.rs`,
`crates/caliban-provider-ollama/src/transport/direct.rs`, `crates/caliban-provider-ollama/src/config.rs`

- `http.rs`: add `build_stream_client(connect_timeout: Duration)` that sets `.connect_timeout(...)` and
  **no** `.timeout()`. The existing `build_client` (bounded total timeout) is unchanged.
- Provider transports hold a second `stream_client` built via `build_stream_client`. The `send` path keeps
  using the existing bounded client → satisfies #254's "non-streaming requests retain a bounded total
  timeout."
- **User-requested control setting:** `stream_total_timeout_ms: Option<u32>` on the provider config.
  `None` (default) = no total cap on the stream path (the fix). `Some(n)` = re-impose an `n`-ms total
  `.timeout()` on the stream client for anyone who wants a hard wall-clock cap.
- Connect timeout for the stream client reuses `caliban_common::http::DEFAULT_TIMEOUT` (30s).
- **ollama env override:** `OLLAMA_STREAM_PREFILL_TIMEOUT_MS` and `OLLAMA_STREAM_IDLE_TIMEOUT_MS` read in
  ollama's `from_env` path (mirrors the existing `OLLAMA_BASE_URL`), so eval/emulated runs widen the window
  with zero file edits.

## Data flow

1. Provider `stream()` sends the request on `stream_client` (connect-timeout only, no total timeout).
2. Response headers arrive; `bytes_stream()` begins. agent-core wraps it in `WatchedStream::new(inner,
   idle, prefill)`.
3. Before the first chunk, the active idle limit is `prefill` (300s). A slow prefill is tolerated.
4. On the first chunk, `first_chunk_seen = true`; the active limit drops to `idle` (90s).
5. A genuine mid-content stall aborts at `idle`; a stream that never produces any token aborts at
   `prefill`; a connect failure surfaces as `Network`. A healthy slow stream is never aborted by
   wall-clock.

## Error handling

- Genuine hangs are still caught: mid-content stall → abort at `idle`; no-first-token hang → abort at
  `prefill`. Both surface as `StopCondition::StreamIdle` (`StopLevel::Error`), unchanged.
- Connect failures still classify as `Network`.
- Non-streaming `send` requests retain their bounded total timeout.

## Testing

- **Unit (`stream.rs`):**
  - Prefill grace: a stream that yields no chunk does **not** abort before the prefill budget (prefill >
    idle; assert it survives past the idle window).
  - Mid-content tightness: after one chunk, a stall aborts at `idle`, not `prefill`.
  - Existing `passes_through_normal_data` and `resets_idle_clock_on_each_chunk` still pass with the new
    constructor signature.
- **Settings (`settings.rs`):** round-trip parse, `apply_stream_watchdog` overrides each field, and
  unset-leaves-default — mirroring the existing `apply_context_management` tests.
- **Integration (`crates/caliban-agent-core/tests/recovery_stream_idle.rs`):** a MockProvider that delays
  the first chunk within the prefill budget completes; one that stalls mid-content aborts with
  `StreamIdle`.
- **ollama (`config.rs`):** `from_env` parses the new `OLLAMA_STREAM_*` knobs.
- **http.rs:** `build_stream_client` constructs successfully (no total timeout applied).
- **Out-of-band (not CI):** re-run the 27b containerized SWE-bench eval on an unloaded `.240` and confirm
  **0** `stream idle for 90s` errors — the acceptance criterion that proves the real-world fix.

## Acceptance criteria (from the tickets)

- [x] A slow local-model turn that stalls only on long prefill (no output token yet) is not aborted before
      a configurable, local-appropriate budget. (Component 1 + 2)
- [x] A genuinely hung stream is still caught. (mid-content aborts at `idle`; no-token hang at `prefill`)
- [x] A streaming turn steadily producing tokens is not aborted by a fixed total timeout. (Component 5)
- [x] Non-streaming requests retain a bounded total timeout. (Component 5, `send` path unchanged)
- [x] An env/config knob widens the window without a rebuild. (Component 4 TOML + Component 5 env)
- [ ] 27b containerized eval re-run shows 0 `stream idle for 90s` errors. (out-of-band, post-merge)
