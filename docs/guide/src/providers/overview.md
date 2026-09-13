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

```admonish note title="Running local models"
The bespoke `ollama` provider was removed ([ADR 0056](../adr/README.md)). Run local models through the OpenAI provider pointed at a local server's `/v1` endpoint instead — see [Local Inference](./local-inference.md). Local inference keeps all its advantages (no API key, no network egress, no per-token cost); you reach it through the OpenAI-compatible seam, which works with llama.cpp, mlx-lm, LM Studio, and llama-swap.
```

```admonish tip title="Multiple providers at once"
The [model router](./router.md) lets you combine providers: for example, route main-loop turns through Anthropic while using a local model (via the OpenAI provider + `base_url`) for fast classification. Each route gets its own provider, model, and resilience policy.
```
