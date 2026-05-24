# Model router v2 — Design (fallback, hedging, breakers, caps, binary wiring)

**Date:** 2026-05-24
**Status:** Proposed
**Author:** john.ford2002@gmail.com
**Sub-project of:** caliban Rust agent harness
**Companion ADR:** `adrs/0038-model-router-v2.md`
**Supersedes scope of:** v1 deferred follow-ups in
`adrs/0022-model-routing-architecture.md` and
`docs/superpowers/specs/2026-05-23-model-router-design.md`.

## Goal

Close the v2 follow-up scope from PR #12 so the model router stops being
"a fancy single-route dispatcher" and starts being the resilience layer
the agent always wanted. Specifically:

1. **Fallback chains.** When the primary route fails with a fatal-for-
   route error, transparently try the next route declared for the same
   purpose (or named in an explicit `fallback` list).
2. **Hedged requests.** After a configurable delay, race a second
   request against an in-flight primary; first response wins, loser is
   cancelled.
3. **Circuit breakers.** Per-route failure-rate threshold trips the
   breaker; cool-off + half-open probe gates re-entry.
4. **Capability-based filtering.** A request with vision content / a
   thinking budget / parallel tool-use is auto-routed to a capable
   model regardless of declaration order.
5. **Per-route prompt-cache normalization.** The Anthropic
   `cache_control`, OpenAI `cache_read_input_tokens`, and Gemini
   context-caching surfaces fold into a uniform `Usage` view at the
   router boundary.
6. **`caliban.toml` discovery + binary wiring.** The CLI actually
   constructs a `ModelRouter` from the `[router]` section instead of
   today's single-provider fallback.

## Non-goals

- **Auto-learned routing.** Same call as v1 ADR 0022: operator owns the
  policy.
- **Adaptive hedge tuning.** v2 honors a fixed `hedge_after` delay; an
  EWMA-driven adaptive hedge is not in scope.
- **Cross-process router state.** Circuit breakers and stats live in
  the current `ModelRouter` instance. A new process starts with all
  breakers closed and zero stats. Shared state across binaries is out
  of scope (would gate on a fleet-store ADR).
- **Per-tool routing.** A tool that wraps its own provider call still
  passes whatever provider it was constructed with; the router does
  not introspect into tool internals.
- **Mid-turn route switching.** If a route is in-flight when its
  breaker trips, the in-flight request completes (or times out) on
  the original route. Fallback applies on the *next* request.
- **`prompt_cache` rewriting.** v2 *records* normalized cache usage; it
  does not rewrite `cache_control` markers between providers
  mid-session. See v1 spec's "cross-route session migration" non-goal.

## Architecture

```
                      CompletionRequest { metadata.purpose, tools, … }
                                       │
                                       ▼
┌───────────────────────────────────────────────────────────────────────┐
│  ModelRouter (caliban-model-router)  impl Provider                    │
│                                                                       │
│   resolve()                                                           │
│      ├── filter by purpose                                            │
│      ├── filter by declared `requires`                                │
│      ├── filter by request-derived capability needs                   │
│      ├── skip routes whose breaker is Tripped                         │
│      └── return ordered candidate Vec<&RouteEntry>                    │
│                                                                       │
│   dispatch(candidates)                                                │
│      ├── HedgePolicy::None  → sequential fallback                     │
│      └── HedgePolicy::Race  → tokio::select! after hedge_after        │
│                                                                       │
│   record(route, outcome)                                              │
│      ├── stats.merge(usage)                                           │
│      ├── breaker.observe(success|failure)                             │
│      └── fire `router.route.*` metrics                                │
└───────────────────────────────────────────────────────────────────────┘
                     │            │             │             │
                     ▼            ▼             ▼             ▼
                  anthropic    openai        gemini        ollama
                  adapter      adapter       adapter       adapter
```

Resolution and dispatch are now separate methods. The candidate vec is
the same shape that fallback / hedging / breakers all consume.

## Crate structure (delta from v1)

