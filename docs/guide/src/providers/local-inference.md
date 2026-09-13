# Local Inference (llama.cpp / MLX)

caliban runs local models through its **OpenAI-compatible provider** pointed at a
local inference server — not through a dedicated integration. Any server that
speaks the OpenAI `/v1/chat/completions` API works: llama.cpp's `llama serve`,
`mlx_lm.server`, LM Studio, `llama-swap`, vLLM, and others.

```admonish info title="The built-in `ollama` provider was removed"
The bespoke `ollama` provider was removed ([ADR 0056](../adr/README.md)). Reach
local models through the OpenAI provider + a `base_url` instead, as described
below. This is the same seam that reaches OpenAI itself — no dedicated provider,
so any OpenAI-compatible local engine is one config line away. `--provider
ollama` is no longer valid; use `--provider openai` with a `base_url`.
```

## Why this instead of a dedicated provider

The decision, its benchmark and conformance evidence (gathered on Apple M5 Pro),
and the phased removal plan are recorded in **ADR 0056**. In short: the local
engine landscape is uniformly OpenAI-compatible now; **llama.cpp** is the best
all-round default (portable single binary, widest model coverage, strong
prompt-cache warm path), **MLX** (`mlx_lm.server`) is an optional per-host tune
on Apple Silicon, and all three engines drive caliban's agent loop correctly.

## Wiring caliban to a local server

Point the OpenAI provider at the server's `/v1` endpoint. When `OPENAI_BASE_URL`
is set, **no API key is required** — caliban sends an empty bearer and the local
server ignores it (a keyed proxy would reject the request itself). Set
`OPENAI_API_KEY` only if your endpoint actually enforces auth.

**Env override (quickest):**

```bash
export OPENAI_BASE_URL="http://192.168.1.240:9292/v1"
caliban --provider openai --model <model-name> "…"
```

**Router route (per-model / per-host), in `caliban.toml`:**

```toml
[provider.openai]
base_url = "http://192.168.1.240:9292/v1"

[[router.route]]
purpose = "main_loop"
provider = "openai"
model = "<model-name>"
```

```admonish important title="The model name must be one the backend accepts"
caliban sends the route's `model` field to the server **verbatim**. Engines
differ in how they treat it:

- **llama.cpp** ignores the field and serves whatever is loaded — any name works.
- **mlx-lm** treats it as a model to *load*: an unknown name is fetched from
  Hugging Face and 404s. Use the **exact HF repo id** (e.g.
  `mlx-community/Qwen3.6-27B-4bit`).
- **llama-swap** routes on the name, then passes it through to the backend — so a
  friendly alias is fine for a llama.cpp backend, but a llama-swap entry fronting
  mlx-lm must be keyed by the repo id.
```

## Engine setup

Standing up llama.cpp (`llama serve`), `mlx_lm.server`, and `llama-swap` (a proxy
that hot-swaps engines behind one `/v1` port) on a headless Apple-Silicon box —
including the TLS-inspection cert fix and health-check settings — is an operator
task. Two helper scripts in the repo support the migration:

- `scripts/bench-local-inference.sh` — cold/warm prefill, TTFT, and decode
  throughput across engines.
- `scripts/conformance-local-inference.sh` — a capability matrix (tool calling,
  stop reasons, reasoning field) asserting a backend behaves as caliban needs.

## Reasoning models

Reasoning-family models (Qwen3.x, DeepSeek-R1, …) stream a thinking trace.
llama.cpp emits it under `reasoning_content`; MLX servers use `reasoning`.
caliban accepts both, so thinking is captured as a Thinking block regardless of
which engine serves it.
