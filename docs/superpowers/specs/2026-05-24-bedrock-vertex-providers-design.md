# Bedrock + Vertex providers — Design

**Date:** 2026-05-24
**Author:** john.ford2002@gmail.com
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** `docs/adr/0034-bedrock-and-vertex-providers.md`

## Goal

Ship two new caliban provider crates targeting Anthropic Claude models
served by hyperscaler endpoints:

1. **`caliban-provider-bedrock`** — Claude on AWS Bedrock.
2. **`caliban-provider-vertex`** — Claude on Google Vertex AI.

Both implement `caliban_provider::Provider`. Both reuse the existing
`caliban-provider-anthropic` message-format adapter (the IR-to-native
conversion is identical; only auth, endpoint, and streaming envelope
differ). They register cleanly with `caliban-model-router` so operators
can mix Bedrock-served Sonnet with direct-Anthropic Opus in the same
purpose-keyed config.

## Non-goals

- **No new IR / message-format work.** Bedrock + Vertex speak the same
  Anthropic native schema as direct Anthropic; the IR translation in
  `caliban-provider-anthropic::ir_convert` is reused as-is.
- **No Foundry provider.** Azure / Foundry remains 🔴 in the matrix
  and ships as a separate effort.
- **No Bedrock / Vertex models from non-Anthropic publishers** (Llama,
  Mistral, Cohere on Bedrock; Gemini on Vertex). The Anthropic
  publisher endpoints are the in-scope surface.
- **No custom inference-profile management UI.** Operators configure
  inference profiles in the AWS/GCP console; caliban only consumes
  them.

## Architecture

```
caliban binary
  ProviderFactory
    ├── caliban-provider-anthropic   (direct https)
    ├── caliban-provider-openai
    ├── caliban-provider-google
    ├── caliban-provider-ollama
    ├── caliban-provider-bedrock     ← new
    └── caliban-provider-vertex      ← new
                  │                  │
                  ▼                  ▼
  ┌──────────────────────────┐  ┌─────────────────────────────┐
  │ BedrockProvider          │  │ VertexProvider              │
  │   wraps AnthropicProvider│  │   wraps AnthropicProvider   │
  │   <BedrockTransport>     │  │   <VertexTransport>         │
  │   + AuthRefresh task     │  │   + AuthRefresh task        │
  │   + ListInferenceProfiles│  │   + Publishers/Anthropic    │
  │   for list_models()      │  │   for list_models()         │
  └──────────────────────────┘  └─────────────────────────────┘
                  │                  │
                  ▼                  ▼
       aws-sdk-bedrockruntime    reqwest + gcp_auth bearer
       (SigV4 via aws-config)    (PEM/JWT → OAuth token)

       Streaming:                Streaming:
         application/vnd.        Server-Sent Events
         amazon.eventstream      (`event:`/`data:` framing)
         (smithy event-stream)   wrapping Anthropic deltas
```

Each provider crate is a thin wrapper around
`caliban_provider_anthropic::AnthropicProvider<T>` where `T` is the
existing transport implementation. The new crates own:

1. **Configuration** — env reading + the `aws-config` / `gcp_auth`
   credential-provider plumbing.
2. **Auth refresh** — a background tokio task that refreshes
   short-lived credentials before they expire.
3. **`list_models`** — calling `bedrock:ListInferenceProfiles` /
   `publishers/anthropic/models` and caching the result for the session.
4. **Provider naming** — `name() -> "bedrock"` and `"vertex"` so the
   router and telemetry attribute them correctly.

## Crate structure (delta)

```
crates/caliban-provider-bedrock/
├── Cargo.toml
└── src/
    ├── lib.rs               # BedrockProvider impl Provider
    ├── config.rs            # BedrockConfig (region, inference profile, sdk_config)
    ├── auth.rs              # AuthRefresh task (AWS credential cache)
    ├── models.rs            # list_models via ListInferenceProfiles
    └── error.rs

crates/caliban-provider-vertex/
├── Cargo.toml
└── src/
    ├── lib.rs               # VertexProvider impl Provider
    ├── config.rs            # VertexConfig (project, region, credentials)
    ├── auth.rs              # AuthRefresh task (gcp_auth bearer cache)
    ├── models.rs            # list_models via publishers/anthropic
    └── error.rs
```

### Cargo dependencies

