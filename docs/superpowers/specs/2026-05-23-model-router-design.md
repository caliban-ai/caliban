# Model Router — Design

**Date:** 2026-05-23
**Status:** Design pass (not yet committed for implementation)
**Target branch:** `jf/feat/model-router`
**Author:** John Ford
**Sub-project of:** caliban Rust agent harness
**Depends on:** `caliban-provider` (trait + `Capabilities` + `RequestMetadata`)
**Companion ADR:** `adrs/0022-model-routing-architecture.md`

## Goal

Let an operator route distinct categories of provider calls — main
conversational loop, summarization, embeddings, fast classification,
sub-agent loops — to distinct provider+model pairs, declaratively, via
a TOML policy file. The router itself implements the `Provider` trait,
so the `Agent` and every other caller continue to take a single
`Arc<dyn Provider>`; nothing else in the codebase needs to know
routing exists.

The router is also the right place to put cross-provider concerns the
roadmap has been deferring: prompt-cache normalization, cost / usage
aggregation, per-route fallback chains, hedged requests, and circuit
breakers.

## Non-goals

- **Auto-learned routing.** No bandit policy, no "discover the cheapest
  model that still passes". The operator owns the policy, end of story.
- **Provider implementation changes.** Adapter crates
  (`caliban-provider-anthropic`, etc.) stay as they are. The router
  composes them; it doesn't replace them.
- **Dynamic route mutation from inside an agent run.** A route resolved
  at the start of a turn stays in force until that turn ends. Config
  reload at session boundary only (v1).
- **Cross-route session migration tooling.** If you point a resumed
  session at a different route, prompt-cache markers from the previous
  route are inert; the router documents this and moves on. No
  re-warming.