```
crates/caliban-model-router/
├── Cargo.toml                # add: futures, tokio (timeout), arc-swap, metrics
└── src/
    ├── lib.rs                # ModelRouter + Provider impl (rewritten dispatch)
    ├── config.rs             # extend RouteEntry + RouterConfig
    ├── resolver.rs           # NEW: candidate filter pipeline
    ├── fallback.rs           # NEW: sequential driver
    ├── hedging.rs            # NEW: hedge_after race driver
    ├── breaker.rs            # NEW: CircuitBreaker state machine
    ├── capabilities.rs       # NEW: derive needs from request
    ├── cache.rs              # NEW: prompt-cache normalization
    ├── stats.rs              # extend RouteUsage + breaker counters
    ├── metrics.rs            # NEW: tracing + OTel-counter shims
    └── error.rs              # add: FallbackExhausted, BreakerOpen, NoCandidate
```

### Cargo deltas

```toml
[dependencies]
futures   = { workspace = true }
tokio     = { workspace = true, features = ["time", "macros", "sync"] }
arc-swap  = "1"                  # for in-place breaker state updates
tracing   = { workspace = true }
```

No new heavy deps; the breaker and hedge logic is small enough to live in
the crate.

## Config schema (extended)

```toml
# caliban.toml — full router example
[router]
default_purpose = "main_loop"

# Optional global breaker defaults — per-route can override.
[router.breaker]
failure_threshold = 5            # consecutive failures (or N within window)
window_secs       = 60           # rolling failure window
cooldown_secs     = 30           # how long Tripped stays Tripped
half_open_probes  = 1            # successes needed to close after cool-off

# Optional global hedge defaults — per-route opts in explicitly.
[router.hedge]
hedge_after_ms = 1000
max_hedges     = 1

# --- main loop, with fallback chain + hedging ---
[[router.route]]
purpose  = "main_loop"
provider = "anthropic"
model    = "claude-sonnet-4-7"
requires = { vision = true, tool_use = "parallel_calls" }
fallback = ["main-openai", "main-bedrock"]   # named-route fallbacks (see below)
hedge    = { hedge_after_ms = 800, max = 1 }

# Named alternates — referenced by `fallback = [...]` above.
[[router.route]]
id       = "main-openai"
purpose  = "main_loop"
provider = "openai"
model    = "gpt-5-omni"
requires = { vision = true, tool_use = "parallel_calls" }

[[router.route]]
id       = "main-bedrock"
purpose  = "main_loop"
provider = "anthropic-bedrock"
model    = "anthropic.claude-sonnet-4:0"

# --- fast classifier with a local-first preference ---
[[router.route]]
purpose  = "fast_classifier"
provider = "ollama"
model    = "llama3.2:3b"
fallback = []                                # explicit: no fallback wanted
breaker  = { failure_threshold = 3, cooldown_secs = 10 }

# --- summarization (no special policy) ---
[[router.route]]
purpose  = "summarization"
provider = "anthropic"
model    = "claude-haiku-4-5"
```

### Field semantics (delta from v1)

| Field        | Type                          | Default            | Notes                                                              |
| ------------ | ----------------------------- | ------------------ | ------------------------------------------------------------------ |
| `id`         | string                        | derived            | Stable name used by `fallback = [...]` and metrics. Defaults to `{provider}:{model}:{purpose}` when omitted. |
| `requires`   | inline table                  | `{}`               | `vision: bool`, `tool_use: "basic"|"parallel_calls"`, `thinking: bool`, `min_input_tokens: u32`, `effort: "low"|"medium"|"high"` |
| `fallback`   | array of route ids            | implicit (declaration order, same purpose) | When set, *only* listed routes fall back, in order. Empty array disables fallback. |
| `hedge`      | inline table                  | inherit `[router.hedge]` | Per-route override. `hedge = false` disables.                  |
| `breaker`    | inline table                  | inherit `[router.breaker]` | Per-route override. `breaker = false` disables.            |
| `effort`     | `"low"|"medium"|"high"`       | `"medium"`         | Surfaces on `RequestMetadata.effort` for downstream adapters that map effort to `extended_thinking` budget / temperature / max_output_tokens. |

Implicit fallback chain (when `fallback` is unset on the primary) remains
v1's "all entries with the same purpose, in declaration order". Explicit
`fallback = [...]` overrides.