```toml
# caliban-provider-bedrock/Cargo.toml
[dependencies]
caliban-provider-anthropic = { workspace = true, features = ["bedrock"] }
caliban-provider           = { workspace = true }
aws-config                 = { workspace = true }
aws-sdk-bedrockruntime     = { workspace = true }
aws-sdk-bedrock            = "1"               # ListInferenceProfiles
aws-smithy-types           = { workspace = true }
async-trait                = { workspace = true }
tokio                      = { workspace = true }
tracing                    = { workspace = true }
thiserror                  = { workspace = true }

# caliban-provider-vertex/Cargo.toml
[dependencies]
caliban-provider-anthropic = { workspace = true, features = ["vertex"] }
caliban-provider           = { workspace = true }
gcp_auth                   = { workspace = true }
reqwest                    = { workspace = true }
serde                      = { workspace = true, features = ["derive"] }
serde_json                 = { workspace = true }
async-trait                = { workspace = true }
tokio                      = { workspace = true }
tracing                    = { workspace = true }
thiserror                  = { workspace = true }
```

## Configuration

### Bedrock

```toml
# ~/.config/caliban/providers.toml
[bedrock]
region                  = "us-west-2"              # else $AWS_REGION
inference_profile_id    = "us.anthropic.claude-opus-4-7-bedrock-20260423"
# optional explicit credentials path; default uses the AWS provider chain
# (env vars → ~/.aws/credentials → IMDS / SSO / web-identity)
profile                 = "caliban-prod"           # else $AWS_PROFILE
endpoint_override       = "https://bedrock-runtime.us-west-2.amazonaws.com"  # rarely needed
aws_auth_refresh        = "5m"                     # background refresh interval
```

Env-var fallbacks (read by `aws-config` automatically):

| Env var                          | Effect |
| -------------------------------- | ------ |
| `AWS_REGION` / `AWS_DEFAULT_REGION` | region                                            |
| `AWS_PROFILE`                    | named profile in `~/.aws/credentials`               |
| `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN` | static creds       |
| `AWS_WEB_IDENTITY_TOKEN_FILE` / `AWS_ROLE_ARN` | IRSA / IMDSv2                         |
| `BEDROCK_INFERENCE_PROFILE_ID`   | inference profile (overrides config file)         |
| `CALIBAN_AWS_AUTH_REFRESH`       | refresh interval (`5m` default; `0` = disabled)   |

### Vertex

```toml
[vertex]
project_id              = "my-gcp-project"          # else $VERTEX_PROJECT_ID
region                  = "us-east5"                # else $VERTEX_REGION
credentials_path        = "/etc/caliban/gcp-sa.json"  # else $GOOGLE_APPLICATION_CREDENTIALS
gcp_auth_refresh        = "5m"
publisher               = "anthropic"               # immutable; documented but not configurable
```

Env-var fallbacks:

| Env var                            | Effect |
| ---------------------------------- | ------ |
| `VERTEX_PROJECT_ID`                | GCP project                                       |
| `VERTEX_REGION`                    | Vertex region (`us-east5`, `europe-west1`, …)    |
| `GOOGLE_APPLICATION_CREDENTIALS`   | service-account JSON path                         |
| `GOOGLE_CLOUD_PROJECT`             | fallback for `VERTEX_PROJECT_ID`                  |
| `CALIBAN_GCP_AUTH_REFRESH`         | refresh interval                                  |

`gcp_auth` natively chooses between SA-JSON, ADC, GCE metadata server,
and `gcloud` user creds; we expose no explicit knobs beyond
`credentials_path`.

## Model-id format

| Provider | Wire model id format | Example |
| -------- | --- | --- |
| Bedrock  | `anthropic.<base-model>-bedrock-<release-date>` or an `inferenceProfileArn` | `anthropic.claude-opus-4-7-bedrock-20260423` |
| Vertex   | `<base-model>@<version>` (always `@`-suffixed) | `claude-opus-4-7@20260423` |

Both crates use `Transport::wire_model_id` (already defined in
`caliban-provider-anthropic`) to translate caliban's canonical
`claude-opus-4-7` ID into the platform-specific form. The
`canonical_model` typed into `caliban.toml` or passed via `--model`
stays clean; the router emits the canonical name and the transport
rewrites it inside `complete` / `stream`.

The wire mapping is deterministic and table-driven:

