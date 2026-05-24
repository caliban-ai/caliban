# ADR 0022 · Model routing architecture

- **Status:** accepted
- **Date:** 2026-05-23

## Context

The agent makes provider calls for several distinct purposes — the main
conversational loop, summarization for compaction, embeddings for memory,
fast classification for routing decisions, sub-agent loops, etc. Today
those all run through the single `Arc<dyn Provider>` handed to the
`Agent`. Operators who want to use Sonnet for the main loop, Haiku for
summarization, and a local Ollama model for fast classification have no
clean way to express that.

Claude Code solves this with hardcoded `getMainLoopModel` /
`getSmallFastModel` helpers. That's fine for a single-vendor harness;
it's wrong for caliban, which is provider-agnostic by design. Operators
should be able to compose any model from any provider for any purpose
without recompiling.

A model router also turns out to be the natural home for several
already-deferred concerns: per-route fallback chains, hedged requests,
circuit breakers, cost/usage aggregation, and unification of the
divergent prompt-cache surfaces across Anthropic, OpenAI, and Gemini.

This is signature differentiation for caliban; it deserves its own
layer.

## Decision

- **Add a new Layer-3 crate `caliban-model-router`.** It sits between
  `caliban-agent-core` and the four `caliban-provider-*` adapter
  crates. No agent-core code changes shape; the agent continues to
  take a single `Arc<dyn Provider>`.
- **The router IS a `Provider`.** It implements the same trait the
  adapters implement, so the agent sees one provider — the router —
  and the router internally dispatches each `complete` / `stream`
  call to the right downstream `Provider` + model based on the
  request's purpose, the operator's policy, and the capabilities the
  request needs.
- **Routes are matched by `RequestMetadata.purpose`.** A new field on
  the existing `RequestMetadata` struct:
  `purpose: Option<RequestPurpose>` with variants
  `MainLoop | Summarization | Embedding | FastClassifier | SubAgent | Custom(String)`.
  Callers that don't set a purpose route through a default
  configured by the operator (likely `MainLoop`).
- **Routing policy is operator-defined.** A TOML config file plus a
  builder API. No auto-learning, no automatic cost optimization, no
  hidden behavior. The operator owns the cost / latency / capability
  trade-offs explicitly. This is a deliberate differentiator from
  Claude Code's hardcoded paths.
- **Capability filtering is mandatory.** Each route declares its
  provider + model; the router consults `Provider::capabilities(model)`
  before dispatch and skips a route whose capabilities don't satisfy
  the request (e.g. request needs `ToolUseCapability::ParallelCalls`
  but the route's model only supports `Basic`).
- **Per-route fallback is opt-in and ordered.** When the same
  `purpose` appears in multiple `[[route]]` entries, the entries form
  a fallback chain in declaration order. The router tries them in
  sequence on a retryable failure of the previous entry (rate-limit,
  model unavailable, transient network error). Implementation is
  deferred to v2 — this ADR commits to the design.
- **Cost / usage aggregation is a router responsibility.** The router
  sees every call and every `Usage`. It maintains a per-`(provider,
  model)` accumulator and exposes a `RouterStats` snapshot for the
  TUI's existing `/usage` overlay (ADR 0013) to render.
- **Hedging and circuit-breakers are router responsibilities.** Both
  are sketched in the design spec but deferred to v2.

## Consequences

- **Agent constructor unchanged.** `AgentBuilder::provider(...)` takes
  the router as its `Arc<dyn Provider>` exactly like any adapter. No
  code in `caliban-agent-core` knows the router exists.
- **Adapters stay simple.** Per-adapter retry policy (existing
  `RetryPolicy` for transient errors) remains in the adapter. The
  router handles route-level fallback. The two layers compose:
  adapter retries within a route; router moves to the next route only
  if the adapter exhausts its retries with a fatal-for-this-route
  error.
- **Prompt-cache unification lands here.** Anthropic's
  `cache_control` markers, OpenAI's `cache_read_input_tokens`, and
  Gemini's context-caching all surface as the same
  `Usage.cache_read_input_tokens` / `cache_creation_input_tokens`
  values once they reach the router; the router is the natural place
  to normalize the bookkeeping.
- **`before_turn` hook needs a way to see the resolved route.** The
  agent's `TurnCtx` currently exposes `config.model`, which is the
  caller's request, not the route's actual choice. A new optional
  field (or a router-supplied hook surface) is required so the TUI
  status line can display "Sonnet via Anthropic, fallback gpt-4o"
  instead of just the requested logical name. Detailed in the spec.
- **Sessions become route-history-aware.** If a session was started
  on route A and resumes on route B (because the config changed, or
  the primary route is unavailable), prompt-cache markers from the
  prior provider are inert. The router documents this and falls back
  to no-cache for the transition turn.
- **Forward links:** hedged requests, circuit breakers, and adaptive
  retry budgets were listed as non-goals in
  `2026-05-23-perf-baseline-design.md`. This ADR pulls them under the
  router's umbrella for v2.
- **Revisit if:** the operator-defined policy turns out to be a
  meaningful UX burden in practice (consider a "balanced" default
  policy), or if hedged requests prove valuable enough to promote
  from v2 to v1.

## References

- Design spec: `docs/superpowers/specs/2026-05-23-model-router-design.md`
- Provider trait: `crates/caliban-provider/src/lib.rs`
- Capabilities: `crates/caliban-provider/src/capabilities.rs`
- Per-adapter retry: ADR 0009 (RetryPolicy)
- Usage overlay: ADR 0013 (TUI overlays)
- Perf-baseline non-goals: `docs/superpowers/specs/2026-05-23-perf-baseline-design.md`
