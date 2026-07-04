# Model Selection

Caliban lets you choose the exact model at the command line, in settings, or via the model router. When multiple sources specify a model, a clear precedence chain resolves the winner.

## Selecting a model at the command line

Use `--model` to name the model you want:

```bash
caliban --model claude-opus-4-7 "write a haiku"
caliban --provider openai --model gpt-5.5 "explain monads"
caliban --provider google --model gemini-2.0-flash "summarize this"
caliban --provider ollama --model qwen3.5:9b "local inference"
```

## Per-provider defaults

When `--model` is omitted and no model is set in settings, caliban uses a built-in default for the chosen provider:

| Provider | Default model |
|---|---|
| `anthropic` | `claude-sonnet-4-6` |
| `openai` | `gpt-5.5` |
| `google` | `gemini-2.0-flash` |
| `ollama` | `llama3.1` |

## Ollama: dynamic model discovery

Unlike the hosted providers (Anthropic, OpenAI, Google), Ollama's available models are inherently dynamic — you pull, remove, and load them on the server at will. Caliban therefore treats **the Ollama server as the source of truth** and has **no static model table** for it: the model list and each model's capabilities (context window, vision, tools, reasoning) are discovered at runtime from the server's own API:

- `GET /api/tags` — the models you have pulled.
- `POST /api/show` — per-model `capabilities` and maximum `context_length`.
- `GET /api/ps` — the live context window of a currently-loaded model (honors a server-side `num_ctx`).

This means the context window shown in the status bar reflects what the server actually reports — e.g. a 256K-context model shows 256K, not a hardcoded guess.

**When discovery happens:**

- **At startup** — a background refresh updates capabilities shortly after launch.
- **On `/model`** — opening the model list re-queries the server, so a model pulled or loaded after startup appears immediately.
- **Warm start** — the last successful discovery is cached to `$XDG_CACHE_HOME/caliban/discovery/ollama-<host>.json` (per server) and loaded at startup, so correct values are available instantly and offline.

If the server is unreachable and no cache exists yet, caliban shows a conservative default and marks capabilities as not-yet-known rather than asserting a wrong value. Point caliban at a specific server with `OLLAMA_BASE_URL` (each server caches independently).

> The same pattern will extend to other locally-served, OpenAI-compatible back ends (vLLM, LM Studio) — see the provider roadmap.

## Setting a model in settings

Set `model` in your project or user settings file to avoid repeating `--model` on every invocation. Two forms are accepted:

**Bare string** — the provider is inferred from the model name resolution or `--provider`:

```toml
model = "claude-sonnet-4-6"
```

**Qualified object** — explicitly names both the provider and the model:

```toml
[model]
provider = "anthropic"
name = "claude-sonnet-4-6"
```

The qualified form is the safest option in shared project configs because it makes the intended provider unambiguous.

You can also set a `fallback_model` that caliban uses when the primary model errors:

```toml
[model]
provider = "anthropic"
name = "claude-opus-4-7"

[fallback_model]
provider = "anthropic"
name = "claude-sonnet-4-6"
```

## Fallback model (`--fallback-model`)

Pass `--fallback-model` on the command line to override the settings fallback for a single run:

```bash
caliban --model claude-opus-4-7 --fallback-model claude-sonnet-4-6 "long task"
```

The fallback is wired through `caliban-model-router` (ADR 0038) and is also surfaced in the headless `system/init` frame.

## Per-turn limits

Control token usage and sampling with these flags:

| Flag | Default | Description |
|---|---|---|
| `--max-tokens N` | `8192` | Per-turn output token limit. Must be ≥ 1. |
| `--temperature F` | *(provider default)* | Sampling temperature in `[0.0, 2.0]`. Values outside this range are rejected at startup. |

```bash
caliban --max-tokens 8192 --temperature 0.2 "write a long essay"
```

## Per-purpose model overrides (`model_overrides`)

For finer-grained control without a full router config, set `model_overrides` in settings to pin specific request purposes to a particular model string:

```toml
[model_overrides]
fast-classifier = "claude-haiku-4-5"
summarization = "claude-haiku-4-5"
```

The keys must match the purpose slugs understood by the router (`main_loop`, `summarization`, `fast_classifier`, `sub_agent`, `embedding`). This setting does not support cross-provider routing; use the [model router](./router.md) for that.

## Precedence

When multiple sources specify a model, this chain resolves the winner (highest priority first):

```mermaid
flowchart LR
    A["CLI<br/>--model / --provider"] --> B["settings.model<br/>(project > user > managed)"]
    B --> C["Provider default<br/>(built-in table)"]
```

1. **CLI flags** (`--model`, `--provider`) — always win.
2. **`settings.model`** — merged across the settings scope chain (project > user > managed).
3. **Provider built-in default** — the per-provider fallback in the table above.

For the most flexible per-purpose routing, see [The Model Router](./router.md).