```rust
// crates/caliban-provider-bedrock/src/models.rs (sketch)
fn canonical_to_bedrock(canonical: &str) -> String {
    // claude-opus-4-7  -> anthropic.claude-opus-4-7-bedrock-20260423
    // claude-haiku-4-7 -> anthropic.claude-haiku-4-7-bedrock-20260423
    // anthropic.<…>    -> passthrough (operator pre-mapped)
    if canonical.starts_with("anthropic.") || canonical.contains("arn:") {
        return canonical.to_string();
    }
    format!("anthropic.{canonical}-bedrock-{RELEASE_DATE}")
}
```

`RELEASE_DATE` is a per-base-model `&str` table updated when
Anthropic rotates model dates on Bedrock.

## Endpoint URL construction

### Bedrock

`aws-sdk-bedrockruntime` constructs the URL internally; we pass either:

- a `model_id` for on-demand throughput, or
- an `inference_profile_arn` for cross-region throughput.

Operator-supplied `endpoint_override` (rare; for FIPS or VPC endpoints)
is propagated via `aws_config::SdkConfig::endpoint_url`.

### Vertex

```
POST https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}:{op}
```

where `{op}` ∈ `{streamRawPredict, rawPredict}`. The existing
`VertexTransport::endpoint` in `caliban-provider-anthropic` already
emits this format; we keep that code path.

## Streaming envelope differences

### Bedrock — `application/vnd.amazon.eventstream`

Bedrock wraps Anthropic SSE deltas in AWS event-stream framing (length
prefix + header table + payload + CRC). `aws-sdk-bedrockruntime`
returns a `ResponseStream` of typed events
(`PayloadPart { bytes: Blob }`); each blob is one Anthropic SSE event's
`data:` payload. The existing `BedrockTransport::stream` unpacks the
event-stream and yields `bytes::Bytes` chunks that the shared
`stream_parse::map_sse_to_events` consumes unchanged.

### Vertex — Server-Sent Events with `:streamRawPredict`

Vertex serves the Anthropic-native SSE stream directly over HTTP;
`event:` and `data:` lines parse with the shared SSE parser. The only
delta is that an `error` event from Vertex's gateway is wrapped in a
JSON envelope `{"error": {...}}` rather than the Anthropic-shaped
error event — `VertexTransport` already normalizes this.

## Capabilities

Both providers mirror `caliban-provider-anthropic::models::capabilities_for`
for the matching base model. Specifically:

```rust
fn capabilities(&self, model: &str) -> Capabilities {
    let base = strip_platform_suffix(model);    // claude-opus-4-7
    caliban_provider_anthropic::models::capabilities_for(&base)
}
```

This means `max_input_tokens`, `supports_vision`, `supports_thinking`,
`prompt_caching`, `tool_use`, and `system_prompt_capability` are all
identical to direct Anthropic. If the hyperscaler later restricts a
feature (e.g. some Bedrock regions disable prompt caching), we'll add
per-platform feature subtraction in `caliban-provider-bedrock::models`.

## `list_models`

### Bedrock

```rust
async fn list_models(&self) -> Vec<ModelInfo> {
    self.cached_models
        .get_or_init(|| async {
            let client = aws_sdk_bedrock::Client::new(&self.sdk_config);
            let resp = client.list_inference_profiles()
                .type_equals(InferenceProfileType::Application)
                .send().await
                .map_err(|e| tracing::warn!(target: "caliban::provider", error = %e, "list_inference_profiles failed"))
                .ok();
            resp.map(...).unwrap_or_else(fallback_known_models)
        }).await
}
```

`aws-sdk-bedrock` (separate from `bedrockruntime`) provides the
control-plane API. Falls back to a vendored list of known
`anthropic.claude-*-bedrock-*` ids if the API call fails (e.g.
read-only IAM). The cache is per-provider-instance and per-process —
no on-disk cache.

### Vertex

```rust
async fn list_models(&self) -> Vec<ModelInfo> {
    self.cached_models
        .get_or_init(|| async {
            let url = format!(
                "https://{region}-aiplatform.googleapis.com/v1/publishers/anthropic/models",
                region = self.config.region,
            );
            // GET with bearer; parse `models[].name`
            …
        }).await
}
```

Same caching + fallback semantics as Bedrock.

## Auth refresh

Both crates spawn one background tokio task on construction:

