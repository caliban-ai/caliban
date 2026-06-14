# Layer 1 / B · Provider Abstraction · Design

- **Date:** 2026-05-22
- **Status:** Draft (pending implementation plan)
- **Sub-project of:** caliban Rust agent harness
- **Depends on:** Layer 0 (workspace skeleton + ADRs)
- **Next sub-project:** Layer 2 / C (memory architecture) or Layer 3 / D (model router)

## Goals

Define caliban's provider-neutral message and streaming model, plus four schema-family adapter crates that implement it. After B lands, a consumer can write:

```rust
let provider: Box<dyn Provider + Send + Sync> = match cfg.kind {
    Kind::AnthropicDirect => Box::new(AnthropicProvider::direct(DirectConfig::from_env()?)),
    Kind::Bedrock         => Box::new(AnthropicProvider::bedrock(BedrockConfig::from_aws_credentials().await?)),
    Kind::Vertex          => Box::new(AnthropicProvider::vertex(VertexConfig::from_gcp_credentials(...).await?)),
    Kind::OpenAI          => Box::new(OpenAIProvider::direct(DirectConfig::from_env()?)),
    Kind::Azure           => Box::new(OpenAIProvider::azure(AzureConfig::from_env()?)),
    Kind::Ollama          => Box::new(OllamaProvider::direct(DirectConfig::local())),
    Kind::Gemini          => Box::new(GoogleProvider::ai_studio(AIStudioConfig::from_env()?)),
    Kind::VertexGemini    => Box::new(GoogleProvider::vertex(VertexConfig::from_gcp_credentials(...).await?)),
};
let resp = provider.complete(req).await?;
```

…and get back a fully-populated `Message` regardless of which (schema, transport) pair is wired up.

## Non-goals (explicit deferrals)

- Unified config file (`~/.config/caliban/...`) — own sub-project; B ships per-adapter `Config` + `from_env()` only.
- Bedrock model families other than Anthropic Claude — separate work if/when wanted.
- Azure OpenAI Entra ID (Azure AD) OAuth auth — API-key auth only for v1; OAuth is a follow-on.
- Image / audio / video output — strictly chat-completion text + multimodal *input* (with text + tool_use output) for B.
- Model-router (Layer 3) — picks which provider/model for a task. B exposes capabilities; routing logic is elsewhere.
- Retry / rate-limit backoff in adapters — `Provider` returns categorized errors; retry/backoff lives in the router or caller.
- Cost/usage aggregation across calls — `Usage` is on each response; tallying is the caller's job.
- Dynamic model-list refresh — capability data is static const tables per adapter; an `async refresh_models()` method is a future enhancement.

## The two orthogonal dimensions

caliban's adapter design factors providers into:

| Schema family | Direct API | AWS Bedrock | Google Vertex AI | Azure OpenAI |
|---|---|---|---|---|
| Anthropic Claude | api.anthropic.com | SigV4 + Bedrock endpoint | GCP OAuth + Vertex endpoint | — |
| OpenAI | api.openai.com | — | — | `<resource>.openai.azure.com` |
| Gemini | Google AI Studio | — | Vertex AI | — |
| OpenAI-compat (local) | Ollama (localhost) | — | — | — |

The schema family owns the wire-format serialization, IR conversions, streaming-event parsing, and `Provider` impl. The transport handles auth, URL construction, request signing, and response framing. This means Claude on Bedrock reuses `caliban-provider-anthropic`'s schema work and adds only the SigV4/event-stream specifics.

## Crate structure

```
crates/
├── caliban-provider/                  # Trait crate
├── caliban-provider-anthropic/        # Claude schema family
├── caliban-provider-openai/           # OpenAI schema family
├── caliban-provider-ollama/           # Ollama API
└── caliban-provider-google/           # Gemini schema family
```

**Cargo features:**
- `caliban-provider-anthropic`: `bedrock` (pulls in `aws-sdk-bedrockruntime`), `vertex` (pulls in `google-cloud-auth`).
- `caliban-provider-openai`: `azure` (no extra heavy dep — Azure auth is API-key-only initially).
- `caliban-provider-google`: `vertex` (pulls in `google-cloud-auth`).
- Default features: just the direct transport (or `AIStudioTransport` for Google).

