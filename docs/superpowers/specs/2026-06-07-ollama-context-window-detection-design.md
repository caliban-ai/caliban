# Ollama context-window detection (`/api/ps` + `/api/show`)

**Issue:** caliban-ai/caliban#60
**Date:** 2026-06-07
**Scope:** `crates/caliban-provider-ollama` only — no `Provider` trait, agent-core, or
`caliban-provider::Capabilities` changes.

## Problem

The Ollama provider derives a model's context window from a **static lookup table**
(`models.rs`) with a `FALLBACK = 32_768` for anything unmatched. It never asks the
server. Custom/MLX builds (e.g. `qwen3.6:27b-mlx`) aren't in the table, so the
status-bar "X% of N" segment and any context-budget logic use a wrong `N` — an 8×
underestimate for a 256K model.

Ollama exposes the **real** context length two ways:

- `GET /api/ps` → `context_length` per **currently-loaded** model (the live value,
  honoring the server's runtime `num_ctx`).
- `POST /api/show` → `model_info["<arch>.context_length"]` (the model's max, works
  even when the model is **not** loaded).

## Ground truth (captured from the live server, Ollama 0.30.6, 2026-06-07)

| Model | `/api/show` arch key | context_length | format | loads (`/api/ps`)? |
|---|---|---|---|---|
| `qwen3.6:27b-mlx` | `qwen3_5.context_length` | 262144 | safetensors | yes (live 262144) |
| `gemma4:12b-mlx` | `gemma4_unified.context_length` | 131072 | safetensors | yes |
| `gemma4:26b-mlx` | `gemma4.context_length` | 262144 | safetensors | yes |
| `gemma3:1b` | `gemma3.context_length` | 32768 | gguf | **no** (GGUF runner absent → `/api/ps` empty) |

Shapes:

```jsonc
// GET /api/ps
{ "models": [ { "name": "qwen3.6:27b-mlx", "model": "qwen3.6:27b-mlx",
               "context_length": 262144, "details": { ... } } ] }
// {"models": []} when nothing is loaded.

// POST /api/show  {"model": "qwen3.6:27b-mlx"}
{ "model_info": { "general.architecture": "qwen3_5",
                  "qwen3_5.context_length": 262144, ... } }
```

Two lessons that shape the design:

1. The `model_info` key is **architecture-prefixed** and the prefix varies
   (`qwen3_5`, `gemma4`, `gemma4_unified`, `gemma3`). Hardcoding arch names is
   fragile → **scan for any key ending in `.context_length`** (preferring the
   `general.architecture`-derived key when present).
2. A real model can resolve via `/api/show` but **never** via `/api/ps`
   (`gemma3:1b` here) → the fallback chain is exercised by real data, not just
   synthetic fixtures.

## Constraint

`Provider::capabilities(&self, model) -> Capabilities` is **synchronous** and is
called on the hot path inside the streaming loop (`agent-core/src/stream/mod.rs`
:405, :424). It cannot issue async HTTP. So detection must happen on the async
`complete`/`stream` path and be cached for the sync `capabilities()` reader.

## Design

Interior-mutable cache + lazy async refresh, entirely inside the ollama crate.

### 1. `Transport` trait — two new probe methods (default no-op impls)

```rust
async fn running_models(&self) -> Result<Vec<RunningModel>, OllamaError> { Ok(Vec::new()) }
async fn show_model(&self, _model: &str) -> Result<Option<ModelShow>, OllamaError> { Ok(None) }
```

Defaults return empty so existing/test transports need no changes and simply fall
through to the static table. `DirectTransport` implements both with real HTTP.

### 2. Schema (`schema/probe.rs`)

```rust
struct RunningModelList { models: Vec<RunningModel> }       // GET /api/ps
struct RunningModel { name: String, model: String, context_length: Option<u32> }
struct ModelShow { model_info: HashMap<String, serde_json::Value> }   // POST /api/show
```

- `RunningModel::matches(wire)` → `name == wire || model == wire`.
- `ModelShow::context_length()` → prefer `"<general.architecture>.context_length"`,
  else first key ending in `.context_length`; value via `as_u64` → `u32::try_from`
  (rejects non-integers / overflow → robust to garbage).

### 3. Provider cache

```rust
pub struct OllamaProvider<T: Transport> {
    transport: T,
    ctx_cache: RwLock<HashMap<String, u32>>,   // canonical model id -> resolved context length
}
```

`std::sync::RwLock` (no new dep); critical sections hold no `.await`.

### 4. Resolution order (`resolve_and_cache(canonical, wire)`)

1. `/api/ps` `context_length` for the matching loaded model → **wins** (live value).
2. else if a value is already cached for `canonical` → keep it (don't downgrade /
   don't re-hit `/api/show` every turn).
3. else `/api/show` `context_length` → the model max (works unloaded).
4. else leave unset → `capabilities()` returns the static-table value → `FALLBACK`.

Every rung tolerates failure: a transport `Err` (404 on old Ollama, connection
refused, parse failure) is treated as "no data" and falls through to the next rung.

### 5. Wiring

- `complete()` / `stream()` call `resolve_and_cache(&canonical, &wire)` before
  sending (errors ignored — non-fatal). Cost: one tiny local GET per turn; on a
  model's first turn (not yet loaded) one extra `/api/show` POST.
- `capabilities(model)` overlays the cached value:
  ```rust
  let mut caps = models::capabilities_for(model);
  if let Some(ctx) = self.cached_ctx(model) { caps.max_input_tokens = ctx; }
  caps
  ```

**Convergence:** turn 1 → `/api/show` max (model not loaded yet); turn 2+ →
`/api/ps` live value overrides. The status bar self-corrects after the first turn.

## Testing

Unit (`schema/probe.rs`):
- Parse `/api/ps` fixture → `context_length` extracted; `matches()` by name & model.
- `/api/show` extraction across all four real arch prefixes; `{"models": []}` /
  missing-key / non-integer value → `None`.

Integration (wiremock, real `DirectTransport`, mirrors existing test style):
- `/api/ps` wins over the static table (`max_input_tokens` becomes 262144).
- not-loaded: `/api/ps` empty → `/api/show` fallback resolves (the `gemma3:1b`
  scenario).
- both absent / erroring → static `FALLBACK` (32768) preserved.
- `/api/ps` live value overrides a previously `/api/show`-sourced value.
- `capabilities()` reflects the cache only after `complete()`/`stream()` runs.

## Out of scope

- Periodic background refresh / TTL eviction (per-turn refresh suffices).
- Surfacing `/api/show` `parameter_size`/quant into `Capabilities`.
- Reworking the static table (kept as the final fallback).
