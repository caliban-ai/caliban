# Stream-timeout policy above the transport layer (#330)

**Status:** design settled (Option B, maintainer-confirmed during the post-0.4.0 QA review).
**Date:** 2026-07-05.

## Problem

Streaming timeouts are enforced inconsistently, split across the transport layer:

- **M13 — total-deadline caps long streams.** `#269` exempted streaming from reqwest's *total* `.timeout()` by adding `build_stream_client` (connect-timeout only), but adoption is per-transport and only **2 of 7 reqwest transports** switched: `openai/direct`, `ollama/direct`. The 5 laggards — `anthropic/direct`, `anthropic/vertex`, `google/ai_studio`, `google/vertex`, `openai/azure` — still call `build_client(config.timeout)`, so a total deadline (anthropic's default is 60s) kills any stream that runs longer.
- **M1 — connect→first-header hang.** Nothing bounds the wait *inside* `provider.stream(...)`'s `.send().await`. `build_stream_client` sets only `connect_timeout`; agent-core's `WatchedStream` (which already enforces a 300s **prefill** budget + a 90s **idle** budget) can only observe the stream *after* `.send()` returns headers. A server that accepts the TCP connection but never sends headers hangs forever (regression vs the old 300s cap; acute for a local ollama that OOMs mid-prefill).

`anthropic/transport/bedrock.rs` (AWS SDK) and the `caliban-provider-bedrock` crate are **not reqwest** and own their own timeouts — out of scope.

## Design (Option B) — four coherent layers

Enforcement moves up to where the idle/prefill watchdog already lives; transports own zero streaming-timeout *policy*:

| Layer | Bounds | Owner |
|-------|--------|-------|
| connect | TCP connect | transport's stream client (`connect_timeout`) |
| **first byte** | connect → response headers (inside `.send()`) | **agent-core** (`tokio::time::timeout` around `provider.stream()`) — NEW |
| prefill | headers → first chunk | `WatchedStream` (existing) |
| idle | between chunks | `WatchedStream` (existing) |

### agent-core

Wrap the `provider.stream(req)` future in `tokio::time::timeout(first_byte_budget, …)` **inside** the `with_retry` closure, so a wedged first attempt times out and retries. `first_byte_budget = stream_prefill_timeout_ms` (default 300_000) — one "time-to-first-token" budget, **no new config knob**. `0` disables (unbounded, today's behaviour). On elapse, surface a retryable stream error; on retry exhaustion it becomes terminal like any provider error. `WatchedStream` wrapping is unchanged and still applied after the stream is obtained.

### transports

All 7 reqwest transports build their streaming client with `build_stream_client` (connect-only, **no total deadline**) by default, honoring an optional operator override `stream_total_timeout` (the exact pattern `openai/direct` + `ollama/direct` already use). The 5 laggards gain a `stream_client` field + that builder and route `stream()` through it. This removes the M13 default cap uniformly.

- Config additions (additive, default `None`): `stream_total_timeout: Option<Duration>` on anthropic `DirectConfig` + `VertexConfig` and google `AIStudioConfig` + `VertexConfig`. (openai + ollama already have it.)

## Testing

- **agent-core (key):** a mock `Provider` whose `stream()` future never resolves + a small `stream_prefill_timeout_ms` → the agent loop terminates with a timeout error within ~budget instead of hanging. A `0` budget leaves it unbounded (documented).
- **transports:** each migrated transport constructs a `stream_client` (no panic) and, with `stream_total_timeout: None`, uses the connect-only builder; with `Some(t)`, honors the override. Assert via the construction path (reqwest doesn't expose the configured timeout for direct inspection, so assert the branch selection).

## Non-goals / follow-ups

- Bedrock (SDK/manual) timeout unification — separate.
- A dedicated agent-core-level *total* stream cap (vs the per-transport opt-in) — not needed once first-byte + idle exist; can follow if operators ask.