**Dependency graph:**
```
caliban-core ◄── caliban-provider ◄── caliban-provider-anthropic
                                  ◄── caliban-provider-openai
                                  ◄── caliban-provider-ollama
                                  ◄── caliban-provider-google
```

No adapter depends on another. `caliban-provider` is the trait + IR + Error + Capabilities only.

## File layout per crate

### `caliban-provider/`
- `src/lib.rs` — re-exports
- `src/message.rs` — `Message`, `Role`, `ContentBlock`, `TextBlock`, `ImageBlock`, `ImageSource`
- `src/tool.rs` — `Tool`, `ToolUseBlock`, `ToolResultBlock`, `ToolChoice`
- `src/request.rs` — `CompletionRequest`, builder, validation
- `src/response.rs` — `CompletionResponse`, `Usage`, `StopReason`
- `src/stream.rs` — `StreamEvent`, `StreamingContentType`, `StreamingDelta`, `MessageStream`
- `src/capabilities.rs` — `Capabilities`, `ToolUseCapability`, `PromptCachingCapability`, `SystemPromptCapability`, `ModelInfo`
- `src/error.rs` — `Error`, `Result`
- `src/provider.rs` — `Provider` trait, `#[async_trait]`
- `src/thinking.rs` — `ThinkingBlock`, `ThinkingConfig`
- `src/cache.rs` — `CacheControl`

### Each adapter crate (uniform shape)
- `src/lib.rs` — re-exports + `XxxProvider<T: Transport>`
- `src/schema/` — native request/response/event types (serde)
- `src/ir_convert.rs` — IR ↔ native conversions
- `src/stream_parse.rs` — wire-format → `StreamEvent`
- `src/transport/mod.rs` — `Transport` trait
- `src/transport/direct.rs` — default transport
- `src/transport/<cloud>.rs` — feature-gated transports
- `src/config.rs` — per-transport `Config` structs, `from_env()` impls
- `src/models.rs` — const `ModelInfo` table

## Public API — IR types

### Messages and content

```rust
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

pub enum Role { User, Assistant, System }

pub enum ContentBlock {
    Text(TextBlock),
    Image(ImageBlock),
    ToolUse(ToolUseBlock),
    ToolResult(ToolResultBlock),
    Thinking(ThinkingBlock),
}

pub struct TextBlock {
    pub text: String,
    pub cache_control: Option<CacheControl>,
}

pub struct ImageBlock {
    pub source: ImageSource,
    pub cache_control: Option<CacheControl>,
}

pub enum ImageSource {
    Base64 { media_type: String, data: String },  // image/png, image/jpeg, image/webp, image/gif
    Url(String),
}

pub struct ToolUseBlock {
    pub id: String,                       // adapter-assigned; round-trip-preserved
    pub name: String,
    pub input: serde_json::Value,
}

pub struct ToolResultBlock {
    pub tool_use_id: String,
    pub content: Vec<ContentBlock>,       // Text or Image (Anthropic-only)
    pub is_error: bool,
}

pub struct ThinkingBlock {
    pub thinking: String,
    pub signature: Option<String>,        // Anthropic redacted-thinking signature for replay
}

pub enum CacheControl { Ephemeral }       // only variant currently supported by Anthropic
```

**Role semantics.** Three roles: `User`, `Assistant`, `System`. System messages must appear contiguously at the start of `CompletionRequest.messages` and may only contain `Text` content blocks (with optional `cache_control`). Validation rejects out-of-order System messages, System messages with non-text content, and requests without at least one User or Assistant message.

### Tools

```rust
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,  // JSON Schema draft-07
    pub cache_control: Option<CacheControl>,
}

pub enum ToolChoice {
    Auto,
    Any,                                  // force a tool call (Anthropic + OpenAI; emulated where needed)
    Specific(String),                     // call this specific tool by name
    None,                                 // disable tool use this turn
}
```

JSON schema is `serde_json::Value` — providers all accept JSON Schema; no caliban-imposed Rust type so schema evolution stays flexible.

### Request

