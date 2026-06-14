# Model Router v1 — Implementation Plan

> Executed inline. v1 is a working router with the `Provider`-impl + purpose-keyed routing + per-route stats; fallback chains, hedging, circuit breakers, and TUI/`caliban.toml` wiring are explicitly deferred.

**Architecture (delivered):**

- **`caliban-provider`** gains `RequestPurpose` enum (`MainLoop`, `Summarization`, `FastClassifier`, `SubAgent`, `Embedding`, `Other`) and a new `purpose: Option<RequestPurpose>` field on `RequestMetadata` (snake_case serde, `#[serde(default, skip_serializing_if = "Option::is_none")]` so existing on-disk requests round-trip cleanly).
- **`caliban-agent-core`** stamps the main-loop request's metadata with `purpose: Some(MainLoop)` in `stream.rs` and the summarizer's request with `purpose: Some(Summarization)` in `compact.rs`. Adapter crates ignore the field; only the router consumes it.
- **`caliban-model-router`** new crate:
  - `RouterConfig` + `RouteEntry` (TOML schema) and `parse_router_config` that reads the `[router]` section of a `caliban.toml` body.
  - `ModelRouterBuilder` fluent API: `default_purpose`, `add_provider(name, Arc<dyn Provider>)`, `route(purpose, provider, model)`.
  - `ModelRouter` impl `Provider`:
    - `complete` and `stream` override `request.model` with the resolved route's model, dispatch to the named provider, record per-route success/failure + tokens (only for complete; stream events feed agent-level aggregation).
    - `capabilities` falls back to the default-purpose route's provider.
    - `list_models` aggregates de-duped models across registered providers.
    - `name()` returns `"router"`.
  - Validation in `build()`: rejects empty provider map, unknown provider references, default-purpose with no route.
  - First-match-wins resolution by declaration order; falls back to default-purpose when the requested purpose has no route.

**Tests delivered (12):**

- Config parsing: minimal, multi-purpose, missing-router-section, invalid-purpose.
- Builder validation: unknown provider, empty providers, default-purpose unrouted.
- Resolution: explicit purpose, no-purpose fallback, unrouted purpose fallback, first-match wins for same purpose.
- `name()` returns `"router"`.

**Spec:** `docs/superpowers/specs/2026-05-23-model-router-design.md`
**ADR:** `docs/adr/0022-model-routing-architecture.md`

**Deferred:**

- Fallback chain (try next entry on fatal-for-route errors).
- Hedged requests; circuit breakers.
- Capability-based filtering (vision/thinking/tool_use auto-derived from request shape).
- Per-route prompt-cache normalization.
- `caliban.toml` discovery + binary wiring to construct the router. The crate is library-ready; the binary continues to use a single provider until a follow-up PR adds the TOML loader path.
