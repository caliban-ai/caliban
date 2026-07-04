# Dynamic Ollama Model Discovery — Design

- **Status:** approved
- **Date:** 2026-07-04
- **Ticket:** #316 (follow-ups: #317 vLLM, #318 LM Studio)

## Context & problem

Caliban's Ollama provider treats a hardcoded static table
(`caliban-provider-ollama/src/models.rs`) as the source of truth for the model
list and per-model capabilities (context window, vision, tools). Static data is
inherently wrong for a provider whose model availability is dynamic:

- Models absent from the table fall to `FALLBACK = 32_768` context.
  `qwen3.6:27b-mlx` isn't in the table (only `qwen3.5`/`qwen3`/…), so caliban
  shows **32K** while the server reports **262144 (256K)** — verified live on
  `192.168.1.240`: `/api/show` → `qwen3_5.context_length: 262144`, `/api/ps`
  (live) → `context_length: 262144`.
- Custom / MLX / quantized builds, newly-pulled models, and server-side
  `num_ctx` overrides cannot be represented statically.
- The #60 runtime context probe (`resolve_and_cache` via `/api/ps` + `/api/show`)
  exists but is only invoked as a side effect of `complete`/`stream` and inside
  `refresh_models()` — which has **no runtime caller**. `capabilities()` is a
  synchronous cache read. The TUI sets its context-window capacity **once at
  startup** (`tui/app.rs:479-480`) from that sync (empty-cache) read, taking the
  static `FALLBACK` (32K) and never refreshing it. The real value never reaches
  the display, and the agent's context budgeting truncates at 32K.

## Goals

1. The Ollama model list + capabilities are **discovered at runtime from the
   server**, not a static table.
2. **No static per-model table as a source of truth.** Static data drifts and is
   "likely to be wrong"; the fallback is a *persisted cache of the last
   successful discovery*, not hardcoded values.
3. The TUI context-window capacity reflects the server's real value (fixes the
   32K display and the downstream budgeting truncation).