```rust
pub struct CompletionRequest {
    pub model: String,                    // canonical name; transports may translate to wire format
    pub messages: Vec<Message>,           // leading Role::System messages are the system prompt
    pub tools: Vec<Tool>,
    pub tool_choice: ToolChoice,
    pub max_tokens: u32,                  // required
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,               // Anthropic-only; capability-gated
    pub stop_sequences: Vec<String>,
    pub thinking: Option<ThinkingConfig>,
    pub metadata: RequestMetadata,
}

pub struct ThinkingConfig { pub budget_tokens: u32 }
pub struct RequestMetadata { pub user_id: Option<String> }
```

A `CompletionRequest::builder()` fluent API provides defaults:

```rust
let req = CompletionRequest::builder("claude-3-5-sonnet")
    .system("You are a helpful assistant.")
    .user_text("Hello!")
    .max_tokens(1024)
    .build()?;
```

### Response

```rust
pub struct CompletionResponse {
    pub id: String,
    pub model: String,                    // server-reported (may differ from request — aliases, defaulting)
    pub message: Message,                 // role: Assistant
    pub stop_reason: StopReason,
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    ContentFilter,
    Refusal,
}

pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: Option<u32>,
    pub cache_read_input_tokens: Option<u32>,
}
```

### Streaming

```rust
pub enum StreamEvent {
    MessageStart { id: String, model: String },
    ContentBlockStart { index: u32, content_type: StreamingContentType },
    Delta { index: u32, delta: StreamingDelta },
    ContentBlockStop { index: u32 },
    MessageDelta { stop_reason: Option<StopReason>, usage_delta: Option<Usage> },
    MessageStop,
    Ping,
}

pub enum StreamingContentType {
    Text,
    ToolUse { id: String, name: String },
    Thinking,
    Image,                                // reserved
}

pub enum StreamingDelta {
    Text(String),
    ToolUseInputJson(String),
    Thinking(String),
}

pub struct MessageStream { /* ... */ }    // implements Stream<Item = Result<StreamEvent>>

impl MessageStream {
    pub async fn collect_message(self) -> Result<(Message, StopReason, Usage)>;
}
```

### Error

```rust
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("rate limit exceeded (retry after {retry_after:?})")]
    RateLimit { retry_after: Option<Duration> },

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("context too long: requested {requested_tokens} but max is {max_tokens}")]
    ContextTooLong { max_tokens: u32, requested_tokens: u32 },

    #[error("model unavailable: {0}")]
    ModelUnavailable(String),

    #[error("server error (HTTP {status}): {body}")]
    ServerError { status: u16, body: String },

    #[error("content filter triggered: {0}")]
    ContentFilter(String),

    #[error("network error: {0}")]
    Network(Box<dyn std::error::Error + Send + Sync>),

    #[error("operation cancelled")]
    Cancelled,

    #[error("adapter error: {0}")]
    Adapter(#[source] Box<dyn std::error::Error + Send + Sync>),
}

pub type Result<T> = std::result::Result<T, Error>;
```

The `Adapter` variant carries adapter-specific information that doesn't fit a standard category (e.g., a Bedrock-specific validation error code) so callers can downcast if they need detail. Most callers should match on the categorized variants.

### Capabilities

```rust
pub struct Capabilities {
    pub max_input_tokens: u32,
    pub max_output_tokens: u32,
    pub vision: bool,
    pub tool_use: ToolUseCapability,
    pub thinking: bool,
    pub prompt_caching: PromptCachingCapability,
    pub json_mode: bool,
    pub streaming: bool,
    pub stop_sequences: bool,
    pub top_k: bool,
    pub system_prompt: SystemPromptCapability,
    pub refusal_field: bool,
}

pub enum ToolUseCapability { None, Basic, ParallelCalls }
pub enum PromptCachingCapability {
    None,
    Automatic,
    Explicit { max_breakpoints: u32 },
}
pub enum SystemPromptCapability { SeparateField, SystemRole, DeveloperRole }

pub struct ModelInfo {
    pub id: String,                       // canonical
    pub native_id: String,                // wire format
    pub display_name: String,
    pub capabilities: Capabilities,
}
```