```rust
// crates/caliban-provider-vertex/src/auth.rs (sketch)

pub struct AuthRefresh {
    handle:    JoinHandle<()>,
    cached:    Arc<RwLock<Token>>,
    cancel:    CancellationToken,
}

impl AuthRefresh {
    pub fn spawn(provider: Arc<dyn TokenProvider>, interval: Duration) -> Self {
        let cached = Arc::new(RwLock::new(provider.get_token().await?));
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(refresh_loop(provider, cached.clone(), cancel.clone(), interval));
        Self { handle, cached, cancel }
    }
    pub async fn token(&self) -> String { self.cached.read().await.access_token.clone() }
}
```

Default refresh interval is 5 minutes (controlled by
`caliban.toml [bedrock|vertex].{aws,gcp}_auth_refresh`). Refresh
failures log a `tracing::warn!` and retry with exponential backoff
capped at the configured interval; the cached token continues to be
served until it actually expires (then `complete` / `stream` will 401
and the AuthRefresh task is woken to refresh immediately).

For Bedrock, `aws-config` handles credential rotation internally for
IMDS / SSO / web-identity sources; `aws_auth_refresh` simply triggers
a fresh `SdkConfig::load()` on the interval to pick up any external
changes (e.g. profile credentials rotated by `aws sso login`). For
static `AWS_ACCESS_KEY_ID` credentials it's a no-op.

## Public API sketches

```rust
// crates/caliban-provider-bedrock/src/lib.rs

pub use config::BedrockConfig;
pub use error::BedrockError;

pub struct BedrockProvider {
    inner: AnthropicProvider<BedrockTransport>,
    auth:  AuthRefresh,
    models: tokio::sync::OnceCell<Vec<ModelInfo>>,
}

impl BedrockProvider {
    pub async fn from_env() -> Result<Self, BedrockError>;
    pub async fn from_config(cfg: BedrockConfig) -> Result<Self, BedrockError>;
}

#[async_trait]
impl Provider for BedrockProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        self.inner.complete(req).await   // request metadata.purpose flows through unchanged
    }
    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream> {
        self.inner.stream(req).await
    }
    fn capabilities(&self, model: &str) -> Capabilities { self.inner.capabilities(model) }
    fn list_models(&self) -> Vec<ModelInfo> { /* drains OnceCell */ }
    fn name(&self) -> &'static str { "bedrock" }
}
```

`VertexProvider` mirrors this exactly with the corresponding types.

## Router composition

Operator's `caliban.toml`:

```toml
[router]
default_purpose = "main_loop"

[[router.routes]]
purpose  = "main_loop"
provider = "bedrock"
model    = "claude-opus-4-7"           # canonical; transport rewrites to anthropic.…-bedrock-20260423

[[router.routes]]
purpose  = "summarization"
provider = "vertex"
model    = "claude-haiku-4-7"

[[router.routes]]
purpose  = "fast_classifier"
provider = "anthropic"
model    = "claude-haiku-4-7"
```

`RequestMetadata.purpose` flows through `caliban-model-router`
unchanged; the router picks a `(provider, model)` pair and dispatches.
The provider crates don't read `purpose` themselves — they treat every
request identically.

## Permission / safety surface

Both providers route through `caliban-tools-builtin` for tool calls
exactly like direct Anthropic; there's no platform-specific permission
plumbing. Operators using IAM-restricted Bedrock principals or Vertex
SA-bound projects get those restrictions for free at the cloud layer.

## Error handling

```rust
pub enum BedrockError {
    AwsConfig    { source: aws_config::ConfigError },
    Sdk          { source: aws_sdk_bedrockruntime::error::SdkError<…> },
    Auth         { source: aws_credential_types::Error },
    InvalidModel { model: String, reason: String },
    InvalidRegion{ region: String },
}

pub enum VertexError {
    Auth         { source: gcp_auth::Error },
    Http         { source: reqwest::Error },
    Json         { source: serde_json::Error },
    InvalidModel { model: String, reason: String },
    InvalidRegion{ region: String },
}
```

Both convert into `caliban_provider::Error::Adapter(Box<dyn Error>)`
when crossing the trait boundary.

Streaming errors (auth expiry mid-stream, transient 5xx) are mapped to
`MessageStream::Err(_)` events; the caller (`agent-core`) handles
backoff. We do *not* implement provider-side retry — that's the model
router's hedging job (ADR 0022 follow-up).

## Testing strategy

