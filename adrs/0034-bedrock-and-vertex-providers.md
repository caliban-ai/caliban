# ADR 0034 · Bedrock + Vertex providers

- **Status:** proposed
- **Date:** 2026-05-24
- **Author:** john.ford2002@gmail.com
- **Spec:** `docs/superpowers/specs/2026-05-24-bedrock-vertex-providers-design.md`

## Context

`caliban-provider-anthropic` already contains feature-gated
`BedrockTransport` and `VertexTransport` implementations (`bedrock` and
`vertex` Cargo features), plus the workspace already declares
`aws-config`, `aws-sdk-bedrockruntime`, `aws-smithy-types`, and
`gcp_auth` as dependencies in anticipation of this work. What's
missing is the top-level `Provider`-implementing crates that expose
these transports as first-class providers with their own `name()`,
their own `list_models` (which require control-plane APIs the
Anthropic crate has no business knowing about), and their own auth
refresh policy. Parity with Claude Code's `--bedrock` / `--vertex`
flags requires both crates.

## Decision

### Two new crates, both thin wrappers around the existing transports

`caliban-provider-bedrock` and `caliban-provider-vertex` each contain
~300 lines of glue:

1. A `Provider`-implementing struct wrapping
   `AnthropicProvider<BedrockTransport>` or
   `AnthropicProvider<VertexTransport>`.
2. A `*Config` struct + `from_env` / `from_config` constructors.
3. An `AuthRefresh` background task.
4. A `list_models` that hits the relevant control-plane API
   (`bedrock:ListInferenceProfiles` / `publishers/anthropic/models`),
   caches the result for the session, and falls back to a vendored
   list on failure.
5. A `name()` returning `"bedrock"` / `"vertex"` so the model router
   and telemetry attribute these correctly.

We *do not* extend `caliban-provider-anthropic` to expose Bedrock /
Vertex as alternate constructors because (a) it would force the
Anthropic crate to depend on `aws-sdk-bedrock` (control plane) and
gain its own non-trivial auth code, and (b) operators have a real
mental-model expectation that `provider = "bedrock"` and
`provider = "anthropic"` are separate provider entries.

### Auth refresh is a per-provider tokio task with a 5-minute default

Both crates spawn one background task on construction that calls
`provider.get_token()` (via `aws-config`'s `ProvideCredentials` or
`gcp_auth`'s `TokenProvider`) on a configurable interval. Settings
fields `aws_auth_refresh` and `gcp_auth_refresh` (and env
`CALIBAN_AWS_AUTH_REFRESH` / `CALIBAN_GCP_AUTH_REFRESH`) control the
interval; default `5m`; `0` disables proactive refresh and relies on
inline 401 recovery only. Refresh failures back off exponentially up
to the configured interval and surface as `tracing::warn!` until they
succeed; the cached token continues to be served until it expires.

### Model-id canonicalization stays in `caliban-provider-anthropic`

`Transport::wire_model_id` already lives in the Anthropic crate. The
new provider crates expose a small per-base-model release-date table
(e.g. `("claude-opus-4-7", "20260423")`) consumed by the transport's
`wire_model_id`. The caliban canonical model name (`claude-opus-4-7`)
remains the same across Anthropic / Bedrock / Vertex — only the wire
form differs.

### `Capabilities` mirror direct Anthropic per base model

The hyperscalers serve the same Anthropic models with the same context
windows, vision support, and tool-use semantics. Until a real
discrepancy emerges (e.g. some regions lacking prompt caching), both
crates' `capabilities()` strip the platform suffix and delegate to
`caliban_provider_anthropic::models::capabilities_for`. Any future
regional / platform restriction is added as a small subtraction layer
on top — not by forking the capabilities table.

### `list_models` is on-demand + per-session-cached, with fallback

We resist the temptation to call `list_inference_profiles` at provider
startup because (a) startup latency is precious and (b) operators with
read-restricted IAM principals shouldn't fail startup just because
they can't introspect. Both crates call the control-plane API the
first time `list_models` is invoked, cache the result in a
`tokio::sync::OnceCell`, and fall back to a vendored list of
well-known models if the API call fails.

### Request metadata flows through unchanged

`RequestMetadata.purpose`, `user_id`, and any future fields pass
through both crates untouched into the transport into the wire body.
The provider crates own auth + endpoint + list_models — not request
shape.

## Consequences

- **Positive:** Closes two 🔴 rows under I. Model router & providers
  (`Bedrock`, `Vertex`). Enables operators in regulated industries
  (financial services, healthcare, gov) to use caliban with their
  contractual cloud provider. Composes cleanly with
  `caliban-model-router` so the same operator can route Sonnet via
  Bedrock for compliance and Haiku via direct Anthropic for cost.
  Reuses the Anthropic IR adapter so the message-shape correctness
  surface stays single-sourced.
- **Negative:** Adds two new crates to the workspace; the `aws-*`
  dependency tree is heavy (~30 transitive crates, mostly hyper/tower
  stack). Bedrock model-id rotation (Anthropic occasionally re-dates
  Bedrock models without changing direct-API names) requires
  per-base-model date-table maintenance. Two new mock-based test
  surfaces to maintain.
- **Revisit if:** AWS or GCP changes the canonical wire format
  significantly (e.g. Bedrock unifies under inference-profile ARNs
  exclusively), in which case the canonical→wire mapping simplifies.
  If `caliban-provider-anthropic`'s embedded `bedrock` / `vertex`
  features turn out to be confusing duplicate paths, deprecate those
  feature flags in favor of the new crates and route all
  hyperscaler-served Anthropic through here.
