# Settings.model wiring + api_key_helper integration — design

Date: 2026-05-31
Status: Accepted (sprint mode)
Related: ADR 0026 (settings hierarchy), ADR 0038 (model router v2)

## Problem

Two unrelated wiring gaps both manifest as "caliban defaults to Anthropic even after the operator points `.caliban/settings.toml` at OpenAI":

1. **`Settings.model` is parsed but never consumed.** The unified settings schema
   (`crates/caliban-settings/src/settings.rs:188`) carries a
   `model: Option<ModelSelector>` field, and the `/config` overlay reads it
   (`caliban/src/tui/overlay.rs:453`) — but the model-resolution path in the
   binary (`caliban/src/main.rs:240-243`) only consults `args.model` (CLI flag)
   and falls back to `default_model_for(args.provider)`, where `args.provider`
   defaults to `ProviderKind::Anthropic` via clap (`caliban/src/args.rs:155`).
   The settings field is decorative.

2. **`ApiKeyHelperPool` is built but never called.** A complete helper-script
   pool with TTL cache, slow-helper warnings, and 401-invalidation support
   lives in `crates/caliban-settings/src/api_key_helper.rs`. Zero call sites
   exist in the `caliban` binary crate. `router.rs::build_one` reads
   `std::env::var(api_key_env)` directly.

Both gaps are visible to operators: setting `[model]` in TOML has no effect,
and configuring `api_key_helper` has no effect. The schemas tell the
operator the feature exists; the runtime ignores them.

## Goals

1. Make `Settings.model` and `Settings.fallback_model` the effective model
   selection when CLI flags are absent.
2. Make `api_key_helper` provide the API key when configured, with
   transparent re-acquisition on 401 from the provider.
3. Preserve existing behavior for operators who use neither — no env-var or
   CLI-flag user should see a difference.

## Non-goals

- Wiring helpers for Bedrock / Vertex (they use their own AWS / GCP auth
  chains; out of scope).
- Proactive pre-expiry helper refresh — TTL cache + on-401 reaction is enough.
- Router-config integration: routes already have their own per-`[provider.X]`
  blocks; they reuse `build_one` and inherit the fix.
- Refactoring the single-provider fallback path away — model router v2
  (ADR 0038) is the long-term cure, but the fallback path will stay around
  during the rollout and must work correctly.

## Architecture

### Unit: `EffectiveModel` (new, in `caliban` binary)

A small resolved-config struct constructed once in `main.rs`, threaded into
`startup::*`, the TUI `App`, agents CLI, diagnostics, and TUI overlays —
replacing the ~10 call sites that today read `args.provider` + call
`default_model_for(args.provider)`.

```rust
pub(crate) struct EffectiveModel {
    pub provider: ProviderKind,
    pub name: String,
    pub fallback: Option<(ProviderKind, String)>,
    pub source: ModelSource,  // Cli | Settings | BuiltinDefault
}

impl EffectiveModel {
    pub fn resolve(args: &Args, settings: &Settings) -> Result<Self> { ... }
}
```

**Precedence (high → low):**

```
1. --model + --provider CLI flags
2. Settings.model (project/local/user/managed merged by caliban-settings)
3. ProviderKind::Anthropic + "claude-sonnet-4-6"   (last-resort fallback)
```

Settings consult both forms of `ModelSelector`:

- `ModelSelector::Qualified { provider, name }` → use both.
- `ModelSelector::Name(name)` → use name; provider defaults to `Anthropic`
  if no CLI `--provider` was passed (and emit a warning suggesting the
  qualified form). The fully-qualified form is the supported configuration;
  bare-string is accepted for Claude Code interop.

`Settings.fallback_model` populates `EffectiveModel.fallback` (router path
already consumes `args.fallback_model`; we widen it to settings).

To distinguish "user passed `--provider anthropic` explicitly" from "user
didn't pass `--provider`", change `args.provider` from `ProviderKind`
(currently defaults to `Anthropic` via `default_value_t`) to
`Option<ProviderKind>`. `args.model` is already `Option<String>`. No
mutation of `args` after parse — the `EffectiveModel` struct is the
authoritative source downstream.

### Unit: `RefreshingProvider<P>` (new, in `caliban-provider`)

A decorator wrapping any `Provider`. Implements `Provider` itself, so the
decision to wrap is invisible to consumers.

```rust
pub struct RefreshingProvider<P: Provider> {
    inner: ArcSwap<Arc<P>>,
    pool: Arc<ApiKeyHelperPool>,
    provider_id: String,
    rebuild: Arc<dyn Fn(SecretString) -> Result<P, ProviderError> + Send + Sync>,
}
```

On a `stream` error classified as auth-shaped (see `is_auth_error` below),
it:

1. Calls `pool.invalidate(provider_id)`.
2. Calls `pool.key_for(provider_id)` — may block briefly on the helper
   script; the existing slow-helper warning fires if it exceeds the
   configured threshold.
3. Calls `rebuild(new_key)` to construct a fresh inner provider.
4. Hot-swaps via `ArcSwap::store`.
5. Retries the failed request **once**. A second 401 propagates the original
   error variant unchanged.

Other concurrent in-flight requests continue against the pre-swap inner —
they either succeed (the existing key is still valid for whatever they got
past auth with) or they trigger their own refresh. The pool's cache
deduplicates concurrent refresh attempts: only one helper invocation per
TTL window per provider.

### Helper plumbing in `router.rs::build_one`

```text
spec      = ApiKeyHelperPool::from_raw(settings.api_key_helper)
              .spec_for(provider)

if let Some(spec) = spec:
    key   = pool.key_for(provider)?   // helper run + TTL cache
    inner = build_inner(key)
    return RefreshingProvider::new(inner, pool, provider_id, rebuild_closure)
else:
    key   = env::var(block.api_key_env or default)?
    inner = build_inner(key)
    return inner
```

