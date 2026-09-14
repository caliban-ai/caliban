# Supported Providers

Caliban is provider-agnostic: you choose which AI provider and model to use at runtime, and the same agent loop, tool engine, and permission system work regardless of which backend answers the requests.

## Provider table

| Provider | `--provider` value | Transport / access | Notes |
|---|---|---|---|
| **Anthropic** | `anthropic` | Direct HTTPS (`api.anthropic.com`) | Default provider |
| **OpenAI** | `openai` | Direct HTTPS (`api.openai.com/v1`), or any OpenAI-compatible `base_url` | Also the path for [local inference](./local-inference.md) |
| **Google** | `google` | Google AI Studio (`generativelanguage.googleapis.com`) | Gemini models |

These three are the only providers the `caliban` binary can use, whether through `--provider` or a `[provider.<name>]` block in the [model router](./router.md). Any other provider name fails at startup (`unknown provider … — supported: anthropic, openai, google`).

```admonish warning title="Bedrock, Vertex, and Azure are library-only"
The workspace also ships `caliban-provider-bedrock` and `caliban-provider-vertex`, plus
`bedrock`/`vertex`/`azure` Cargo features on the provider crates. These transports
work when you embed the crates as a library. The `caliban` binary neither depends on
nor routes to them, so they **cannot be selected from the CLI or `caliban.toml`
today**.
```

## Capability matrix

Bedrock, Vertex, and Azure rows describe the library adapters (see above).

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