4. The list stays current at runtime (models pulled/loaded after startup appear).
5. Discovery is expressed through the **provider-agnostic** `Provider` trait so
   vLLM (#317) and LM Studio (#318) implement the same contract.

## Non-goals (YAGNI)

- Background polling. Freshness is **refresh-on-picker-open** + live loaded-context
  for the active model.
- A new cross-provider discovery framework. The existing
  `Provider::refresh_models`/`list_models` + `ModelInfo`/`Capabilities` types are
  the contract.
- Changing Anthropic/OpenAI providers. Their model availability is stable and
  they have no capability-bearing discovery endpoint; they keep static tables.
- Capability inference heuristics beyond an honest "unknown".

## The provider-agnostic contract

Discovery already has a home on the `Provider` trait:

- `async fn refresh_models(&self) -> Result<Vec<ModelInfo>>` — hit the server,
  return the current models with capabilities. **This is the source of truth.**
- `fn list_models(&self) -> Vec<ModelInfo>` — synchronous reader of the
  last-known result (in-memory, seeded from the persisted cache at startup).
- `fn capabilities(&self, model) -> Capabilities` — synchronous reader used by
  the TUI; returns the discovered/cached capability, or an honest default.

Each discoverable provider maps its own API onto `ModelInfo { id, native_id,
display_name, capabilities }`. Ollama enriches via native endpoints; the
OpenAI-compatible `/v1/models` (vLLM/llama.cpp) is the lowest common denominator
(list only). The persisted-discovery-cache pattern (below) is part of this
contract, so all dynamic providers inherit warm-start + offline behavior.

## Ollama implementation

### Endpoints (verified live)

- `GET /api/tags` — available (pulled) models: `name`, `details`.
- `POST /api/show` — per-model `capabilities[]`
  (`completion`/`vision`/`tools`/`thinking`), `model_info["<arch>.context_length"]`,
  `details`.
- `GET /api/ps` — currently-loaded models with live `context_length` (honors
  runtime `num_ctx`).

### Components

1. **Transport** (`schema` + trait): add `list_tags() -> Vec<TagEntry>`
   (`/api/tags`). Keep `running_models()` (`/api/ps`) and `show_model()`
   (`/api/show`).
2. **Capability parsing** (`schema/probe.rs`): extend `ModelShow` with the
   `capabilities: Vec<String>` array. Map to `Capabilities`:

   | Ollama `capabilities[]` / field | `Capabilities` |
   |---|---|
   | `model_info[*.context_length]` (or `/api/ps` live) | `max_input_tokens` |
   | `vision` | `vision = true` |
   | `tools` present | `tool_use = ToolUseCapability::ParallelCalls` (else `None`) |
   | `thinking` | `thinking = true` |
   | `completion` (base) | `streaming = true`, `stop_sequences = true` |
   | *(not reported)* | `max_output_tokens` = conservative default (8192) |

3. **`refresh_models()`**: `GET /api/tags` → model list; for each, `POST
   /api/show` (bounded-parallel, cached by canonical id) → caps + context;
   overlay `GET /api/ps` live context for loaded models (wins over the show
   value). Persist the result (below). Any transport error ⇒ do **not** fail;
   fall through to the persisted cache.
4. **Retire the static table**: `models.rs`'s per-model list and `FALLBACK` are
   removed as a source of truth. `capabilities_for` / `list_models` resolve from
   the in-memory discovery (seeded from cache), else the honest default.

### Persisted discovery cache

- **Location:** `$XDG_CACHE_HOME/caliban/discovery/ollama-<host>.json`
  (`caliban_common::paths::platform_cache_dir()`; ADR 0050). Regenerable ⇒ cache,
  not state.
- **Key:** provider name + server base URL (`<host>` derived from the base URL,
  sanitized). Different Ollama servers cache independently.
- **Contents:** the `Vec<ModelInfo>` from the last successful `refresh_models()`
  (models + capabilities + context), plus a timestamp.
- **Write:** after every successful `refresh_models()`.
- **Read:** synchronously at provider construction, to seed the in-memory list +
  capacities with last-known-good values *before* any async refresh — so the
  startup capacity read never sees a wrong static default.

### Fallback hierarchy

1. Live discovery (`/api/tags` + `/api/show` + `/api/ps`) — truth.
2. Persisted cache — last-known-good (warm start; server briefly unreachable).
3. Neither (cold first run + server down): the configured/selected model shown
   with context/caps **"unknown (server unreachable)"**. No fabricated per-model
   numbers. A single conservative bootstrap default keeps the UI functional but
   is never asserted as the model's real capability.

### TUI wiring (fixes the 32K display)

- **Refresh-on-open:** opening the model picker triggers `refresh_models()`
  (async), so the list reflects the server and catches models pulled/loaded
  after startup.
- **Capacity from the resolved value:** the context-window capacity is set from
  the discovered/cached capability and **re-set when the async refresh returns**
  (not read once at startup from a stale sync value). `tui/app.rs` seeds from the
  cache at construction; a refresh completion updates `context_window`
  capacity. On `/model` switch (`tui/slash/model.rs`), capacity comes from the
  discovered capability for the new model.
- **Live loaded-context** overlays the status bar for the active model when
  `/api/ps` reports it.

## Testing

Offline unit tests against a mock `Transport` (extending the existing
`SequencedProbe` style in `caliban-provider-ollama`):

- `list_tags` → model list assembly.
- `show_model` `capabilities[]` → `Capabilities` mapping (vision / tools /
  thinking / context_length).
- `/api/ps` live context overlays and **wins** over the `/api/show` value.
- persisted cache: write on refresh; read seeds `list_models` / `capabilities`;
  server-unreachable ⇒ cache fallback; no cache ⇒ honest "unknown".
- **32K regression:** the capability→capacity path yields the server's context
  (e.g. 262144), not `FALLBACK`.

TUI capacity refresh is covered by a focused test on the seed→refresh→capacity
update path (the existing `ContextWindow::set_capacity` tests are the anchor).

## Follow-ups

- **#317 vLLM** — `/v1/models` + `max_model_len`; capabilities honest-unknown.
- **#318 LM Studio** — native `/api/v0/models` (rich caps: `max_context_length`,
  `loaded_context_length`, `state`), OpenAI `/v1/models` fallback.

Both implement this contract (discovery + persisted cache + refresh-on-open).
