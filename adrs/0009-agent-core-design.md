# ADR 0009 · Agent-core design (stream-as-primitive, sequential tools, opt-in compaction)

- **Status:** accepted
- **Date:** 2026-05-23

## Context

Layer 1 / C adds the agent loop. Three design dimensions had real
trade-offs: where the streaming surface lives, whether tool calls in
one response are dispatched concurrently or sequentially, and what
the default compaction strategy is.

## Decision

- **`stream_until_done` is the single source of truth.** Non-streaming
  `run_turn` and `run_until_done` are thin consumers of the stream.
  This means the streaming code path is always exercised; bugs surface
  through unit + integration tests of either surface.
- **Tool calls are dispatched sequentially within a single turn.**
  Anthropic and Gemini can emit multiple `tool_use` blocks in one
  response; we run them in the order received. Parallelism is a
  follow-on (Hooks-pluggable dispatch strategy).
- **Default compactor is `NoopCompactor`.** Compaction strategies
  (`DropOldest`, `Summarizing`) are explicit opt-ins. The library
  doesn't silently mutate the user's message history; callers decide.
- **Retries only on the provider call.** Tool failures don't retry —
  tools manage their own retry semantics. Retryable provider errors:
  `RateLimit`, `Network`, `ServerError 502-599`. NOT retryable:
  `Auth`, `InvalidRequest`, `ContextTooLong`, `ContentFilter`,
  `Cancelled`, `Adapter`, `ModelUnavailable`, `ServerError 500`.

## Consequences

- **Positive:** Single source of truth → simpler correctness story.
  Sequential tool dispatch → predictable behavior, easier debugging.
  Opt-in compaction → no surprise history mutation. Retry policy
  classifier is conservative and stable.
- **Negative:** Sequential dispatch is slower than parallel for
  independent tools. Token-counting heuristic (chars/4) is approximate.
- **Revisit if:** Real workloads show sequential dispatch as a
  bottleneck (add parallel strategy); a non-English language is
  consistently mis-estimated (integrate a tokenizer crate).