The pool is constructed once at startup and passed into `build_one`. When no
`api_key_helper` setting is configured, the pool is empty, `spec_for` returns
`None`, and the env-var path runs exactly as today.

### Error classification

```rust
pub fn is_auth_error(err: &ProviderError) -> bool { ... }
```

Lives in `caliban-provider`. Covers:

- Explicit `AuthFailed` variants per adapter (Anthropic + OpenAI use their
  own error types but route through `ProviderError` at the trait boundary).
- `ProviderError::Transport` whose inner reqwest status is `401 Unauthorized`.

Single function so the decorator stays adapter-agnostic.

## Components touched

| File / crate | Change |
|---|---|
| `caliban/src/args.rs` | `provider: Option<ProviderKind>`; drop `default_value_t` |
| `caliban/src/effective_model.rs` (new) | `EffectiveModel::resolve(args, settings)` + tests |
| `caliban/src/main.rs` | Build `EffectiveModel` once after settings load; thread through |
| `caliban/src/startup.rs` | Take `&EffectiveModel`; drop `args.provider` reads (~6 sites) |
| `caliban/src/tui.rs`, `tui/app.rs`, `tui/events.rs`, `tui/render.rs`, `tui/overlay.rs`, `tui/slash/session.rs` | Read from `EffectiveModel` instead of recomputing from args |
| `caliban/src/agents_cli.rs` | Same |
| `caliban/src/diagnostics.rs` | Same |
| `caliban/src/router.rs` | Pass pool into `build_one`; conditionally wrap with `RefreshingProvider` |
| `crates/caliban-provider/src/refreshing.rs` (new) | `RefreshingProvider<P>` decorator |
| `crates/caliban-provider/src/error.rs` | `is_auth_error(&ProviderError) -> bool` |
| `crates/caliban-provider/src/lib.rs` | Re-export `RefreshingProvider`, `is_auth_error` |
| `docs/parity-gap-matrix.md` | Tick the rows this closes |

`crates/caliban-settings/src/api_key_helper.rs` requires **no change** — the
pool API is already complete.

## Testing

### Unit — `caliban` binary

`EffectiveModel::resolve` precedence table:

- CLI `--provider X --model Y` wins over all settings.
- CLI `--model Y` only → settings provider wins; model is CLI Y.
- Settings with `ModelSelector::Qualified { openai, gpt-4o }` and no CLI →
  effective = openai/gpt-4o.
- Settings with `ModelSelector::Name("gpt-4o")` and no CLI provider →
  effective = anthropic/gpt-4o + warning (anti-pattern; bare name doesn't
  imply provider).
- No settings, no CLI → effective = anthropic/claude-sonnet-4-6
  (`BuiltinDefault` source for diagnostics).
- `Settings.fallback_model` lifts into `EffectiveModel.fallback` when set.

### Unit — `caliban-provider`

`RefreshingProvider`:

- Inner returns `Ok` → passes through unchanged, no helper call.
- Inner returns `AuthFailed` → pool invalidated, rebuild called, request
  retried, second response returned.
- Inner returns `AuthFailed` twice → first error variant propagated; only
  one rebuild attempted.
- Inner returns `Transport(non-auth)` → passes through unchanged, no helper
  call.
- Inner returns `Transport(401)` → treated as auth-shaped; refresh + retry.

`is_auth_error` truth table for each ProviderError variant.

### Integration — `caliban` bin

Fixture in a tempdir with:

```toml
# .caliban/settings.toml
[model]
provider = "openai"
name = "gpt-4o"

[[api_key_helper]]
provider = "openai"
command  = "/bin/sh"
args     = ["-c", "printf sk-test-from-helper"]
```

Run `caliban --bare --print "hello"` against a mock OpenAI HTTP server.
Assert:

- Mock receives `Authorization: Bearer sk-test-from-helper`.
- No `OPENAI_API_KEY` env var was read (verify by unsetting it pre-run).
- `caliban_doctor` output (or equivalent diagnostic) reports
  `source: helper`, not `source: env`.

### Regression

Existing `cargo test --workspace` suite must pass unchanged. The env-var
path is the default when no settings configure a helper, so existing
fixtures and CI behavior are preserved.

## YAGNI cuts

- No new crate; the decorator lives in `caliban-provider`.
- No `Provider::refresh()` trait method; the decorator owns the refresh
  externally so adapters stay clean.
- No proactive pre-expiry refresh.
- No router-config helper integration beyond `build_one`.
- No retries beyond 1. Operators with broken helpers get a clear single
  error instead of a retry storm.

## Risks

1. **`Option<ProviderKind>` ripples.** ~10 TUI sites mutate together;
   tedious but mechanical. Mitigated by `EffectiveModel` centralization —
   call sites become "read `effective.name`" instead of "recompute from
   args".
2. **Helper script latency on hot path.** Pool TTL (5 min default) keeps
   it amortized; the existing slow-helper warning surfaces pathological
   scripts.
3. **`ArcSwap` race.** In-flight requests during a swap finish on the
   pre-swap inner (key already valid past their auth handshake). New
   requests after the swap use the fresh key. No torn reads.
4. **Bare-string model with no provider.** A bare `model = "gpt-4o"` in
   settings with no `--provider` flag picks Anthropic + warning, which
   will fail at the provider. This is intentionally loud — operators
   should use the qualified `{ provider, name }` form. We don't try to
   guess provider from model name.

## Open questions

None blocking. Sprint-mode skips plan review; implementation proceeds
directly.
