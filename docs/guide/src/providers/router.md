# The Model Router

```admonish note title="Advanced / optional feature"
The model router is an optional layer. If you only need a single provider and model, the `--provider` and `--model` flags are all you need. This chapter is relevant when you want per-purpose model dispatch, fallback chains, hedging, or circuit breakers.
```

The model router is a purpose-keyed dispatcher that sits between the agent loop and your provider adapters. It lets you assign different models — from the same or different providers — to different kinds of requests, and adds resilience features (fallback, hedging, circuit breakers) on top.

## Why a router?

The agent makes provider calls for several distinct purposes: the main conversational loop, summarization for compaction, fast classification for permission decisions, sub-agent loops, and more. The router lets you express policies like:

- Use Claude Opus for main-loop turns, Claude Haiku for summarization.
- Route fast classification to a local model via the OpenAI provider + `base_url` (zero API cost, low latency).
- Fall back from Anthropic to OpenAI if Anthropic returns a rate-limit error.

## Request purposes

Each internal request carries a `purpose` that the router uses for dispatch:

| Purpose | Slug | Description |
|---|---|---|
| Main loop | `main_loop` | Primary conversational turns |
| Summarization | `summarization` | Context compaction summaries |
| Fast classifier | `fast_classifier` | Auto-mode permission decisions |
| Sub-agent | `sub_agent` | Spawned sub-agent loops |
| Embedding | `embedding` | Embedding / memory retrieval |
| Other | `other` | Requests that don't fit a category |

## Enabling the router

