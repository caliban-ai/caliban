# Supported Providers

Caliban is provider-agnostic: you choose which AI provider and model to use at runtime, and the same agent loop, tool engine, and permission system work regardless of which backend answers the requests.

## Provider table

| Provider | `--provider` value | Transport / access | Notes |
|---|---|---|---|
| **Anthropic** | `anthropic` | Direct HTTPS (`api.anthropic.com`) | Default provider |
| **Anthropic via Bedrock** | *(router only)* | AWS Bedrock (`bedrock-runtime.*`) | Requires `caliban-provider-bedrock`; configured via `caliban.toml` |
| **Anthropic via Vertex** | *(router only)* | Google Vertex AI | Requires `caliban-provider-vertex`; configured via `caliban.toml` |
| **OpenAI** | `openai` | Direct HTTPS (`api.openai.com/v1`) | |
| **OpenAI via Azure** | *(router only)* | Azure OpenAI Service | `azure` feature flag on `caliban-provider-openai`; configured via `caliban.toml` |
| **Google** | `google` | Google AI Studio (`generativelanguage.googleapis.com`) | Gemini models |
| **Google via Vertex** | *(router only)* | Google Vertex AI | `vertex` feature flag; configured via `caliban.toml` |
| **OpenRouter** | *(router only)* | Direct HTTPS (`openrouter.ai/api/v1`) | Gateway fronting 400+ models from many vendors; one key, one bill. Configured via `caliban.toml`; capabilities come from a vendored catalogue — see below |
| **Ollama** | `ollama` | Local HTTP (`http://localhost:11434`) | No API key required |

Bedrock, Vertex, and Azure transports are enabled by **Cargo feature flags** at build time. Binary distributions built by the project team include all features; self-compiled builds must enable the relevant feature (e.g. `--features bedrock`). These transports can only be selected through the [model router](./router.md) — they are not available via the `--provider` CLI flag.

## Capability matrix

| Provider | Tool use | Vision | Thinking | Prompt caching |
|---|---|---|---|---|
| Anthropic | Parallel | Yes | Yes | Explicit (up to 4 breakpoints) |
| Bedrock | Parallel | Yes | Yes | Explicit (mirrors Anthropic) |
| Vertex (Anthropic) | Parallel | Yes | Yes | Explicit (mirrors Anthropic) |
| OpenAI | Parallel | Yes | Yes (o-series) | Automatic |
| Azure OpenAI | Parallel | Yes | Yes (o-series) | Automatic |
| Google AI Studio | Parallel | Yes | No | None |
| Google Vertex | Parallel | Yes | No | None |
| OpenRouter | Per-model (from catalogue) | Per-model | Per-model | None (upstream-dependent) |
| Ollama | Basic | Model-dependent | Model-dependent | None |

```admonish note title="Ollama is local"
Ollama runs models on your own machine. No API key, no network traffic, no per-token cost. Ideal for fast-classifier routes, offline use, or privacy-sensitive workloads. Capability varies by the specific model you pull.
```

```admonish tip title="Multiple providers at once"
The [model router](./router.md) lets you combine providers: for example, route main-loop turns through Anthropic while using a local Ollama model for fast classification. Each route gets its own provider, model, and resilience policy.
```

## OpenRouter

[OpenRouter](https://openrouter.ai) is a single OpenAI-compatible gateway in
front of several hundred models. caliban talks to it through a dedicated
`openrouter` provider rather than the `openai` one — see the warning below for
why that distinction matters.

Like Bedrock, Vertex, and Azure, it is configured through the router in
`caliban.toml` rather than the `--provider` flag: OpenRouter has no single
default model to fall back to, so a route must name one explicitly.

```toml
[router.providers.openrouter]
api_key_env = "OPENROUTER_API_KEY"   # default
# base_url  = "https://openrouter.ai/api/v1"   # default

[[router.routes]]
purpose  = "main_loop"
provider = "openrouter"
model    = "anthropic/claude-opus-4"   # OpenRouter ids are `vendor/model`
```

### Where capabilities come from

Every other provider adapter carries a hardcoded capability table. That does not
work for a gateway whose catalogue spans hundreds of models with wildly
different context windows, modalities, and tool support — so the `openrouter`
adapter reads a **vendored snapshot** of OpenRouter's public catalogue, embedded
at build time. This keeps a third-party endpoint off caliban's boot path, the
same posture as the telemetry rate card (ADR 0033).

Refresh it when OpenRouter's catalogue moves, and commit the result:

```bash
scripts/refresh-openrouter-models.sh
```

Two escape hatches:

- `CALIBAN_OPENROUTER_MODELS=/path/to/models.json` points at a newer snapshot
  without rebuilding. A file that cannot be read or parsed is a hard error — it
  never falls back to the embedded copy behind your back.
- `Provider::refresh_models()` fetches the live catalogue on demand. It is
  opt-in by contract and is never called during startup.

A model that is not in the catalogue does **not** get invented capabilities. It
receives a minimal set (no tools, no vision, no reasoning) and logs a warning
naming the model, once per session.

```admonish warning title="Don't reach OpenRouter via the `openai` provider"
Pointing `[router.providers.openai].base_url` at `https://openrouter.ai/api/v1`
appears to work — requests reach OpenRouter and come back. But the OpenAI
adapter resolves capabilities from its own static table, and no OpenRouter model
id is in it, so **every** model silently falls back to
`128k in / 4096 out, no vision, no thinking` while still claiming parallel tool
use, JSON mode, and automatic prompt caching.

Measured against the catalogue, that fallback denies vision to 250 models,
denies reasoning to 288, claims tool use for 70 that have none, and understates
output limits by up to ~94x. Use `provider = "openrouter"`.
```