```rust
pub struct RouteEntry {
    pub id: Option<String>,
    pub purpose: RequestPurpose,
    pub provider: String,
    pub model: String,
    pub requires: Option<CapabilityRequirements>,
    pub fallback: Option<Vec<String>>,
    pub hedge: Option<HedgePolicy>,
    pub breaker: Option<BreakerPolicy>,
    pub effort: Option<EffortLevel>,
}

pub enum HedgePolicy {
    Disabled,
    Race { hedge_after: Duration, max_hedges: u8 },
}

pub struct BreakerPolicy {
    pub failure_threshold: u32,
    pub window: Duration,
    pub cooldown: Duration,
    pub half_open_probes: u32,
}

pub enum EffortLevel { Low, Medium, High }
```

## Resolution (extended)

```rust
pub(crate) fn resolve_candidates<'a>(
    routes: &'a [RouteEntry],
    request: &CompletionRequest,
    providers: &HashMap<String, Arc<dyn Provider>>,
    breakers: &BreakerRegistry,
    cfg: &RouterConfig,
) -> Result<Vec<&'a RouteEntry>, RouterError>;
```

Pipeline:

1. **Purpose filter** — `request.metadata.purpose.unwrap_or(default_purpose)`
   selects same-purpose routes.
2. **Declared `requires`** — route's `requires` must be satisfied by the
   provider's `capabilities(model)` result. Drop on miss.
3. **Request-derived capability needs** — scan the request:
   - any `ImageBlock` content → require `capabilities.vision`.
   - any `tools` non-empty → require at least `ToolUseCapability::Basic`.
   - `tools.parallel_calls_allowed && route.requires.tool_use ==
     Some(ParallelCalls)` → require `ToolUseCapability::ParallelCalls`.
   - `request.metadata.thinking_budget.is_some()` →
     require `capabilities.thinking`.
4. **Breaker filter** — drop routes whose breaker is in `Tripped` state.
   Routes in `HalfOpen` survive; their dispatch is rate-limited (one
   probe at a time).
5. **Explicit `fallback` ordering** — if the primary has
   `fallback = ["b", "c"]`, the result is `[primary, b, c]` regardless
   of declaration order; if `fallback = []`, the result is `[primary]`
   only; otherwise the result is `[primary, all_other_same_purpose...]`
   in declaration order.

If the candidate set is empty: `Err(RouterError::NoCandidate { purpose,
needs })`. The TUI surfaces this with a route-debug overlay (see
"Diagnostics" below).

## Fallback semantics

Sequential by default. The router tries `candidates[0]` and on fatal-for-
route error advances to `candidates[1]`, etc.

**Fatal-for-route:**

- `Error::ModelUnavailable`
- `Error::RateLimit` — only *after* the adapter's own `RetryPolicy` has
  exhausted retries.
- `Error::ContextTooLong` — next route may have a larger context window.
- `Error::ServerError(_)` (5xx).
- `Error::NetworkTimeout` (`tokio::time::timeout` elapsed).

**Never-fall-back:**

- `Error::Auth` — operator configuration.
- `Error::InvalidRequest` — schema problem.
- `Error::Cancelled` — propagate.
- `Error::ContentPolicy` — user content; same content will fail
  elsewhere.

Each fatal-for-route hop increments `router.route.fallback_engaged`. If
every candidate fails: `Err(RouterError::FallbackExhausted { tried: …,
last_error: … })`.

## Hedged requests

When the resolved candidate set has at least two entries and the primary's
`HedgePolicy::Race { hedge_after, max_hedges }` is configured, the
dispatch path is:

```rust
async fn dispatch_hedged(cands: &[&RouteEntry], req: &CompletionRequest)
    -> Result<CompletionResponse, RouterError> {
    let (tx, mut rx) = mpsc::channel(cands.len());
    let tokens: Vec<CancellationToken> = (0..cands.len()).map(|_| CancellationToken::new()).collect();
    let mut launched = 1usize;
    spawn_one(&cands[0], req, &tokens[0], tx.clone(), 0);

    let mut hedge_timer = sleep(hedge_after);
    loop {
        tokio::select! {
            biased;
            outcome = rx.recv() => return finalize(outcome, &tokens, /* winner */),
            _ = &mut hedge_timer, if launched <= max_hedges as usize && launched < cands.len() => {
                spawn_one(&cands[launched], req, &tokens[launched], tx.clone(), launched);
                hedge_timer = sleep(hedge_after);
                launched += 1;
            }
        }
    }
}
```