### The `Provider` trait

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse>;
    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream>;
    fn capabilities(&self, model: &str) -> Capabilities;
    fn list_models(&self) -> Vec<ModelInfo>;
    fn name(&self) -> &'static str;       // "anthropic", "openai", "ollama", "google", etc.
}
```

Object-safe — no generics on methods, no `Self: Sized` bounds, no associated types. `Box<dyn Provider + Send + Sync>` works for collections and dynamic dispatch.

## Transport trait pattern

Each schema-family crate defines its own `Transport` trait (not exported from `caliban-provider`). Shape:

```rust
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    async fn send(&self, body: NativeRequest) -> Result<NativeResponse, TransportError>;
    async fn stream(
        &self,
        body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<NativeEvent, TransportError>>, TransportError>;
    fn wire_model_id(&self, canonical: &str) -> String;
    fn finalize_request(&self, body: &mut NativeRequest);
}
```

The schema crate's `XxxProvider<T: Transport>` is generic over the transport; per-transport convenience constructors (`Provider::direct(cfg)`, `Provider::bedrock(cfg)`, etc.) build the concrete type.

## Adapter-specific translation rules

### `caliban-provider-anthropic`

- Leading `Role::System` messages → concatenated into `system: "..."` *or*, if any block has `cache_control`, serialized as `system: [TextBlock, ...]` array.
- `ContentBlock` enum → Anthropic's native union (text, image, tool_use, tool_result, thinking).
- `ToolUseBlock.input` is `serde_json::Value` → serialized inline.
- `cache_control: Ephemeral` → `{"type": "ephemeral"}`.
- `ThinkingConfig` → `thinking: { type: "enabled", budget_tokens: ... }`.
- Streaming SSE → `StreamEvent` map (1:1 — Anthropic's event names are nearly identical to ours by design).
- **`DirectTransport`** — `https://api.anthropic.com`; headers `x-api-key`, `anthropic-version`, `content-type: application/json`.
- **`BedrockTransport` (feat=bedrock)** — `https://bedrock-runtime.{region}.amazonaws.com/model/{model_id}/invoke[-with-response-stream]`; SigV4-signed; `anthropic_version: "bedrock-2023-05-31"` injected into body; `wire_model_id`: `claude-3-5-sonnet-20240620` → `anthropic.claude-3-5-sonnet-20240620-v1:0`; streaming uses AWS event-stream framing (parsed into the same `StreamEvent`s).
- **`VertexTransport` (feat=vertex)** — `https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}:rawPredict`; GCP OAuth token in `Authorization: Bearer`; same body shape as direct minus `model` field (it's in the URL).

### `caliban-provider-openai`

- Leading `Role::System` → `{"role": "system", "content": "..."}` messages (or `"role": "developer"` for o1+ models per `Capabilities::SystemPromptCapability`).
- `ContentBlock::Image` → `{"type": "image_url", "image_url": {"url": ...}}` for URL; `{"type": "image_url", "image_url": {"url": "data:{mime};base64,{data}"}}` for Base64.
- `ToolUseBlock` → `tool_calls: [{"id": ..., "type": "function", "function": {"name": ..., "arguments": ...}}]`.
- `ToolResultBlock` → message with `role: "tool"`, `tool_call_id: ...`, `content: ...`.
- `ThinkingBlock` is dropped on serialize (OpenAI doesn't expose reasoning); reasoning text on o1 responses is captured as a `Thinking` block on the IR response.
- `cache_control` is ignored on the wire (OpenAI is auto-cached); usage's `cache_read_input_tokens` is populated from `prompt_tokens_details.cached_tokens`.
- Streaming SSE → `StreamEvent` map: `choices[0].delta` content goes to `Delta::Text`; tool_call_deltas go to `Delta::ToolUseInputJson`.
- **`DirectTransport`** — `https://api.openai.com/v1`; `Authorization: Bearer {key}`.
- **`AzureTransport` (feat=azure)** — `https://{resource}.openai.azure.com/openai/deployments/{deployment}/chat/completions?api-version={ver}`; `api-key` header; `wire_model_id` consults `AzureConfig.deployments` HashMap; if missing → `Error::InvalidRequest("no deployment configured for model X")`.

### `caliban-provider-ollama`

- Talks to `/api/chat`. Schema is similar to OpenAI's chat completions with a few quirks (`options.num_predict` instead of `max_tokens`, no streaming `tool_calls` deltas in older versions).
- Streaming is newline-delimited JSON, not SSE — adapter handles framing.
- No auth.
- Tool use: supported on models with tool support (Llama 3.1+, etc.). Capability table reflects per-model.

### `caliban-provider-google`

- Leading `Role::System` → `systemInstruction: { parts: [{text: "..."}] }` (concatenated).
- `Role::User` → `{role: "user", parts: [...]}`. `Role::Assistant` → `{role: "model", parts: [...]}`.
- `ContentBlock::Text` → `{text: "..."}`. `ContentBlock::Image` → `{inlineData: {mimeType, data}}` (Base64) or `{fileData: {mimeType, fileUri}}` (URL, Vertex only; AI Studio requires Base64 inline).
- `ToolUseBlock` → `{functionCall: {name, args}}`. `ToolResultBlock` → `{functionResponse: {name, response}}`.
- `cache_control` is ignored (Gemini's "context caching" works differently — separate API for creating cached content; out of scope for B).
- Streaming is JSON-array chunks (Gemini's older protocol) or newline-delimited (newer endpoints) — adapter detects and handles both.
- **`AIStudioTransport`** — `https://generativelanguage.googleapis.com/{api_version}/models/{model}:streamGenerateContent?key={api_key}`.
- **`VertexTransport` (feat=vertex)** — `https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/google/models/{model}:streamGenerateContent`; GCP OAuth.

## Testing strategy

Three test tiers per adapter:

1. **Pure-unit tests on IR conversion** — round-trip IR ↔ native byte-equivalence on representable inputs. Always-on, hermetic.
2. **Fixture tests against mock HTTP** — `wiremock` serves recorded API responses. Tests verify the adapter's serialization matches recorded request expectations and that parsed responses produce the right IR. Always-on, hermetic. Catches API-shape regressions without live keys.
3. **Live integration tests** — `--features live-tests`, off by default. Each adapter has one or two smoke tests; runs only when `*_API_KEY` env vars are set. Manual or scheduled CI only. Total cost under $0.05 per full run with stub prompts.

Plus:

- **Streaming-event parser tests** per adapter — pump recorded byte streams (SSE, AWS event-stream, JSON-lines), verify `StreamEvent` sequence. Malformed-input handling (partial events, mid-stream disconnects, malformed JSON) tested from day one.
- **Property tests on IR types** — `proptest` round-trips. Random `Message` → JSON → `Message` equality.
- **`MockProvider`** (in `caliban-provider`, behind `mock` feature) — scripted responses for downstream consumer testing. Configurable response queue, error injection, streaming-event queue.

**CI matrix change:** Layer 0's single `cargo test --workspace --all-features` job becomes:
- Default job: `cargo test --workspace` (no cloud features) — fast, exercises direct transports.
- Cloud job: `cargo test --workspace --features bedrock,vertex,azure` — slower, only fires when cloud-transport files change (path filtering on `crates/caliban-provider-*/src/transport/{bedrock,vertex,azure}.rs` and feature-gate-affecting code).

## Acceptance criteria

Layer 1 / B is done when **all** of the following hold:

**Workspace structure**
- `crates/caliban-provider/`, `crates/caliban-provider-anthropic/`, `crates/caliban-provider-openai/`, `crates/caliban-provider-ollama/`, `crates/caliban-provider-google/` all exist as workspace members.
- Each adapter crate has its own `Cargo.toml` inheriting workspace metadata, declaring its own deps, with `[lints] workspace = true`.
- `cargo build --workspace` succeeds with default features.
- `cargo build --workspace --features bedrock,vertex,azure` succeeds.

**Trait crate (`caliban-provider`)**
- All IR types from this spec are defined and `Debug + Clone + Serialize + Deserialize`.
- `Provider` trait is object-safe (`Box<dyn Provider + Send + Sync>` compiles).
- `CompletionRequest::builder()` works end-to-end.
- Validation rejects out-of-order System messages, non-text System content, and empty/conversation-less requests with `Error::InvalidRequest`.
- `MockProvider` (feat=mock) lets callers script responses and streaming events.

**Each schema-family crate**
- Compiles with default features and with all crate-specific features.
- Implements `Provider` via the generic `XxxProvider<T: Transport>` + per-transport `XxxProvider::direct(cfg)` constructor.
- `from_env()` works for the relevant `Config` structs.
- Capabilities table for at least the current flagship model(s):
  - Anthropic: `claude-3-5-sonnet`, `claude-3-opus`, `claude-3-haiku` (plus latest if a more recent flagship exists at B-implementation time).
  - OpenAI: `gpt-4o`, `gpt-4o-mini`, `o1-preview`, `o1-mini`.
  - Ollama: at least `llama3.1`, `qwen2.5`, `mistral`.
  - Google: `gemini-2.0-flash`, `gemini-1.5-pro`, `gemini-1.5-flash`.

**Tests**
- Per-crate unit tests for IR conversion pass.
- Fixture tests against mock HTTP servers pass for every (schema, transport) pair shipped.
- Streaming-event parser tests pass for every transport variant.
- `cargo test --workspace` (no `--features live-tests`) is fully hermetic — no network, no env vars required.

**Live integration tests (manual)**
- Each direct adapter (`anthropic`, `openai`, `google-ai-studio`, `ollama`) has at least one passing live test under `--features live-tests` when the relevant credentials are present.
- Cloud transports (`bedrock`, `vertex-anthropic`, `vertex-gemini`, `azure`) have live tests gated on `bedrock,vertex,azure` features AND credentials being present.

**Documentation**
- New ADRs under `docs/adr/`:
  - `0006-message-schema-ir.md` — provider-neutral IR choice (final iteration of the message-schema decision deferred from Layer 0).
  - `0007-transport-trait-pattern.md` — schema/transport factoring decision.
  - `0008-system-role-positional.md` — `Role::System` leading-only constraint.
- `docs/adr/README.md` index updated to list the three new ADRs.
- Each schema-family crate has a top-level rustdoc on `lib.rs` describing supported transports, models, and known quirks.
- README updated to describe B's deliverables (replace the "no real agent runtime yet" caveat with a brief description of the provider crates).

**CI**
- Default CI job (`cargo test --workspace`) is green.
- Cloud CI job (`cargo test --workspace --features bedrock,vertex,azure`) is green.
- Path filtering for the cloud job is configured so it only fires on relevant changes.

## Open questions

None blocking. Items consciously deferred (Azure OAuth, Bedrock non-Claude families, unified config file, model-list runtime refresh) are documented above.

## Risks

- **AWS SDK weight.** `aws-sdk-bedrockruntime` + transitive deps is large (~30MB in `target/`, many crates). Mitigation: gated behind `bedrock` feature; CI separates cloud-feature build from default build.
- **GCP auth churn.** The Rust GCP auth ecosystem is less mature than the Python/Go equivalents. Risk of crate-deprecation. Mitigation: pin a specific `google-cloud-auth` (or alternative) version, treat any switch as an ADR-level decision.
- **Capability staleness.** Static const `ModelInfo` tables go stale when providers release new models. Mitigation: a small per-adapter doc note pointing future-self to update the table, plus the deferred `refresh_models()` enhancement.
- **Bedrock streaming framing complexity.** AWS event-stream is a binary framing protocol distinct from SSE. Implementation requires either using `aws-sdk-bedrockruntime`'s built-in stream support (preferred) or implementing AWS event-stream parsing manually. We prefer the former.
- **OpenAI o1 reasoning surfaces.** OpenAI's o1+ models expose reasoning tokens differently from Anthropic's thinking blocks. Capability advertised (`thinking: false` for o1; reasoning shows up only in usage counts). If o1's API exposes reasoning text later, we add it; for B we don't pretend to parse it.

## Implementation order (informs the plan)

Suggested task ordering for B's implementation plan:
1. `caliban-provider` (trait crate): IR types, `Error`, `Capabilities`, `Provider` trait, `MockProvider`, validation, builders, tests.
2. `caliban-provider-anthropic` with `DirectTransport` only (reference adapter — most expressive, fewest translation hacks).
3. `caliban-provider-openai` with `DirectTransport` only.
4. `caliban-provider-ollama`.
5. `caliban-provider-google` with `AIStudioTransport` only.
6. Cloud transports added one at a time: `BedrockTransport`, then `VertexTransport` for both Anthropic and Google, then `AzureTransport`.
7. ADRs + README updates + CI matrix update.

Each step has its own tests; the plan should treat each adapter (or each cloud-transport addition) as its own task so the per-task review cycle stays focused.