- **Per-tool routing.** Tools that wrap their own LLM calls (e.g.
  `WebFetch`'s optional summarizer) are wired with their own provider
  handle. They CAN pass an `Arc<dyn Provider>` that happens to be the
  router; they don't have to.

## Architecture

```
┌──────────────────────────────────────────────────────┐
│  Agent (caliban-agent-core)                          │
│    provider: Arc<dyn Provider>                       │
└───────────────────┬──────────────────────────────────┘
                    │ CompletionRequest { metadata.purpose, ... }
                    ▼
┌──────────────────────────────────────────────────────┐
│  ModelRouter (caliban-model-router)                  │
│    impl Provider for ModelRouter {                   │
│      complete / stream / capabilities                │
│    }                                                 │
│                                                      │
│    1. Resolve route from purpose + capability needs  │
│    2. Apply prompt-cache normalization               │
│    3. Dispatch to chosen adapter                     │
│    4. Record usage in RouterStats                    │
│    5. On fatal-for-route error, try next in chain    │
└───┬──────────────┬──────────────┬──────────────┬─────┘
    ▼              ▼              ▼              ▼
┌────────┐  ┌────────────┐  ┌────────┐  ┌────────────┐
│anthropic│  │  openai    │  │ gemini │  │  ollama    │
│adapter  │  │  adapter   │  │ adapter│  │  adapter   │
└────────┘  └────────────┘  └────────┘  └────────────┘
```

The router takes a `Vec<RouteEntry>` (parsed from TOML or built via
the builder API) and a `HashMap<ProviderId, Arc<dyn Provider>>`
mapping route entries to live provider handles. The agent sees only
the router.

## Crate structure

New crate `crates/caliban-model-router/`:

```
caliban-model-router/
├── Cargo.toml
└── src/
    ├── lib.rs            # ModelRouter struct + Provider impl
    ├── config.rs         # RouterConfig + TOML deserialize
    ├── resolver.rs       # Route resolution logic
    ├── stats.rs          # RouterStats + accumulator
    ├── fallback.rs       # Fallback-chain driver (v2)
    └── hedging.rs        # Hedged-request driver (v2 stub)
```

Workspace dep on `caliban-provider`. NOT a dep on any specific
adapter — the operator wires those into the `HashMap` at construction.

`Cargo.toml` deps: `caliban-provider`, `tokio` (with the existing
workspace features), `serde`, `toml`, `tracing`, `async-trait`,
`futures`, `arc-swap` (for v2 hot-reload). About what
`caliban-agent-core` already pulls.

## `RouterConfig` schema

TOML, parsed via `serde` into `RouterConfig`:

```toml
# caliban.toml — partial example
[router]
default_purpose = "MainLoop"

[[router.route]]
purpose  = "MainLoop"
provider = "anthropic"
model    = "claude-3-5-sonnet"

[[router.route]]
purpose  = "Summarization"
provider = "anthropic"
model    = "claude-3-5-haiku"

[[router.route]]
purpose  = "FastClassifier"
provider = "ollama"
model    = "llama3.2:3b"

# Fallback chain: same purpose, second entry = first fallback.
[[router.route]]
purpose  = "MainLoop"
provider = "openai"
model    = "gpt-4o"

# Optional per-route capability requirements (declared, not enforced —
# the router enforces them against Provider::capabilities at runtime).
[[router.route]]
purpose  = "MainLoop"
provider = "anthropic"
model    = "claude-3-5-opus"
requires = { vision = true }
```

Rust shape:

```rust
pub struct RouterConfig {
    pub default_purpose: RequestPurpose,
    pub routes: Vec<RouteEntry>,
}

pub struct RouteEntry {
    pub purpose: RequestPurpose,
    pub provider: String,
    pub model: String,
    pub requires: Option<CapabilityRequirements>,
}

pub struct CapabilityRequirements {
    pub vision: Option<bool>,
    pub tool_use: Option<ToolUseCapability>,
    pub thinking: Option<bool>,
    pub min_input_tokens: Option<u32>,
}
```

Builder API for callers who don't want a config file:

```rust
let router = ModelRouter::builder()
    .add_provider("anthropic", anthropic.clone())
    .add_provider("openai", openai.clone())
    .add_provider("ollama", ollama.clone())
    .route(RequestPurpose::MainLoop,       "anthropic", "claude-3-5-sonnet", None)
    .route(RequestPurpose::Summarization,  "anthropic", "claude-3-5-haiku",  None)
    .route(RequestPurpose::FastClassifier, "ollama",    "llama3.2:3b",        None)
    .fallback(RequestPurpose::MainLoop,    "openai",    "gpt-4o",             None)
    .build()?;
```

`build()` validates: every `provider` in a route appears in the
provider map; the map isn't empty; at least one route matches the
configured `default_purpose`.

## Route resolution

`resolver.rs`:

```rust
pub(crate) fn resolve<'a>(
    routes: &'a [RouteEntry],
    request: &CompletionRequest,
    providers: &HashMap<String, Arc<dyn Provider>>,
) -> Result<Vec<&'a RouteEntry>, Error>;
```

Algorithm:

1. Determine the request's purpose:
   `request.metadata.purpose.unwrap_or(config.default_purpose)`.
2. Filter `routes` to entries whose `purpose` matches.
3. For each surviving entry, fetch its provider's
   `capabilities(model)` and check the entry's `requires` block.
   Drop entries whose capabilities don't satisfy the request's
   stated needs.
4. Also enforce **request-derived** capability needs:
   - If `request.tools` is non-empty, the route must support at least
     `ToolUseCapability::Basic`.
   - If the request has any `ImageBlock` in its messages, the route
     must have `capabilities.vision == true`.
   - If `request.thinking.is_some()`, the route must have
     `capabilities.thinking == true`.
5. Return the surviving routes in declaration order. The first is the
   primary; the rest form the fallback chain.

If no route survives: `Error::Misconfigured("no route satisfies
purpose={purpose} with required capabilities")`.

## Fallback semantics

The router's `complete` / `stream` impl tries the primary route first.
On a fatal-for-this-route error, it moves to the next entry in the
chain.

What counts as "fatal for this route":

- `Error::ModelUnavailable` — definitely.
- `Error::RateLimit` — yes, but only AFTER the adapter's own
  `RetryPolicy` has exhausted its retries (the adapter swallows
  transient rate-limits; the router only sees what comes out the
  other end).
- `Error::ContextTooLong` — yes; the next route in the chain may have
  a larger context.
- `Error::ServerError(_)` — yes.
- `Error::Auth` / `Error::InvalidRequest` — **no**. These are
  operator configuration problems; trying the next route just
  multiplies the errors. Surface to the caller.
- `Error::Cancelled` — never fall back; propagate immediately.

The chain is exhausted top-to-bottom. If every route fails, the
router returns the last error it saw. The router tracks fallback hops
in `RouterStats` so the operator can see when their primary is
flaking.

**v1:** synchronous fallback. The primary is fully attempted (including
adapter-level retry) before the secondary is tried.
**v2:** hedged fallback — fire primary and secondary near-
simultaneously, take whichever responds first, cancel the loser.
Sketched below.

## Capability filtering

The router already pulls `Provider::capabilities(model)` once per
route during resolution. It also calls
`Provider::capabilities(model)` for its OWN
`Provider::capabilities(model)` impl — returning the capabilities of
whichever route would resolve for an `unspecified-purpose` request.
This is a small approximation: the agent's `before_turn` capability
check sees the primary route's view, not the fallback's. Acceptable
since the agent uses capabilities mostly for compactor budgeting,
where being off-by-one route is fine.

## Cost / usage aggregation

`stats.rs`:

```rust
pub struct RouterStats {
    inner: Arc<Mutex<RouterStatsInner>>,
}

struct RouterStatsInner {
    per_route: HashMap<(String, String), RouteUsage>,
    fallback_hops: u64,
    route_failures: HashMap<(String, String), u64>,
}

pub struct RouteUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub call_count: u64,
}

impl RouterStats {
    pub fn snapshot(&self) -> RouterStatsSnapshot { ... }
}
```

The router exposes `fn stats(&self) -> RouterStats`. The TUI's
existing `/usage` overlay (ADR 0013) gains a second mode: when the
provider is a `ModelRouter`, render the per-route breakdown alongside
the per-turn `Usage` it already shows.

Implementation detail: every successful `complete` / `stream` call
ends with a `Usage` that the router merges into the right
`(provider, model)` bucket. Errors increment `route_failures`.

## Hedged requests (sketch — v2)

When configured, the router fires the primary AND the first fallback
on the same `tokio::spawn` group, then `tokio::select!`s their results.
The first to respond wins; the loser's `CancellationToken` is
cancelled and its `Drop` aborts the underlying HTTP request.

Trade-off: hedging doubles request-rate and cost for a latency win.
Probably opt-in per-purpose. Defer detailed design; this spec marks
the integration point.

## Circuit-breaker (sketch — v2)

Per-`(provider, model)`: track consecutive failure count. After N
failures (default 5), the route enters `Tripped` state for a cool-off
window (default 30s), during which the resolver skips it. After cool-
off, the route enters `HalfOpen` and is given one probe request; on
success it returns to `Closed`, on failure it stays `Tripped` for
another window.

Standard pattern; the design here is to confirm it belongs in the
router (not in the adapters) so it composes cleanly with the fallback
chain — a tripped primary skips immediately to the fallback rather
than waiting for the failure to materialize again.

## Testing strategy

In `caliban-model-router/src/`:

1. **Resolver unit tests** (`resolver.rs`):
   - `resolves_first_matching_purpose`
   - `produces_ordered_fallback_chain_for_repeated_purpose`
   - `filters_by_declared_requires`
   - `filters_by_request_derived_tool_use_need`
   - `filters_by_request_derived_vision_need`
   - `returns_misconfigured_when_no_route_survives`
2. **Fallback driver tests** (`fallback.rs`), each backed by
   `MockProvider`:
   - `falls_back_on_rate_limit`
   - `falls_back_on_model_unavailable`
   - `does_not_fall_back_on_auth_error`
   - `propagates_cancelled_immediately`
   - `exhausted_chain_returns_last_error`
   - `increments_fallback_hops_stat`
3. **Stats accumulator tests** (`stats.rs`):
   - `merges_usage_per_route_key`
   - `counts_failures_per_route_key`
   - `snapshot_is_consistent_under_concurrent_writes`
4. **End-to-end `Provider` impl tests** (`lib.rs`):
   - `router_as_provider_dispatches_streaming_to_correct_adapter`
   - `router_as_provider_normalizes_cache_usage_across_providers`
   - `capabilities_returns_primary_route_view`
5. **TOML round-trip** (`config.rs`):
   - `parses_minimal_config`
   - `parses_full_config_with_requires_blocks`
   - `rejects_dangling_provider_reference`

Target ~20 new tests. The router is small but does enough that the
tests pay for themselves.

## Open questions

- **`before_turn` route visibility.** The agent's `TurnCtx` exposes
  `config.model`, which is the request, not the resolved route.
  Proposal: add an optional `resolved_route: Option<(String, String)>`
  field, populated by the router via a side-channel before the
  provider call. Decide: side-channel (thread-local? Arc<Mutex>?) or a
  new hook the router invokes? Likely a new
  `Hooks::after_route_resolve(&RouteResolved)` hook to keep the
  contract explicit.
- **Prompt-cache across route changes mid-session.** When a session
  resumes on a different route, what happens to cache_control markers
  in the persisted messages? Proposal: the router drops them (replaces
  with `None`) when it detects a route change vs. the session's
  recorded primary. Document; verify with a wiremock integration test
  for the transition turn.
- **Retry / fallback overlap.** Adapter-level `RetryPolicy` retries
  transient errors WITHIN a route; router fallback moves BETWEEN
  routes. Overlap point: a rate-limit that the adapter retried, gave
  up on, and bubbled up — the router then tries the fallback. Is
  that the right behavior? Yes, but document it explicitly so
  operators don't think "rate-limit on primary always means fall
  back" — there's the adapter retry budget in between.
- **`RouterStats` exposure surface.** Today the agent doesn't carry
  any provider-specific debug surface to the TUI. We'll need a small
  trait — `ProviderDebug` or similar — that exposes
  `as_router_stats() -> Option<RouterStatsSnapshot>` so the TUI can
  detect "this provider is a router" without a downcast. Or, just
  downcast; it's pragmatic and the TUI already knows the
  construction shape. Lean toward downcast for v1.
- **Sub-agent purpose default.** Sub-agents will start to need their
  own purpose tag. We're sketching `RequestPurpose::SubAgent` — but
  there's only ever one sub-agent purpose, which feels wrong for a
  multi-agent system. Proposal: `SubAgent(SubAgentRole)` where
  `SubAgentRole` is an open enum the operator extends via the same
  TOML. Defer until the sub-agent primitive lands (ADR 0021).

## Acceptance criteria

- `crates/caliban-model-router/` exists with `Cargo.toml`, `lib.rs`,
  and the modules listed above.
- Public surface: `ModelRouter`, `ModelRouterBuilder`,
  `RouterConfig`, `RouterStats`, `RouteEntry`, `RequestPurpose`.
- `ModelRouter` implements `Provider` end-to-end (complete + stream +
  capabilities).
- `RequestMetadata` (in `caliban-provider`) gains a `purpose:
  Option<RequestPurpose>` field with `#[serde(default)]` so existing
  callers keep working.
- `caliban` binary gains a `--router-config <PATH>` flag and a
  `CALIBAN_ROUTER_CONFIG` env var. When set, the binary builds a
  `ModelRouter` and passes it to `AgentBuilder::provider`; when
  unset, the binary falls back to the existing single-provider
  construction.
- `cargo build --workspace` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean; `cargo test --workspace`
  passes with ≥ 20 new tests in `caliban-model-router`.
- TUI's `/usage` overlay shows per-route breakdown when the active
  provider is a `ModelRouter` (via downcast for v1).
- ADR 0022 is committed alongside this implementation.
- v2 features (hedged requests, circuit-breakers, async config
  reload) are NOT required for the v1 acceptance — they live on
  the same branch family but get their own follow-on commits.