The first successful response wins; all other tokens are cancelled
(`tokens[i].cancel()`); adapter futures observe cancellation through the
provider trait's existing `CancellationToken` plumbing.

**Cost contract:** every hedge launched bills the provider it was sent
to. The `router.route.hedge_won` and `router.route.hedge_loss` metrics
let operators see how much they're spending on hedging vs. how much
latency they're buying.

If the primary errors *before* the hedge fires, the hedge is launched
immediately (which is just sequential fallback). If the hedge errors and
the primary still hasn't responded, we wait on the primary; if both
error, fallback continues with `candidates[2..]`.

`hedge_after` is fixed per route. v2 ships no adaptive heuristic.

## Circuit breakers

State machine per route id:

```
            ┌─────────────────┐
            │     Closed      │  ◄────── success in HalfOpen
            └────┬────────────┘
   N failures in │
   `window`      ▼
            ┌─────────────────┐
            │     Tripped     │
            └────┬────────────┘
   cooldown      │
   elapsed       ▼
            ┌─────────────────┐
            │    HalfOpen     │ ─── failure ──► Tripped
            └────┬────────────┘
   `half_open_probes` successes
                 │
                 ▼
             Closed
```

Implementation: per-route `Arc<ArcSwap<BreakerState>>`. `observe()`
takes a `&self` (lock-free) and CAS-swaps the next state. Failure window
is a small ring buffer (`Vec<Instant>`) under a `Mutex`; window
membership is recomputed on each observation. Window-eviction is lazy.

In `Tripped`, the route is skipped by the resolver. The first call after
`cooldown` elapses advances the route to `HalfOpen` and lets exactly one
probe through; subsequent concurrent calls wait or fall back (gated by a
`tokio::sync::Semaphore::new(half_open_probes)`).

A `Cancelled` outcome does NOT count toward the failure threshold (it's a
caller decision). `RateLimit` after adapter retries DOES count (it's a
provider-side signal).

## Capability filtering

Already partially in v1; v2 makes it pre-routing:

```rust
pub fn derived_needs(req: &CompletionRequest) -> CapabilityRequirements {
    CapabilityRequirements {
        vision:           Some(req.messages.iter().any(has_image_block)),
        tool_use:         req.tools.is_empty().not().then(|| {
            if req.allow_parallel { ToolUseCapability::ParallelCalls }
            else { ToolUseCapability::Basic }
        }),
        thinking:         req.metadata.thinking_budget.is_some().then_some(true),
        min_input_tokens: Some(estimate_input_tokens(req)),
        effort:           req.metadata.effort,
    }
}
```

A request with image + parallel tools auto-routes to the first declared
route that satisfies both, even if the operator put a non-vision Haiku
route first. Operators see the auto-route via `router.route.selected`
trace events; they can override by setting an explicit `purpose`.

If `min_input_tokens > route's documented context window minus 8k`, the
route is dropped. The 8k headroom is for output + tool-result tokens.

## Per-route prompt-cache normalization

Anthropic returns `usage.cache_read_input_tokens` + `cache_creation_input_tokens`.
OpenAI returns `usage.prompt_tokens_details.cached_tokens` (one number).
Gemini returns `usage_metadata.cached_content_token_count`.

The router merges these into a uniform `RouteUsage`:

```rust
pub struct RouteUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,       // unified
    pub cache_creation_input_tokens: u64,   // Anthropic-only today; 0 elsewhere
    pub call_count: u64,
}
```

When a request crosses route boundaries (resumed session, fallback hop),
`cache_control` markers in the persisted messages are *cleared* before
the second route sees them, so a non-Anthropic adapter doesn't choke on
a foreign marker. The cleared count is recorded in the `router.cache.
markers_cleared` counter.

This normalization runs as a final stage in `record(route, outcome)`; the
`Usage` returned to the caller is the route-native one — only the
router's aggregate `RouterStats` carries the normalized view.

## `caliban.toml` discovery + binary wiring

The binary picks up router config from (in order):

1. `--router-config <PATH>` CLI flag (v1, retained).
2. `CALIBAN_ROUTER_CONFIG` env var (v1, retained).
3. `caliban.toml` at the current working directory, walking up to the
   nearest git root or `$HOME` — same algorithm as CLAUDE.md lookup
   (ADR 0018).