Router config lives in the [settings layer](../reference/settings-schema.md#router): add a `[router]` section to `.caliban/settings.toml` (project or user scope) and it flows through the normal settings precedence, showing up in `caliban config print` with its source scope like any other key.

You can also point directly at a standalone `caliban.toml` file, which is the **highest-precedence** source and overrides the settings `[router]` section:

```bash
caliban --config /path/to/caliban.toml "my prompt"
# or via env var:
CALIBAN_ROUTER_CONFIG=/path/to/caliban.toml caliban "my prompt"
```

Resolution order (highest priority first): `--config` flag / `CALIBAN_ROUTER_CONFIG` → settings-layer `[router]` → single-provider fallback.

```admonish warning title="Discovery removed (#699)"
Earlier versions auto-discovered a `caliban.toml` by walking up from the current directory to the git root / `$HOME` and then checking `~/.config/caliban/caliban.toml`. That implicit discovery was **removed** — a repo-root `caliban.toml` is no longer picked up automatically. Migrate it into the settings layer with `caliban config import-router`, or pass it explicitly with `--config`.
```

## Basic configuration

A minimal `caliban.toml` with two purpose-keyed routes:

```toml
[router]
default_purpose = "main_loop"

[[router.route]]
purpose = "main_loop"
provider = "anthropic"
model = "claude-opus-4-7"

[[router.route]]
purpose = "fast_classifier"
provider = "openai"
model = "mlx-community/Qwen3.6-27B-4bit"
```

Valid `provider` values: `anthropic`, `openai`, `google`.

```admonish tip title="Local models: use `openai` + base_url"
For a local model, use `provider = "openai"` with a `[provider.openai] base_url` pointing at your engine's `/v1` endpoint. The route's `model` is sent to the server verbatim, so it must be a name the backend accepts — for `mlx_lm.server` that means the exact Hugging Face repo id. See [Local Inference](./local-inference.md). (The bespoke `ollama` provider was removed in [ADR 0056](../adr/README.md).)
```

## Provider blocks

Override the API key env var or base URL for a provider. In the settings layer these nest under `[router.provider.X]`; in a standalone `caliban.toml` they are top-level `[provider.X]` blocks (`caliban config import-router` relocates them automatically):

```toml
# .caliban/settings.toml
[router.provider.openai]
api_key_env = "OPENAI_API_KEY_STAGING"
base_url = "https://oai-staging.example.com/v1"

[router.provider.google]
base_url = "https://gemini-proxy.example.com/v1beta"
```

```toml
# standalone caliban.toml (passed via --config)
[provider.openai]
api_key_env = "OPENAI_API_KEY_STAGING"
base_url = "https://oai-staging.example.com/v1"
```

For a local model, point `[provider.openai].base_url` at your engine's `/v1`
endpoint (e.g. `http://gpu-server.local:9292/v1`) — no API key required. See
[Local Inference](./local-inference.md).

## Fallback chains

When a route fails with a retriable error (rate-limit, model unavailable, network timeout, server error), the router tries the next route for the same purpose. Define an explicit ordered fallback list, or let declaration order in the file serve as the implicit chain:

```toml
[[router.route]]
id = "main-primary"
purpose = "main_loop"
provider = "anthropic"
model = "claude-opus-4-7"
fallback = ["main-fallback"]    # explicit: only try this specific route next

[[router.route]]
id = "main-fallback"
purpose = "main_loop"
provider = "openai"
model = "gpt-5.5"
```

Set `fallback = []` to disable fallback entirely for a route.

Errors that are **not** retriable (auth failure, content policy, invalid request, cancellation) propagate immediately without trying another route.

## Hedging

Hedging races a second route against the primary after a configurable delay. The first to respond wins; the other is cancelled. This is a spend-for-latency trade-off and must be opted in explicitly:

```toml
[[router.route]]
purpose = "main_loop"
provider = "anthropic"
model = "claude-sonnet-4-6"
hedge = { hedge_after_ms = 1000, max = 1 }
```

A global default applies to all routes in the file:

```toml
[router.hedge]
hedge_after_ms = 1500
max_hedges = 1
```

Set `hedge = false` on a route to disable the global default for that route.

```admonish warning title="Hedging doubles costs"
Every hedged request that wins incurs a full charge on the winning route and a partial charge on the losing route for tokens sent before cancellation. Enable hedging only on routes where the latency benefit justifies the extra spend.
```

## Circuit breakers

A circuit breaker tracks failures per route and temporarily stops routing to a route that is consistently failing. Once the cool-off window passes, the breaker enters a half-open state and probes the route before fully reopening.

```toml
[router.breaker]           # global defaults
failure_threshold = 5      # trip after 5 failures within the window
window_secs = 60
cooldown_secs = 30
half_open_probes = 1

[[router.route]]
purpose = "main_loop"
provider = "anthropic"
model = "claude-sonnet-4-6"
breaker = false            # disable the global breaker for this route
```

Per-route breaker overrides can supply any subset of the fields; the rest inherit the global defaults. Cancellation outcomes do not count as failures.

## Capability filters

Routes can declare capability requirements. The router only sends a request to a route if the request's needs satisfy the route's declared capabilities:

```toml
[[router.route]]
purpose = "main_loop"
provider = "anthropic"
model = "claude-sonnet-4-6"
requires = { vision = true, tool_use = true }
```

The router also derives needs automatically from the request content (image blocks → vision need, tool declarations → tool-use need, thinking budget → thinking need), so you do not need to annotate every route manually.

## Effort levels

Set a default effort level on a route and optionally map each level to a provider-specific knob string:

```toml
[[router.route]]
purpose = "main_loop"
provider = "anthropic"
model = "claude-sonnet-4-6"
effort = "medium"

[router.route.effort_map]
low    = "budget=1024"
medium = "budget=8192"
high   = "budget=32768"
```

Valid effort levels: `low`, `medium` (default), `high`. Callers that don't specify an effort level inherit the route's default; the route default falls back to `medium`.

## Diagnosing the router

Use `caliban router debug` to print the candidate list the router would resolve for a synthetic request, including breaker state and effort knobs:

```bash
# Default: main_loop purpose, no special needs
caliban router debug

# Simulate a vision + tool request
caliban router debug --purpose main_loop --has-vision --has-tools

# Show the effort table for a high-effort request
caliban router debug --effort high

# Point at a specific config file
caliban --config ./caliban.toml router debug --purpose summarization
```

The output shows each route with a `+` (kept) or `-` (dropped) marker, the reason it was kept or dropped, and the current circuit-breaker state.

```mermaid
flowchart LR
    R["Request\n(purpose + needs)"] --> Res["Resolve candidates\n(purpose filter →\ncapability filter →\nbreaker filter)"]
    Res --> D{"Dispatch"}
    D -- "success" --> Resp["Response"]
    D -- "retriable error" --> F["Next candidate\n(fallback chain)"]
    F --> D
    D -- "hedge delay" --> H["Hedge race\n(first wins)"]
    H --> Resp
```