Each crate ships 8 tests for ~16 total:

### Bedrock (8)

1. `BedrockConfig::from_env` reads `AWS_REGION` + `BEDROCK_INFERENCE_PROFILE_ID`.
2. `canonical_to_bedrock` maps `claude-opus-4-7` → `anthropic.claude-opus-4-7-bedrock-20260423`.
3. `canonical_to_bedrock` passes through an `arn:aws:bedrock:...` ARN unchanged.
4. `complete` happy path against a `aws-sdk-bedrockruntime` mock (using `aws_smithy_runtime::client::http::test_util::StaticReplayClient`).
5. `stream` decodes an event-stream containing 3 `PayloadPart` blobs into the right sequence of Anthropic SSE deltas.
6. `list_models` returns the fallback list when `ListInferenceProfiles` returns AccessDenied.
7. `AuthRefresh` reload on configured interval (test-clock).
8. `name()` returns `"bedrock"`; `capabilities("claude-opus-4-7-bedrock-20260423")` matches direct-Anthropic Opus.

### Vertex (8)

9. `VertexConfig::from_env` reads `VERTEX_PROJECT_ID` + `VERTEX_REGION` + `GOOGLE_APPLICATION_CREDENTIALS`.
10. Wire-model: `claude-opus-4-7` → `claude-opus-4-7@20260423`.
11. Wire-model: passthrough when canonical already contains `@`.
12. `complete` happy path against a `wiremock` server returning canonical Anthropic JSON.
13. `stream` parses Vertex SSE `event:`/`data:` framing into the right delta sequence.
14. `stream` surfaces a Vertex gateway error event as `MessageStream::Err`.
15. `list_models` parses `publishers/anthropic/models` response shape.
16. `AuthRefresh` re-fetches bearer when token's `expires_in` < 60s.

Integration test (`tests/router_composition.rs`) constructs a
`ModelRouter` with one `bedrock` route and one `vertex` route, asserts
that `Provider::name()` and `request.metadata.purpose` flow end-to-end.

## Risks

- **AWS event-stream parsing** is owned by `aws-sdk-bedrockruntime`,
  which evolves at AWS pace. Pin minor version; treat upgrades as
  semver-cautious.
- **Vertex SSE error normalization** is undocumented; our adapter
  encodes the observed shapes. Mitigation: log unparseable events at
  `tracing::warn!` with the full body; revisit when Google publishes a
  spec.
- **Model-id mapping table** drifts when Anthropic rotates dates.
  Mitigation: a small const table in `models.rs` keyed by canonical
  name; PRs to bump dates are routine.
- **Bedrock cross-region inference profiles** require operators to
  pre-configure them in AWS; failure mode is opaque (`AccessDenied`
  vs. `ResourceNotFound`). Mitigation: README explicitly explains
  inference profiles + IAM requirements with a sample policy.
- **`gcp_auth` token caching** can hand back an expiring-very-soon
  token in a race with the refresh task. Mitigation: refresh when
  `now > expires_at - 60s`; tolerate one inflight 401 + retry.
- **Doubled CI matrix.** Two new providers means 2× the integration
  surface. Mitigation: integration tests use mocks (no live AWS/GCP);
  a one-off smoke-test job hits real endpoints on a release branch
  only.

## Acceptance criteria

- `cargo build --workspace` clean; `clippy --workspace --all-targets -- -D warnings` clean; `fmt --check` clean.
- 16 new tests passing (8 Bedrock + 8 Vertex) plus the
  router-composition integration test.
- `BedrockProvider::from_env()` succeeds with `AWS_REGION` +
  `BEDROCK_INFERENCE_PROFILE_ID` set against the AWS default
  credential chain.
- `VertexProvider::from_env()` succeeds with `VERTEX_PROJECT_ID` +
  `VERTEX_REGION` + `GOOGLE_APPLICATION_CREDENTIALS` set.
- `caliban-model-router` config example accepts `provider = "bedrock"`
  and `provider = "vertex"` in `[[router.routes]]`.
- `docs/parity-gap-matrix.md` rows under **I. Model router & providers**
  — `Bedrock` and `Vertex` — move 🔴 → ✅.
- README's new "Providers" section documents env vars + an example
  `caliban.toml` routing Sonnet via Bedrock and Haiku via Vertex.
- ADR 0034 in `accepted` status (this spec's prerequisite).
