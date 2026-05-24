# ADR 0038 · Model router v2 — fallback, hedging, breakers, capabilities, binary wiring

- **Status:** proposed
- **Date:** 2026-05-24
- **Spec:** `docs/superpowers/specs/2026-05-24-model-router-v2-design.md`
- **Supersedes scope of:** ADR 0022 deferred items
- **Author:** john.ford2002@gmail.com

## Context

ADR 0022 + PR #12 shipped the model router as a config-driven dispatcher:
TOML schema, builder API, purpose-keyed routes, `impl Provider`, per-
route usage tracking. Five capabilities were deferred to v2 because the
v1 surface needed to settle before resilience landed on top:

- **Fallback chains** — try the next route on a fatal-for-route error.
- **Hedged requests** — race a second route after a delay; first wins.
- **Circuit breakers** — skip a failing route for a cool-off window.
- **Capability-based pre-routing** — auto-route requests that need
  vision/thinking/parallel-tools to capable models even if the operator
  put a non-capable route first.
- **`caliban.toml` discovery + binary wiring** — the CLI does not yet
  construct a `ModelRouter` from `[router]`; it falls back to single-
  provider construction.

Plus the smaller `effort` and per-route prompt-cache normalization
follow-ups. Closing all six in one ADR keeps the router's contract
coherent — fallback, hedging, and breakers all consume the same
candidate-vec from resolution, and capability filtering changes which
candidates appear in the first place.

## Decision

### Resolution and dispatch are separated, with a candidate vec as the seam

`resolve_candidates(...) -> Vec<&RouteEntry>` is the single funnel into
the dispatch driver. Filters apply in order: purpose → declared
`requires` → request-derived needs → breaker state → explicit fallback
re-ordering. Dispatch (`fallback` or `hedging`) consumes the vec
identically. This means the same diagnostic (`/router debug`) shows the
exact list every dispatch will see, and new filters (e.g. cost-budget)
slot in without touching dispatch.

### Fallback is sequential by default; hedging is opt-in per route

Sequential fallback handles the cost-conscious common case: try the
primary, only spend on the secondary on real failure. Hedging is a
spend-for-latency knob the operator opts into per route via
`hedge = { hedge_after_ms = N, max = K }`. We pick this default because
hedging silently doubles the bill for the median request; making it opt-
in keeps the surprise floor low.

### Fatal-for-route is a closed list

`ModelUnavailable`, `RateLimit` (post adapter-retry), `ContextTooLong`,
`ServerError`, `NetworkTimeout` → fall back. Everything else
(`Auth`, `InvalidRequest`, `ContentPolicy`, `Cancelled`) propagates.
The list lives in code (`fallback.rs::is_fatal_for_route`); tests pin
the membership.

### Circuit breaker is per-route id, not per `(provider, model)`

The breaker's state lives in `BreakerRegistry: HashMap<RouteId,
ArcSwap<BreakerState>>`. We key on the route id (which defaults to
`{provider}:{model}:{purpose}`) so the operator can break a provider on
one purpose without disabling it on another. `Closed → Tripped →
HalfOpen → Closed/Tripped` is the standard SRE breaker. `Cancelled`
outcomes do not count toward failure.

### Capability filtering is pre-routing, not post-failure

Today the router relies on `requires` blocks to drop incompatible
routes. v2 adds *request-derived* needs (image content → vision; thinking
budget → thinking capability) so the operator does not need to mark
every route explicitly. This costs one `Provider::capabilities(model)`
call per candidate (already a HashMap lookup in the adapters); we accept
the cost because the diagnostic value is large.

### `caliban.toml` discovery uses the CLAUDE.md walk algorithm

Same ancestor-walk-up-to-git-root-or-`$HOME` as memory tier 0018, with a
different filename predicate. Both walks share a `caliban-memory::walk_up`
utility (already small, factored out for this ADR). Layering: CLI flag >
env var > `caliban.toml` > `$HOME/.config/caliban/caliban.toml`. Unknown
providers fail loudly at startup, not lazily on first call.

### Effort levels live on `RequestMetadata` and map per-adapter

`RequestMetadata.effort: Option<EffortLevel>` is plumbed through to each
adapter. Each adapter owns the mapping to its native effort knob
(`reasoning_effort` / `extended_thinking.budget` / `thinkingConfig`).
Ollama's mapping is a no-op for now. Operators see the table via
`caliban router debug --effort-table`.

### Prompt-cache markers are cleared on cross-route hops

When fallback or hedging moves to a different provider mid-session,
`cache_control` markers in the persisted messages are stripped before
the new adapter sees them. The cleared count is recorded in
`router.cache.markers_cleared`. This is the cheap, safe behavior;
markers are normalization-cost, not correctness-cost.

### Metrics are `tracing` first, OTel-export later

We emit `tracing` events with structured fields (`route_id`, `purpose`,
`kind`, `from`, `to`); the OTel cost spec (out of scope for this ADR)
maps them to OTLP metric streams. Keeping the in-router emission
tracing-only avoids pulling `opentelemetry` into a Layer-3 crate.

## Consequences

- **Positive.** Closes six 🔴 rows under matrix I in one PR — fallback,
  hedging, breakers, capability filtering, `caliban.toml` wiring,
  effort levels. The router now earns its keep as a resilience layer:
  a flaky primary auto-routes to a secondary, a tripped breaker
  prevents cascade failure, hedging gives operators an explicit
  spend/latency knob. The binary actually constructs a router from
  config, removing the awkward "config exists but unwired" state.
- **Negative.** Hedging spend can surprise operators who do not read
  the README. We mitigate with explicit-opt-in and loud per-route
  `hedge_loss` metrics, but it remains a footgun. Breaker false
  positives are real and the cool-off window is fixed (no exponential
  back-off in v2). Capability auto-routing changes which route a
  request lands on without the operator's `purpose` knob; this can be
  debugged via `/router debug` but is a behavior change v1 users may
  not expect — release notes call it out. Prompt-cache marker
  clearing means cross-route hops lose Anthropic cache savings; with
  hedging this happens silently on every hedge to a non-Anthropic
  fallback.
- **Revisit if:** Operator demand for adaptive hedge tuning (EWMA, p95
  observation) materializes — a v3 sketch already lives in the spec's
  non-goals. If breaker false-positive complaints recur, add
  exponential cool-off (cooldown_secs * 2^trip_count up to a cap). If
  the candidate-vec seam ossifies and we need cost/budget routing,
  introduce a `Budget` filter stage *before* dispatch rather than
  rewriting dispatch.