4. `~/.config/caliban/caliban.toml`.

If no config is found, the binary falls back to the existing single-
provider construction (today's default).

Construction:

```rust
let router_cfg = caliban_cli::config::load_caliban_toml(&cwd)?;
let providers: HashMap<String, Arc<dyn Provider>> =
    build_provider_handles(&router_cfg, env_vars)?;
let router = ModelRouter::from_config(router_cfg, providers)?;
let agent = AgentBuilder::default().provider(router).build()?;
```

`build_provider_handles` enumerates the provider strings referenced by
`router.routes[].provider` and constructs each — `anthropic` → `AnthropicProvider::from_env()`,
`openai` → `OpenAiProvider::from_env()`, etc. Unknown provider strings
fail loudly at startup with `RouterError::UnknownProvider { name }`.

`[provider.X]` blocks in `caliban.toml` (optional) override env-based
construction, e.g.:

```toml
[provider.openai]
api_key_env = "OPENAI_API_KEY_DEV"
base_url    = "https://oai.example.test"

[provider.ollama]
base_url = "http://localhost:11434"
```

## Metrics

Emitted via `tracing` (already pulled in `caliban-core`); OpenTelemetry
export is composed by the upcoming OTel cost spec, which sees these as
its base inputs.

| Metric                                  | Type      | Labels                       |
| --------------------------------------- | --------- | ---------------------------- |
| `router.route.success`                  | counter   | `route_id`, `purpose`        |
| `router.route.failure`                  | counter   | `route_id`, `purpose`, `kind`|
| `router.route.fallback_engaged`         | counter   | `from`, `to`                 |
| `router.route.selected`                 | counter   | `route_id`, `via` (purpose/capability) |
| `router.route.breaker_state`            | gauge     | `route_id`, `state`          |
| `router.route.hedge_won`                | counter   | `route_id`                   |
| `router.route.hedge_loss`               | counter   | `route_id`                   |
| `router.cache.markers_cleared`          | counter   | `from`, `to`                 |
| `router.route.latency_ms`               | histogram | `route_id`                   |

The TUI `/usage` overlay reads the same `RouterStats` snapshot it does
today, plus a new `breakers: HashMap<RouteId, BreakerSnapshot>` field.

## Diagnostics

`/router debug` (TUI slash) and `caliban router debug` (CLI) print the
last resolved candidate list for the current session, with reasons each
candidate was kept or dropped — invaluable when "why did it route to
gpt-5 instead of Sonnet?" comes up.

```
$ caliban router debug
purpose=MainLoop  needs={vision:true, tool_use:parallel, thinking:false}
  ✓ anthropic:claude-sonnet-4-7        [primary]
  ✓ openai:gpt-5-omni                  [fallback via id="main-openai"]
  ✗ anthropic:claude-haiku-4-5         [requires.vision unsatisfied]
  ✗ ollama:llama3.2:3b                 [breaker tripped 18s ago, cooldown 30s]
```

## Effort levels

`request.metadata.effort: Option<EffortLevel>` is passed through to the
adapter. Each adapter maps it to its own knob:

| Adapter   | Low                     | Medium (default)         | High                              |
| --------- | ----------------------- | ------------------------ | --------------------------------- |
| Anthropic | `extended_thinking=off` | `extended_thinking=auto` | `extended_thinking={budget: max}` |
| OpenAI    | `reasoning_effort=low`  | `reasoning_effort=medium`| `reasoning_effort=high`           |
| Gemini    | `thinkingConfig=null`   | default                  | `thinkingConfig={budget_tokens:max}` |
| Ollama    | (no-op)                 | (no-op)                  | (no-op)                           |

A route may pin an effort with `effort = "high"` in TOML; the caller can
override per-request via `RequestMetadata.effort`. Closes matrix row I
"effort levels".

## Testing strategy

22 enumerated tests:

**Resolver / capability filtering (`resolver.rs`):**

1. `derived_needs_marks_vision_required_when_image_present`
2. `derived_needs_requires_parallel_when_request_allows_parallel`
3. `breaker_tripped_route_dropped_from_candidates`
4. `breaker_half_open_route_included_with_probe_gate`
5. `explicit_fallback_ids_override_declaration_order`
6. `empty_fallback_array_disables_implicit_chain`
7. `no_candidate_returns_misconfigured_error_with_needs_repr`

**Fallback (`fallback.rs`):**

8. `falls_back_on_model_unavailable`
9. `falls_back_on_rate_limit_after_adapter_retries`
10. `does_not_fall_back_on_auth_error`
11. `does_not_fall_back_on_content_policy`
12. `propagates_cancelled_immediately`
13. `exhausted_chain_returns_fallback_exhausted_error`

**Hedging (`hedging.rs`):**

14. `hedge_fires_after_configured_delay`
15. `hedge_winner_cancels_loser_token`
16. `hedge_loser_error_falls_back_to_next_candidate`
17. `hedge_disabled_per_route_runs_sequentially`

**Breaker (`breaker.rs`):**

18. `breaker_trips_after_threshold_failures_in_window`
19. `breaker_cooldown_elapses_to_half_open`
20. `breaker_half_open_success_closes_breaker`
21. `breaker_half_open_failure_re_trips_with_fresh_cooldown`

**Cache normalization (`cache.rs`):**

22. `merges_anthropic_and_openai_cache_usage_into_route_usage`
23. `clears_cache_control_markers_on_cross_route_hop`

**Binary wiring (`caliban` integration):**

24. `caliban_toml_discovery_walks_up_to_git_root`
25. `provider_handles_built_from_provider_blocks`
26. `unknown_provider_string_fails_at_startup_loudly`

Adds ~26 tests on top of the v1 baseline of ~20.

## Risks

- **Hedging doubles spend.** Mitigation: hedging is opt-in per route;
  `router.route.hedge_loss` is loud in `/usage`; default config has no
  hedge. Document the cost trade-off in the README router section.
- **Circuit breaker false positives.** A transient provider blip during
  a window can trip a breaker for 30s with no recourse. Mitigation:
  per-route override on `failure_threshold`; `router.route.breaker_state`
  visible in `/router debug` so operators can see when this happens.
- **Capability filtering races config.** A model added to an adapter's
  capability table after a session starts won't be visible to a long-
  running router. Mitigation: capabilities are re-read on each call (a
  HashMap lookup); the adapter owns its own caching policy.
- **Cache-marker clearing on fallback may surprise users.** Operators
  who rely on Anthropic prompt-cache savings will see them disappear
  when fallback engages mid-session. Mitigation: clearing is logged via
  `router.cache.markers_cleared`; surfaced as a `/usage` row.
- **`caliban.toml` discovery vs. CLAUDE.md walk competition.** Two
  ancestor walks doing slightly different things is footgun-y.
  Mitigation: share the `WalkUp` utility from the memory crate (ADR
  0018); same algorithm, different filename predicate.
- **Effort-level adapter mapping divergence.** "High effort" on Ollama
  is a no-op today; operators may infer it does something. Mitigation:
  the mapping table above is reproduced in README and exposed via
  `caliban router debug --effort-table`.
- **Tokio time + hedging interplay under runtime-paused tests.** Tests
  need to use `tokio::time::pause()` carefully; mistakes here either
  deadlock or look flaky. Mitigation: a `HedgeDriver::with_clock`
  test seam that swaps the time source.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace --all-targets
  -- -D warnings` clean; `cargo fmt --all -- --check` clean.
- ≥22 new tests passing in `caliban-model-router`, plus ≥3 binary-wiring
  integration tests in `caliban/`.
- `RouteEntry` exposes `id`, `requires`, `fallback`, `hedge`, `breaker`,
  `effort`; `RouterConfig` exposes `[router.breaker]` and `[router.hedge]`
  global defaults. All `#[serde(default)]`-compatible with v1 configs.
- `ModelRouter::from_config` constructs a router with breakers, hedge
  policy, and named-fallback resolution wired up.
- caliban binary auto-discovers `caliban.toml` and builds provider
  handles from `[provider.X]` blocks.
- `caliban router debug` prints the candidate list for the current
  config.
- Matrix I rows for fallback chains, hedging, circuit breakers,
  capability filtering, `caliban.toml` wiring, and effort levels all
  move 🔴 → ✅ in the PR that lands this work.
- ADR 0038 in `accepted` status.
