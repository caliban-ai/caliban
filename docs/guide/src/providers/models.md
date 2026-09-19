# Model Selection

Caliban lets you choose the exact model at the command line, in settings, or via the model router. When multiple sources specify a model, a clear precedence chain resolves the winner.

## Selecting a model at the command line

Use `--model` to name the model you want:

```bash
caliban --model claude-opus-4-7 "write a haiku"
caliban --provider openai --model gpt-5.5 "explain monads"
caliban --provider google --model gemini-2.0-flash "summarize this"
```

For local inference, use `--provider openai` with a `base_url` pointing at a
local server's `/v1` endpoint — see [Local Inference](./local-inference.md).

## Per-provider defaults

When `--model` is omitted and no model is set in settings, caliban uses a built-in default for the chosen provider:

| Provider | Default model |
|---|---|
| `anthropic` | `claude-sonnet-4-6` |
| `openai` | `gpt-5.5` |
| `google` | `gemini-2.0-flash` |

## Local models: dynamic discovery

A local server's models are inherently dynamic — you load, swap, and unload them at will — so the OpenAI adapter discovers them at runtime rather than relying on a static table. When pointed at a local `/v1` endpoint (via `base_url`), it queries `GET /v1/models` and reads each model's loaded context window from the entry's `meta.n_ctx`, so the window shown in the status bar reflects what the server actually reports — e.g. a 256K-context model shows 256K, not a hardcoded guess. This is the same capability the removed bespoke provider offered, now on the OpenAI-compatible seam ([ADR 0056](../adr/README.md)). See [Local Inference](./local-inference.md) for setup.

Not every server reports a window. `meta.n_ctx` is a llama.cpp field; mlx-lm and the llama-swap aggregate `/v1/models` list model ids with no context metadata. When the window can't be discovered — and the model isn't in the static table — caliban treats it as **unknown** rather than inventing a value: the status bar simply omits the utilization segment (no fabricated "0% of 128K"), headless `system` frames report `model_context_window: null`, and context-window-based autocompaction stays off (it has no real limit to target). Set the window explicitly in config if your server doesn't expose it.

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
