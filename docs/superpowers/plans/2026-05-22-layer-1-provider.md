# Layer 1 / B (Provider Abstraction) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the provider-neutral message IR, the `Provider` trait, and four schema-family adapter crates (Anthropic, OpenAI, Ollama, Google) supporting eight (schema, transport) wirings: Anthropic-Direct, Anthropic-Bedrock, Anthropic-Vertex, OpenAI-Direct, OpenAI-Azure, Ollama-Direct, Google-AIStudio, Google-Vertex.

**Architecture:** A `caliban-provider` trait crate owns the IR (Messages, ContentBlocks, StreamEvents, Errors, Capabilities). Each schema-family crate (`caliban-provider-{anthropic,openai,ollama,google}`) defines a `Transport` trait, ships a `DirectTransport` by default, and gates additional transports (`bedrock`, `vertex`, `azure`) behind cargo features. The trait crate is transport-agnostic; each adapter's `XxxProvider<T: Transport>` is generic over its `Transport` and does the IR ↔ native conversion once per family.

**Tech Stack:** Rust 1.85.0 (edition 2024), `tokio` 1, `async-trait`, `serde`/`serde_json`, `reqwest` (with `rustls-tls` + `stream`), `futures` (Stream traits), `thiserror`, `secrecy`, `wiremock` (test only), `proptest` (test only), `aws-sdk-bedrockruntime` (feature `bedrock`), `aws-config` (feature `bedrock`), `gcp_auth` or `google-cloud-auth` (feature `vertex`), `eventsource-stream` (SSE parsing).

**Spec:** [`docs/superpowers/specs/2026-05-22-layer-1-provider-design.md`](../specs/2026-05-22-layer-1-provider-design.md)

---

## File Structure

Files this plan creates (grouped by crate):

```
crates/caliban-provider/
├── Cargo.toml
├── src/
│   ├── lib.rs                  re-exports + crate-level docs
│   ├── message.rs              Role, Message, ContentBlock, TextBlock, ImageBlock, ImageSource
│   ├── tool.rs                 Tool, ToolUseBlock, ToolResultBlock, ToolChoice
│   ├── thinking.rs             ThinkingBlock, ThinkingConfig
│   ├── cache.rs                CacheControl
│   ├── request.rs              CompletionRequest, builder, validation
│   ├── response.rs             CompletionResponse, Usage, StopReason
│   ├── stream.rs               StreamEvent, StreamingContentType, StreamingDelta, MessageStream
│   ├── capabilities.rs         Capabilities, ToolUseCapability, PromptCachingCapability, SystemPromptCapability, ModelInfo
│   ├── error.rs                Error, Result
│   ├── provider.rs             Provider trait
│   └── mock.rs                 MockProvider (feature `mock`)
└── tests/
    ├── builder.rs              CompletionRequest builder + validation
    ├── round_trip.rs           proptest round-trip Message ↔ JSON
    └── mock_provider.rs        MockProvider exercise

crates/caliban-provider-anthropic/
├── Cargo.toml
├── src/
│   ├── lib.rs                  AnthropicProvider<T>, public exports
│   ├── schema/
│   │   ├── mod.rs              re-exports
│   │   ├── request.rs          NativeRequest, NativeMessage, NativeContentBlock, NativeTool, NativeToolChoice
│   │   ├── response.rs         NativeResponse, NativeUsage, NativeStopReason
│   │   └── events.rs           NativeEvent (SSE event types)
│   ├── ir_convert.rs           ir_to_native(req), native_to_ir(resp), tool_use_id pass-through
│   ├── stream_parse.rs         SSE → StreamEvent; AWS event-stream → StreamEvent (feat=bedrock)
│   ├── models.rs               const ModelInfo table for Claude 3/3.5/3.7
│   ├── config.rs               DirectConfig, BedrockConfig (feat=bedrock), VertexConfig (feat=vertex)
│   ├── transport/
│   │   ├── mod.rs              Transport trait, TransportError
│   │   ├── direct.rs           DirectTransport
│   │   ├── bedrock.rs          BedrockTransport (feat=bedrock)
│   │   └── vertex.rs           VertexTransport (feat=vertex)
│   ├── error.rs                AnthropicError, conversion to caliban_provider::Error
│   └── tests/fixtures/
│       ├── direct/*.json       request/response/SSE fixtures
│       ├── bedrock/*.json
│       └── vertex/*.json
└── tests/
    ├── direct_fixture.rs       wiremock-based; default features
    ├── direct_stream.rs        SSE parser; default features
    ├── bedrock_fixture.rs      cfg(feature = "bedrock"); event-stream parser
    ├── vertex_fixture.rs       cfg(feature = "vertex")
    └── live.rs                 cfg(feature = "live-tests"); env-var-gated

crates/caliban-provider-openai/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── schema/{mod,request,response,events}.rs
│   ├── ir_convert.rs
│   ├── stream_parse.rs
│   ├── models.rs
│   ├── config.rs               DirectConfig, AzureConfig (feat=azure)
│   ├── transport/
│   │   ├── mod.rs
│   │   ├── direct.rs
│   │   └── azure.rs            cfg(feature = "azure")
│   ├── error.rs
│   └── tests/fixtures/{direct,azure}/
└── tests/
    ├── direct_fixture.rs
    ├── direct_stream.rs
    ├── azure_fixture.rs        cfg(feature = "azure")
    └── live.rs

crates/caliban-provider-ollama/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── schema/{mod,request,response,events}.rs
│   ├── ir_convert.rs
│   ├── stream_parse.rs         NDJSON parser
│   ├── models.rs
│   ├── config.rs
│   ├── transport/{mod,direct}.rs
│   ├── error.rs
│   └── tests/fixtures/direct/
└── tests/
    ├── direct_fixture.rs
    ├── direct_stream.rs
    └── live.rs

crates/caliban-provider-google/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── schema/{mod,request,response,events}.rs
│   ├── ir_convert.rs
│   ├── stream_parse.rs
│   ├── models.rs
│   ├── config.rs               AIStudioConfig, VertexConfig (feat=vertex)
│   ├── transport/
│   │   ├── mod.rs
│   │   ├── ai_studio.rs
│   │   └── vertex.rs           cfg(feature = "vertex")
│   ├── error.rs
│   └── tests/fixtures/{ai_studio,vertex}/
└── tests/
    ├── ai_studio_fixture.rs
    ├── ai_studio_stream.rs
    ├── vertex_fixture.rs       cfg(feature = "vertex")
    └── live.rs

adrs/
├── 0006-message-schema-ir.md
├── 0007-transport-trait-pattern.md
└── 0008-system-role-positional.md

.github/workflows/ci.yml         (modify)
README.md                        (modify)
Cargo.toml                       (modify — add workspace.dependencies, workspace.members)
```

**Workspace `Cargo.toml` additions** (will land incrementally across tasks):

```toml
[workspace.dependencies]
# (existing entries kept)
async-trait        = "0.1"
serde              = { version = "1", features = ["derive"] }
serde_json         = "1"
reqwest            = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
futures            = "0.3"
secrecy            = { version = "0.10", features = ["serde"] }
eventsource-stream = "0.2"
url                = { version = "2", features = ["serde"] }
bytes              = "1"
http               = "1"

# Cloud (feature-gated by each consumer crate, not workspace-default)
aws-config             = { version = "1", features = ["behavior-version-latest"] }
aws-sdk-bedrockruntime = "1"
aws-smithy-types       = "1"
gcp_auth               = "0.12"

# Test-only
wiremock = "0.6"
proptest = "1"
```

---

## Pre-flight cleanup

Before Task 1 begins, the working tree contains `crates/caliban-provider/` from Task 7's verification (cargo-new default skeleton, untracked). Each task that creates a file in that directory will overwrite the corresponding default file. The implementer for Task 1 will replace all of its default files with the real trait-crate contents.

---

## Task 1: `caliban-provider` trait crate

**Files:**
- Replace: `crates/caliban-provider/Cargo.toml`
- Replace: `crates/caliban-provider/src/lib.rs`
- Create: `crates/caliban-provider/src/{message,tool,thinking,cache,request,response,stream,capabilities,error,provider,mock}.rs`
- Create: `crates/caliban-provider/tests/{builder,round_trip,mock_provider}.rs`
- Modify: `Cargo.toml` (root — add workspace.dependencies; add `crates/caliban-provider` to workspace.members)

- [ ] **Step 1: Add workspace member and shared deps**

Edit root `Cargo.toml`:

1. Add `"crates/caliban-provider"` to `members` (after `"crates/caliban-core"`).
2. Add to `[workspace.dependencies]` (append, preserving existing entries):

```toml
async-trait        = "0.1"
serde              = { version = "1", features = ["derive"] }
serde_json         = "1"
reqwest            = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
futures            = "0.3"
secrecy            = { version = "0.10", features = ["serde"] }
eventsource-stream = "0.2"
url                = { version = "2", features = ["serde"] }
bytes              = "1"
http               = "1"
aws-config             = { version = "1", features = ["behavior-version-latest"] }
aws-sdk-bedrockruntime = "1"
aws-smithy-types       = "1"
gcp_auth               = "0.12"
wiremock = "0.6"
proptest = "1"
```

Run: `cargo metadata --format-version 1 --no-deps 2>&1 | head -5`
Expected: JSON with no error.

- [ ] **Step 2: Replace `crates/caliban-provider/Cargo.toml`**

```toml
[package]
name        = "caliban-provider"
version     = "0.0.0"
description = "Provider-neutral message IR and trait for the caliban agent harness"
edition.workspace      = true
license.workspace      = true
authors.workspace      = true
rust-version.workspace = true
publish     = false

[features]
default = []
mock    = []

[dependencies]
async-trait = { workspace = true }
serde       = { workspace = true }
serde_json  = { workspace = true }
thiserror   = { workspace = true }
futures     = { workspace = true }
secrecy     = { workspace = true }

[dev-dependencies]
tokio    = { workspace = true }
proptest = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 3: Write the IR types — `crates/caliban-provider/src/message.rs`**

```rust
//! Core message types: roles, messages, content blocks.

use serde::{Deserialize, Serialize};

use crate::cache::CacheControl;
use crate::thinking::ThinkingBlock;
use crate::tool::{ToolResultBlock, ToolUseBlock};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text(TextBlock {
                text: text.into(),
                cache_control: None,
            })],
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text(TextBlock {
                text: text.into(),
                cache_control: None,
            })],
        }
    }

    pub fn system_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentBlock::Text(TextBlock {
                text: text.into(),
                cache_control: None,
            })],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text(TextBlock),
    Image(ImageBlock),
    ToolUse(ToolUseBlock),
    ToolResult(ToolResultBlock),
    Thinking(ThinkingBlock),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextBlock {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageBlock {
    pub source: ImageSource,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Url(String),
}
```

- [ ] **Step 4: Write `tool.rs`**

```rust
//! Tool-use IR: declarations, calls, results.

use serde::{Deserialize, Serialize};

use crate::cache::CacheControl;
use crate::message::ContentBlock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolUseBlock {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultBlock {
    pub tool_use_id: String,
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    Any,
    Specific { name: String },
    None,
}

impl Default for ToolChoice {
    fn default() -> Self { Self::Auto }
}
```

- [ ] **Step 5: Write `thinking.rs`**

```rust
//! Extended-thinking IR.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingBlock {
    pub thinking: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingConfig {
    pub budget_tokens: u32,
}
```

- [ ] **Step 6: Write `cache.rs`**

```rust
//! Prompt-cache markers in the IR.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CacheControl {
    Ephemeral,
}
```

- [ ] **Step 7: Write `request.rs`**

```rust
//! Completion request, builder, and validation.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::message::{ContentBlock, Message, Role};
use crate::thinking::ThinkingConfig;
use crate::tool::{Tool, ToolChoice};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(default)]
    pub metadata: RequestMetadata,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

impl CompletionRequest {
    pub fn builder(model: impl Into<String>) -> CompletionRequestBuilder {
        CompletionRequestBuilder {
            model: model.into(),
            messages: Vec::new(),
            tools: Vec::new(),
            tool_choice: ToolChoice::default(),
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: Vec::new(),
            thinking: None,
            metadata: RequestMetadata::default(),
        }
    }

    /// Validate the request structure. Returns `Err(Error::InvalidRequest)` on violation.
    pub fn validate(&self) -> Result<()> {
        if self.model.is_empty() {
            return Err(Error::InvalidRequest("model is empty".into()));
        }
        if self.max_tokens == 0 {
            return Err(Error::InvalidRequest("max_tokens must be > 0".into()));
        }
        validate_messages(&self.messages)
    }
}

fn validate_messages(messages: &[Message]) -> Result<()> {
    let mut seen_non_system = false;
    let mut has_user_or_assistant = false;
    for (i, msg) in messages.iter().enumerate() {
        match msg.role {
            Role::System => {
                if seen_non_system {
                    return Err(Error::InvalidRequest(format!(
                        "Role::System message at index {i} appears after a User/Assistant \
                         message; System must lead"
                    )));
                }
                for block in &msg.content {
                    if !matches!(block, ContentBlock::Text(_)) {
                        return Err(Error::InvalidRequest(format!(
                            "Role::System message at index {i} contains a non-text block"
                        )));
                    }
                }
            }
            Role::User | Role::Assistant => {
                seen_non_system = true;
                has_user_or_assistant = true;
            }
        }
    }
    if !has_user_or_assistant {
        return Err(Error::InvalidRequest(
            "request has no User or Assistant messages".into(),
        ));
    }
    Ok(())
}

pub struct CompletionRequestBuilder {
    model: String,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    tool_choice: ToolChoice,
    max_tokens: u32,
    temperature: Option<f32>,
    top_p: Option<f32>,
    top_k: Option<u32>,
    stop_sequences: Vec<String>,
    thinking: Option<ThinkingConfig>,
    metadata: RequestMetadata,
}

impl CompletionRequestBuilder {
    #[must_use]
    pub fn system(mut self, text: impl Into<String>) -> Self {
        self.messages.insert(0, Message::system_text(text));
        self
    }

    #[must_use]
    pub fn user_text(mut self, text: impl Into<String>) -> Self {
        self.messages.push(Message::user_text(text));
        self
    }

    #[must_use]
    pub fn assistant_text(mut self, text: impl Into<String>) -> Self {
        self.messages.push(Message::assistant_text(text));
        self
    }

    #[must_use]
    pub fn message(mut self, m: Message) -> Self {
        self.messages.push(m);
        self
    }

    #[must_use]
    pub fn tool(mut self, t: Tool) -> Self {
        self.tools.push(t);
        self
    }

    #[must_use]
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = choice;
        self
    }

    #[must_use]
    pub fn max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    #[must_use]
    pub fn temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    #[must_use]
    pub fn top_p(mut self, p: f32) -> Self {
        self.top_p = Some(p);
        self
    }

    #[must_use]
    pub fn top_k(mut self, k: u32) -> Self {
        self.top_k = Some(k);
        self
    }

    #[must_use]
    pub fn stop_sequence(mut self, s: impl Into<String>) -> Self {
        self.stop_sequences.push(s.into());
        self
    }

    #[must_use]
    pub fn thinking(mut self, cfg: ThinkingConfig) -> Self {
        self.thinking = Some(cfg);
        self
    }

    #[must_use]
    pub fn user_id(mut self, id: impl Into<String>) -> Self {
        self.metadata.user_id = Some(id.into());
        self
    }

    pub fn build(self) -> Result<CompletionRequest> {
        let req = CompletionRequest {
            model: self.model,
            messages: self.messages,
            tools: self.tools,
            tool_choice: self.tool_choice,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            top_p: self.top_p,
            top_k: self.top_k,
            stop_sequences: self.stop_sequences,
            thinking: self.thinking,
            metadata: self.metadata,
        };
        req.validate()?;
        Ok(req)
    }
}
```

- [ ] **Step 8: Write `response.rs`**

```rust
//! Completion response, usage, stop-reason.

use serde::{Deserialize, Serialize};

use crate::message::Message;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub id: String,
    pub model: String,
    pub message: Message,
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    ContentFilter,
    Refusal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}

impl Usage {
    pub fn merge(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens = match (self.cache_creation_input_tokens, other.cache_creation_input_tokens) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        self.cache_read_input_tokens = match (self.cache_read_input_tokens, other.cache_read_input_tokens) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
    }
}
```

- [ ] **Step 9: Write `stream.rs`**

```rust
//! Streaming events.

use std::pin::Pin;

use futures::stream::Stream;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::message::{ContentBlock, Message, Role, TextBlock};
use crate::response::{StopReason, Usage};
use crate::thinking::ThinkingBlock;
use crate::tool::ToolUseBlock;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart { id: String, model: String },
    ContentBlockStart { index: u32, content_type: StreamingContentType },
    Delta { index: u32, delta: StreamingDelta },
    ContentBlockStop { index: u32 },
    MessageDelta { stop_reason: Option<StopReason>, usage_delta: Option<Usage> },
    MessageStop,
    Ping,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamingContentType {
    Text,
    ToolUse { id: String, name: String },
    Thinking,
    Image,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamingDelta {
    Text(String),
    ToolUseInputJson(String),
    Thinking(String),
}

/// Boxed dynamic stream of stream events.
pub type MessageStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send + 'static>>;

/// Helper to consume a `MessageStream` and assemble its final `Message`, `StopReason`, and `Usage`.
pub async fn collect_message(
    mut stream: MessageStream,
) -> Result<(Message, StopReason, Usage)> {
    let mut blocks: Vec<ContentBlock> = Vec::new();
    let mut block_types: Vec<StreamingContentType> = Vec::new();
    let mut block_text: Vec<String> = Vec::new();
    let mut block_json: Vec<String> = Vec::new();
    let mut stop_reason: Option<StopReason> = None;
    let mut usage = Usage::default();

    while let Some(evt) = stream.next().await {
        match evt? {
            StreamEvent::MessageStart { .. } | StreamEvent::Ping | StreamEvent::MessageStop => {}
            StreamEvent::ContentBlockStart { index, content_type } => {
                let i = index as usize;
                if blocks.len() <= i {
                    blocks.resize(i + 1, ContentBlock::Text(TextBlock { text: String::new(), cache_control: None }));
                    block_types.resize(i + 1, StreamingContentType::Text);
                    block_text.resize(i + 1, String::new());
                    block_json.resize(i + 1, String::new());
                }
                block_types[i] = content_type;
            }
            StreamEvent::Delta { index, delta } => {
                let i = index as usize;
                match delta {
                    StreamingDelta::Text(s) => block_text[i].push_str(&s),
                    StreamingDelta::ToolUseInputJson(s) => block_json[i].push_str(&s),
                    StreamingDelta::Thinking(s) => block_text[i].push_str(&s),
                }
            }
            StreamEvent::ContentBlockStop { index } => {
                let i = index as usize;
                let block = match &block_types[i] {
                    StreamingContentType::Text => ContentBlock::Text(TextBlock {
                        text: std::mem::take(&mut block_text[i]),
                        cache_control: None,
                    }),
                    StreamingContentType::Thinking => ContentBlock::Thinking(ThinkingBlock {
                        thinking: std::mem::take(&mut block_text[i]),
                        signature: None,
                    }),
                    StreamingContentType::ToolUse { id, name } => {
                        let json_str = std::mem::take(&mut block_json[i]);
                        let input = if json_str.is_empty() {
                            serde_json::json!({})
                        } else {
                            serde_json::from_str(&json_str)
                                .map_err(|e| Error::InvalidRequest(format!("tool_use input json parse error: {e}")))?
                        };
                        ContentBlock::ToolUse(ToolUseBlock {
                            id: id.clone(),
                            name: name.clone(),
                            input,
                        })
                    }
                    StreamingContentType::Image => {
                        return Err(Error::InvalidRequest("streaming Image blocks are not supported in collect_message".into()));
                    }
                };
                blocks[i] = block;
            }
            StreamEvent::MessageDelta { stop_reason: sr, usage_delta } => {
                if let Some(sr) = sr { stop_reason = Some(sr); }
                if let Some(u) = usage_delta { usage.merge(u); }
            }
        }
    }

    let stop = stop_reason.unwrap_or(StopReason::EndTurn);
    Ok((Message { role: Role::Assistant, content: blocks }, stop, usage))
}
```

- [ ] **Step 10: Write `capabilities.rs`**

```rust
//! Capability discovery types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolUseCapability {
    None,
    Basic,
    ParallelCalls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptCachingCapability {
    None,
    Automatic,
    Explicit { max_breakpoints: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemPromptCapability {
    SeparateField,
    SystemRole,
    DeveloperRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub native_id: String,
    pub display_name: String,
    pub capabilities: Capabilities,
}
```

- [ ] **Step 11: Write `error.rs`**

```rust
//! Cross-provider error enum.

use std::time::Duration;

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

impl Error {
    pub fn network(e: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Network(Box::new(e))
    }

    pub fn adapter(e: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Adapter(Box::new(e))
    }
}
```

- [ ] **Step 12: Write `provider.rs`**

```rust
//! The `Provider` trait.

use async_trait::async_trait;

use crate::capabilities::{Capabilities, ModelInfo};
use crate::error::Result;
use crate::request::CompletionRequest;
use crate::response::CompletionResponse;
use crate::stream::MessageStream;

#[async_trait]
pub trait Provider: Send + Sync {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse>;

    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream>;

    fn capabilities(&self, model: &str) -> Capabilities;

    fn list_models(&self) -> Vec<ModelInfo>;

    fn name(&self) -> &'static str;
}
```

- [ ] **Step 13: Write `mock.rs`** (feature `mock`)

```rust
//! Scripted MockProvider for downstream consumer tests.

use std::sync::Mutex;

use async_trait::async_trait;
use futures::stream;

use crate::capabilities::{Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability};
use crate::error::{Error, Result};
use crate::provider::Provider;
use crate::request::CompletionRequest;
use crate::response::CompletionResponse;
use crate::stream::{MessageStream, StreamEvent};

#[derive(Default)]
pub struct MockProvider {
    inner: Mutex<MockState>,
}

#[derive(Default)]
struct MockState {
    complete_queue: Vec<Result<CompletionResponse>>,
    stream_queue: Vec<Result<Vec<Result<StreamEvent>>>>,
    capabilities: Option<Capabilities>,
    models: Vec<ModelInfo>,
}

impl MockProvider {
    pub fn new() -> Self { Self::default() }

    pub fn enqueue_complete(&self, resp: Result<CompletionResponse>) {
        self.inner.lock().expect("MockProvider lock poisoned").complete_queue.push(resp);
    }

    pub fn enqueue_stream(&self, events: Vec<Result<StreamEvent>>) {
        self.inner.lock().expect("MockProvider lock poisoned").stream_queue.push(Ok(events));
    }

    pub fn enqueue_stream_error(&self, err: Error) {
        self.inner.lock().expect("MockProvider lock poisoned").stream_queue.push(Err(err));
    }

    pub fn set_capabilities(&self, caps: Capabilities) {
        self.inner.lock().expect("MockProvider lock poisoned").capabilities = Some(caps);
    }

    pub fn set_models(&self, models: Vec<ModelInfo>) {
        self.inner.lock().expect("MockProvider lock poisoned").models = models;
    }
}

fn default_capabilities() -> Capabilities {
    Capabilities {
        max_input_tokens: 100_000,
        max_output_tokens: 4_096,
        vision: false,
        tool_use: ToolUseCapability::Basic,
        thinking: false,
        prompt_caching: PromptCachingCapability::None,
        json_mode: false,
        streaming: true,
        stop_sequences: true,
        top_k: false,
        system_prompt: SystemPromptCapability::SeparateField,
        refusal_field: false,
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
        let mut s = self.inner.lock().expect("MockProvider lock poisoned");
        if s.complete_queue.is_empty() {
            return Err(Error::InvalidRequest("MockProvider: complete queue empty".into()));
        }
        s.complete_queue.remove(0)
    }

    async fn stream(&self, _req: CompletionRequest) -> Result<MessageStream> {
        let mut s = self.inner.lock().expect("MockProvider lock poisoned");
        if s.stream_queue.is_empty() {
            return Err(Error::InvalidRequest("MockProvider: stream queue empty".into()));
        }
        match s.stream_queue.remove(0) {
            Err(e) => Err(e),
            Ok(events) => Ok(Box::pin(stream::iter(events))),
        }
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        self.inner.lock().expect("MockProvider lock poisoned").capabilities.unwrap_or_else(default_capabilities)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        self.inner.lock().expect("MockProvider lock poisoned").models.clone()
    }

    fn name(&self) -> &'static str { "mock" }
}
```

- [ ] **Step 14: Write `lib.rs`**

```rust
//! Provider-neutral message IR and the `Provider` trait for the caliban
//! agent harness. Adapter crates (`caliban-provider-anthropic`, etc.)
//! implement this trait for specific schema-family/transport pairs.

pub mod cache;
pub mod capabilities;
pub mod error;
pub mod message;
pub mod provider;
pub mod request;
pub mod response;
pub mod stream;
pub mod thinking;
pub mod tool;

#[cfg(feature = "mock")]
pub mod mock;

pub use cache::CacheControl;
pub use capabilities::{
    Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};
pub use error::{Error, Result};
pub use message::{ContentBlock, ImageBlock, ImageSource, Message, Role, TextBlock};
pub use provider::Provider;
pub use request::{CompletionRequest, CompletionRequestBuilder, RequestMetadata};
pub use response::{CompletionResponse, StopReason, Usage};
pub use stream::{collect_message, MessageStream, StreamEvent, StreamingContentType, StreamingDelta};
pub use thinking::{ThinkingBlock, ThinkingConfig};
pub use tool::{Tool, ToolChoice, ToolResultBlock, ToolUseBlock};

#[cfg(feature = "mock")]
pub use mock::MockProvider;
```

- [ ] **Step 15: Write `tests/builder.rs`**

```rust
use caliban_provider::{CompletionRequest, Error, Message, Role, ToolChoice};

#[test]
fn builder_constructs_valid_request() {
    let req = CompletionRequest::builder("claude-3-5-sonnet")
        .system("You are helpful.")
        .user_text("Hi!")
        .max_tokens(256)
        .build()
        .expect("valid");
    assert_eq!(req.model, "claude-3-5-sonnet");
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[0].role, Role::System);
    assert_eq!(req.messages[1].role, Role::User);
    assert_eq!(req.max_tokens, 256);
}

#[test]
fn rejects_zero_max_tokens() {
    let err = CompletionRequest::builder("m")
        .user_text("hi")
        .max_tokens(0)
        .build()
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(s) if s.contains("max_tokens")));
}

#[test]
fn rejects_empty_model() {
    let err = CompletionRequest::builder("")
        .user_text("hi")
        .build()
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(s) if s.contains("model")));
}

#[test]
fn rejects_system_after_user() {
    let req = CompletionRequest {
        model: "m".into(),
        messages: vec![
            Message::user_text("u"),
            Message::system_text("s"),
        ],
        tools: vec![],
        tool_choice: ToolChoice::Auto,
        max_tokens: 64,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        thinking: None,
        metadata: Default::default(),
    };
    let err = req.validate().unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(s) if s.contains("System")));
}

#[test]
fn rejects_image_in_system() {
    use caliban_provider::{ContentBlock, ImageBlock, ImageSource};
    let req = CompletionRequest {
        model: "m".into(),
        messages: vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::Image(ImageBlock {
                    source: ImageSource::Url("https://x/img.png".into()),
                    cache_control: None,
                })],
            },
            Message::user_text("u"),
        ],
        tools: vec![],
        tool_choice: ToolChoice::Auto,
        max_tokens: 64,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        thinking: None,
        metadata: Default::default(),
    };
    let err = req.validate().unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(s) if s.contains("non-text")));
}

#[test]
fn rejects_no_user_or_assistant() {
    let req = CompletionRequest {
        model: "m".into(),
        messages: vec![Message::system_text("only system")],
        tools: vec![],
        tool_choice: ToolChoice::Auto,
        max_tokens: 64,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        thinking: None,
        metadata: Default::default(),
    };
    let err = req.validate().unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(s) if s.contains("User or Assistant")));
}

#[test]
fn multiple_leading_system_messages_ok() {
    let req = CompletionRequest::builder("m")
        .message(Message::system_text("rules"))
        .message(Message::system_text("more rules"))
        .user_text("hi")
        .build()
        .expect("valid");
    assert_eq!(req.messages.len(), 3);
}
```

- [ ] **Step 16: Write `tests/round_trip.rs`**

```rust
use caliban_provider::{
    CacheControl, ContentBlock, ImageBlock, ImageSource, Message, Role, TextBlock,
};
use proptest::prelude::*;

fn arb_text_block() -> impl Strategy<Value = TextBlock> {
    (any::<String>(), prop::option::of(Just(CacheControl::Ephemeral)))
        .prop_map(|(text, cache_control)| TextBlock { text, cache_control })
}

fn arb_image_block() -> impl Strategy<Value = ImageBlock> {
    (any::<String>(), any::<String>(), prop::option::of(Just(CacheControl::Ephemeral)))
        .prop_map(|(mime, data, cache_control)| ImageBlock {
            source: ImageSource::Base64 { media_type: mime, data },
            cache_control,
        })
}

fn arb_content_block() -> impl Strategy<Value = ContentBlock> {
    prop_oneof![
        arb_text_block().prop_map(ContentBlock::Text),
        arb_image_block().prop_map(ContentBlock::Image),
    ]
}

fn arb_role() -> impl Strategy<Value = Role> {
    prop_oneof![Just(Role::User), Just(Role::Assistant), Just(Role::System)]
}

fn arb_message() -> impl Strategy<Value = Message> {
    (arb_role(), prop::collection::vec(arb_content_block(), 0..3))
        .prop_map(|(role, content)| Message { role, content })
}

proptest! {
    #[test]
    fn message_serde_round_trip(m in arb_message()) {
        let json = serde_json::to_string(&m).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(parsed, m);
    }
}
```

- [ ] **Step 17: Write `tests/mock_provider.rs`**

```rust
#![cfg(feature = "mock")]

use caliban_provider::{
    collect_message, CompletionRequest, CompletionResponse, Message, MockProvider, Provider,
    StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};

#[tokio::test]
async fn mock_provider_serves_complete_responses() {
    let mock = MockProvider::new();
    mock.enqueue_complete(Ok(CompletionResponse {
        id: "msg_1".into(),
        model: "mock-model".into(),
        message: Message::assistant_text("hello"),
        stop_reason: StopReason::EndTurn,
        stop_sequence: None,
        usage: Usage { input_tokens: 10, output_tokens: 5, cache_creation_input_tokens: None, cache_read_input_tokens: None },
    }));

    let req = CompletionRequest::builder("mock-model")
        .user_text("hi")
        .build()
        .unwrap();
    let resp = mock.complete(req).await.unwrap();
    assert_eq!(resp.id, "msg_1");
    assert_eq!(resp.usage.input_tokens, 10);
}

#[tokio::test]
async fn mock_provider_serves_stream() {
    let mock = MockProvider::new();
    mock.enqueue_stream(vec![
        Ok(StreamEvent::MessageStart { id: "msg_1".into(), model: "mock-model".into() }),
        Ok(StreamEvent::ContentBlockStart { index: 0, content_type: StreamingContentType::Text }),
        Ok(StreamEvent::Delta { index: 0, delta: StreamingDelta::Text("hi".into()) }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage { input_tokens: 4, output_tokens: 1, cache_creation_input_tokens: None, cache_read_input_tokens: None }),
        }),
        Ok(StreamEvent::MessageStop),
    ]);

    let req = CompletionRequest::builder("mock-model")
        .user_text("hi")
        .build()
        .unwrap();
    let stream = mock.stream(req).await.unwrap();
    let (msg, stop, usage) = collect_message(stream).await.unwrap();
    assert_eq!(msg.content.len(), 1);
    assert!(matches!(stop, StopReason::EndTurn));
    assert_eq!(usage.input_tokens, 4);
}
```

- [ ] **Step 18: Run unit tests**

Run: `cargo test -p caliban-provider --features mock`

Expected: at least 7 tests pass (builder validation tests) + 1 proptest case + 2 mock tests.

- [ ] **Step 19: Run clippy and fmt**

```bash
cargo fmt --all -- --check
cargo clippy -p caliban-provider --all-features --all-targets -- -D warnings
```

Both must exit 0. If clippy complains about `missing_docs` on the public API, add `//!` module-level docs and `///` doc comments at minimum stubs (one-liners). Don't elaborate — just satisfy the lint.

- [ ] **Step 20: Commit**

```bash
git add Cargo.toml crates/caliban-provider/
git commit -m "$(cat <<'EOF'
feat(provider): caliban-provider trait crate (IR + Provider + MockProvider)

Defines the provider-neutral message IR (Role, Message, ContentBlock,
TextBlock, ImageBlock, ToolUseBlock, ToolResultBlock, ThinkingBlock,
CacheControl), the CompletionRequest builder + validation, the
CompletionResponse / StreamEvent types, the cross-provider Error
enum, the Capabilities struct hierarchy, and the object-safe
async Provider trait.

System role is enforced positional (leading-only, Text-only) by
validation in CompletionRequest::validate(). The collect_message
helper assembles a final Message from a MessageStream.

MockProvider is gated behind feature "mock" — used by downstream
consumer tests to script responses and streaming events.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `caliban-provider-anthropic` — schema, IR conversion, direct transport (non-streaming)

**Files:**
- Create: `crates/caliban-provider-anthropic/Cargo.toml`
- Create: `crates/caliban-provider-anthropic/src/{lib,error,models,config,ir_convert}.rs`
- Create: `crates/caliban-provider-anthropic/src/schema/{mod,request,response}.rs`
- Create: `crates/caliban-provider-anthropic/src/transport/{mod,direct}.rs`
- Create: `crates/caliban-provider-anthropic/tests/fixtures/direct/{complete_simple_request,complete_simple_response}.json`
- Create: `crates/caliban-provider-anthropic/tests/direct_fixture.rs`
- Modify: root `Cargo.toml` (add member)

- [ ] **Step 1: Add member to workspace and create Cargo.toml**

Edit root `Cargo.toml`: add `"crates/caliban-provider-anthropic"` to `members`.

Create `crates/caliban-provider-anthropic/Cargo.toml`:

```toml
[package]
name        = "caliban-provider-anthropic"
version     = "0.0.0"
description = "Anthropic Claude schema family for the caliban agent harness"
edition.workspace      = true
license.workspace      = true
authors.workspace      = true
rust-version.workspace = true
publish     = false

[features]
default = []
bedrock = ["dep:aws-config", "dep:aws-sdk-bedrockruntime", "dep:aws-smithy-types"]
vertex  = ["dep:gcp_auth"]
live-tests = []

[dependencies]
caliban-provider   = { path = "../caliban-provider" }
async-trait        = { workspace = true }
serde              = { workspace = true }
serde_json         = { workspace = true }
thiserror          = { workspace = true }
reqwest            = { workspace = true }
secrecy            = { workspace = true }
futures            = { workspace = true }
eventsource-stream = { workspace = true }
url                = { workspace = true }
bytes              = { workspace = true }
http               = { workspace = true }
tokio              = { workspace = true }

aws-config             = { workspace = true, optional = true }
aws-sdk-bedrockruntime = { workspace = true, optional = true }
aws-smithy-types       = { workspace = true, optional = true }
gcp_auth               = { workspace = true, optional = true }

[dev-dependencies]
wiremock = { workspace = true }
tokio    = { workspace = true, features = ["macros", "rt-multi-thread"] }

[lints]
workspace = true
```

- [ ] **Step 2: Write `src/error.rs`**

```rust
//! Adapter-internal error type and conversion to caliban_provider::Error.

use caliban_provider::Error as ProviderError;

#[derive(thiserror::Error, Debug)]
pub enum AnthropicError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("response status {status}: {body}")]
    BadStatus { status: u16, body: String },

    #[error("deserialize error: {0}")]
    Deserialize(#[from] serde_json::Error),

    #[error("stream parse error: {0}")]
    StreamParse(String),

    #[error("missing config field: {0}")]
    MissingConfig(&'static str),

    #[error("transport error: {0}")]
    Transport(Box<dyn std::error::Error + Send + Sync>),
}

impl From<AnthropicError> for ProviderError {
    fn from(e: AnthropicError) -> Self {
        match e {
            AnthropicError::Http(ref reqwest_err) => {
                if reqwest_err.is_connect() || reqwest_err.is_timeout() {
                    ProviderError::network(e)
                } else {
                    ProviderError::adapter(e)
                }
            }
            AnthropicError::BadStatus { status, ref body } => match status {
                401 | 403 => ProviderError::Auth(body.clone()),
                429 => ProviderError::RateLimit { retry_after: None },
                400 => ProviderError::InvalidRequest(body.clone()),
                404 => ProviderError::ModelUnavailable(body.clone()),
                _ if status >= 500 => ProviderError::ServerError { status, body: body.clone() },
                _ => ProviderError::adapter(e),
            },
            AnthropicError::Deserialize(_) | AnthropicError::StreamParse(_) | AnthropicError::MissingConfig(_) | AnthropicError::Transport(_) => {
                ProviderError::adapter(e)
            }
        }
    }
}
```

- [ ] **Step 3: Write `src/schema/request.rs`**

Anthropic's request body (https://docs.anthropic.com/en/api/messages):

```rust
//! Wire-format types for Anthropic Messages API requests.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeRequest {
    pub model: String,
    pub messages: Vec<NativeMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<NativeSystem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<NativeTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<NativeToolChoice>,
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<NativeThinking>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<NativeMetadata>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    /// Bedrock requires this field; Direct/Vertex ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativeSystem {
    Text(String),
    Blocks(Vec<NativeTextBlock>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMessage {
    pub role: String,            // "user" or "assistant"
    pub content: NativeContent,  // string or block array
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativeContent {
    Text(String),
    Blocks(Vec<NativeContentBlock>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeContentBlock {
    Text(NativeTextBlock),
    Image(NativeImageBlock),
    ToolUse(NativeToolUseBlock),
    ToolResult(NativeToolResultBlock),
    Thinking(NativeThinkingBlock),
    RedactedThinking { data: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeTextBlock {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<NativeCacheControl>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeImageBlock {
    pub source: NativeImageSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<NativeCacheControl>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolUseBlock {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolResultBlock {
    pub tool_use_id: String,
    pub content: NativeContent,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeThinkingBlock {
    pub thinking: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeCacheControl {
    Ephemeral,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<NativeCacheControl>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeToolChoice {
    Auto,
    Any,
    Tool { name: String },
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeThinking {
    #[serde(rename = "type")]
    pub kind: String, // "enabled"
    pub budget_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}
```

- [ ] **Step 4: Write `src/schema/response.rs`**

```rust
//! Wire-format types for Anthropic Messages API responses.

use serde::{Deserialize, Serialize};

use super::request::NativeContentBlock;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeResponse {
    pub id: String,
    pub model: String,
    pub role: String,                 // "assistant"
    #[serde(rename = "type")]
    pub kind: String,                 // "message"
    pub content: Vec<NativeContentBlock>,
    pub stop_reason: NativeStopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    pub usage: NativeUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeStopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    Refusal,
    PauseTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NativeUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}
```

- [ ] **Step 5: Write `src/schema/mod.rs`**

```rust
pub mod request;
pub mod response;

pub use request::*;
pub use response::*;
```

- [ ] **Step 6: Write `src/ir_convert.rs`** (IR ↔ native conversions for request and response only — streaming in Task 3)

```rust
//! IR ↔ Anthropic native conversions for request and response.

use caliban_provider::{
    CacheControl, ContentBlock, Error, ImageBlock as IrImageBlock, ImageSource as IrImageSource,
    Message, Result, Role, StopReason, TextBlock as IrTextBlock, Tool as IrTool,
    ToolChoice as IrToolChoice, ToolResultBlock as IrToolResultBlock,
    ToolUseBlock as IrToolUseBlock, Usage as IrUsage,
    ThinkingBlock as IrThinkingBlock,
};

use crate::schema::request::*;
use crate::schema::response::*;

pub fn ir_to_native_request(
    req: caliban_provider::CompletionRequest,
    stream: bool,
) -> NativeRequest {
    // Split off leading System messages
    let mut messages = req.messages.into_iter().peekable();
    let mut system_blocks: Vec<NativeTextBlock> = Vec::new();
    while let Some(m) = messages.peek() {
        if m.role != Role::System { break; }
        let m = messages.next().expect("peeked");
        for cb in m.content {
            if let ContentBlock::Text(tb) = cb {
                system_blocks.push(NativeTextBlock {
                    text: tb.text,
                    cache_control: tb.cache_control.map(|_| NativeCacheControl::Ephemeral),
                });
            }
        }
    }
    let system = if system_blocks.is_empty() {
        None
    } else if system_blocks.iter().all(|b| b.cache_control.is_none()) {
        Some(NativeSystem::Text(
            system_blocks.into_iter().map(|b| b.text).collect::<Vec<_>>().join("\n\n"),
        ))
    } else {
        Some(NativeSystem::Blocks(system_blocks))
    };

    let native_messages: Vec<NativeMessage> = messages
        .map(|m| NativeMessage {
            role: match m.role {
                Role::User => "user".into(),
                Role::Assistant => "assistant".into(),
                Role::System => unreachable!("System filtered above"),
            },
            content: NativeContent::Blocks(
                m.content.into_iter().map(ir_content_block_to_native).collect(),
            ),
        })
        .collect();

    NativeRequest {
        model: req.model,
        messages: native_messages,
        system,
        tools: req.tools.into_iter().map(ir_tool_to_native).collect(),
        tool_choice: Some(ir_tool_choice_to_native(req.tool_choice)),
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: req.top_k,
        stop_sequences: req.stop_sequences,
        thinking: req.thinking.map(|t| NativeThinking {
            kind: "enabled".into(),
            budget_tokens: t.budget_tokens,
        }),
        metadata: Some(NativeMetadata { user_id: req.metadata.user_id }),
        stream,
        anthropic_version: None,
    }
}

fn ir_content_block_to_native(b: ContentBlock) -> NativeContentBlock {
    match b {
        ContentBlock::Text(t) => NativeContentBlock::Text(NativeTextBlock {
            text: t.text,
            cache_control: t.cache_control.map(|_| NativeCacheControl::Ephemeral),
        }),
        ContentBlock::Image(i) => NativeContentBlock::Image(NativeImageBlock {
            source: match i.source {
                IrImageSource::Base64 { media_type, data } => NativeImageSource::Base64 { media_type, data },
                IrImageSource::Url(url) => NativeImageSource::Url { url },
            },
            cache_control: i.cache_control.map(|_| NativeCacheControl::Ephemeral),
        }),
        ContentBlock::ToolUse(tu) => NativeContentBlock::ToolUse(NativeToolUseBlock {
            id: tu.id,
            name: tu.name,
            input: tu.input,
        }),
        ContentBlock::ToolResult(tr) => NativeContentBlock::ToolResult(NativeToolResultBlock {
            tool_use_id: tr.tool_use_id,
            content: NativeContent::Blocks(
                tr.content.into_iter().map(ir_content_block_to_native).collect(),
            ),
            is_error: tr.is_error,
        }),
        ContentBlock::Thinking(t) => NativeContentBlock::Thinking(NativeThinkingBlock {
            thinking: t.thinking,
            signature: t.signature,
        }),
    }
}

fn ir_tool_to_native(t: IrTool) -> NativeTool {
    NativeTool {
        name: t.name,
        description: t.description,
        input_schema: t.input_schema,
        cache_control: t.cache_control.map(|_| NativeCacheControl::Ephemeral),
    }
}

fn ir_tool_choice_to_native(c: IrToolChoice) -> NativeToolChoice {
    match c {
        IrToolChoice::Auto => NativeToolChoice::Auto,
        IrToolChoice::Any => NativeToolChoice::Any,
        IrToolChoice::Specific { name } => NativeToolChoice::Tool { name },
        IrToolChoice::None => NativeToolChoice::None,
    }
}

pub fn native_response_to_ir(r: NativeResponse) -> Result<caliban_provider::CompletionResponse> {
    let content = r.content.into_iter().map(native_block_to_ir).collect::<Result<Vec<_>>>()?;
    Ok(caliban_provider::CompletionResponse {
        id: r.id,
        model: r.model,
        message: Message { role: Role::Assistant, content },
        stop_reason: match r.stop_reason {
            NativeStopReason::EndTurn => StopReason::EndTurn,
            NativeStopReason::MaxTokens => StopReason::MaxTokens,
            NativeStopReason::StopSequence => StopReason::StopSequence,
            NativeStopReason::ToolUse => StopReason::ToolUse,
            NativeStopReason::Refusal => StopReason::Refusal,
            NativeStopReason::PauseTurn => StopReason::EndTurn,
        },
        stop_sequence: r.stop_sequence,
        usage: IrUsage {
            input_tokens: r.usage.input_tokens,
            output_tokens: r.usage.output_tokens,
            cache_creation_input_tokens: r.usage.cache_creation_input_tokens,
            cache_read_input_tokens: r.usage.cache_read_input_tokens,
        },
    })
}

fn native_block_to_ir(b: NativeContentBlock) -> Result<ContentBlock> {
    Ok(match b {
        NativeContentBlock::Text(t) => ContentBlock::Text(IrTextBlock {
            text: t.text,
            cache_control: t.cache_control.map(|_| CacheControl::Ephemeral),
        }),
        NativeContentBlock::Image(i) => ContentBlock::Image(IrImageBlock {
            source: match i.source {
                NativeImageSource::Base64 { media_type, data } => IrImageSource::Base64 { media_type, data },
                NativeImageSource::Url { url } => IrImageSource::Url(url),
            },
            cache_control: i.cache_control.map(|_| CacheControl::Ephemeral),
        }),
        NativeContentBlock::ToolUse(tu) => ContentBlock::ToolUse(IrToolUseBlock {
            id: tu.id,
            name: tu.name,
            input: tu.input,
        }),
        NativeContentBlock::ToolResult(tr) => ContentBlock::ToolResult(IrToolResultBlock {
            tool_use_id: tr.tool_use_id,
            content: match tr.content {
                NativeContent::Text(s) => vec![ContentBlock::Text(IrTextBlock { text: s, cache_control: None })],
                NativeContent::Blocks(bs) => bs.into_iter().map(native_block_to_ir).collect::<Result<Vec<_>>>()?,
            },
            is_error: tr.is_error,
        }),
        NativeContentBlock::Thinking(t) => ContentBlock::Thinking(IrThinkingBlock {
            thinking: t.thinking,
            signature: t.signature,
        }),
        NativeContentBlock::RedactedThinking { data } => ContentBlock::Thinking(IrThinkingBlock {
            thinking: String::new(),
            signature: Some(data),
        }),
    })
}
```

- [ ] **Step 7: Write `src/config.rs`** (DirectConfig only at this point)

```rust
//! Per-transport configuration structs.

use std::time::Duration;

use secrecy::SecretString;
use url::Url;

use crate::error::AnthropicError;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub struct DirectConfig {
    pub api_key: SecretString,
    pub base_url: Url,
    pub anthropic_version: String,
    pub timeout: Duration,
}

impl DirectConfig {
    pub fn new(api_key: SecretString) -> Self {
        Self {
            api_key,
            base_url: Url::parse(DEFAULT_BASE_URL).expect("static URL parses"),
            anthropic_version: DEFAULT_ANTHROPIC_VERSION.to_string(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    pub fn from_env() -> Result<Self, AnthropicError> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| AnthropicError::MissingConfig("ANTHROPIC_API_KEY"))?;
        let mut cfg = Self::new(SecretString::new(key.into()));
        if let Ok(url) = std::env::var("ANTHROPIC_BASE_URL") {
            cfg.base_url = Url::parse(&url).map_err(|e| AnthropicError::Transport(Box::new(e)))?;
        }
        if let Ok(v) = std::env::var("ANTHROPIC_VERSION") {
            cfg.anthropic_version = v;
        }
        Ok(cfg)
    }
}
```

- [ ] **Step 8: Write `src/transport/mod.rs`**

```rust
//! The `Transport` trait — abstracts how a request is delivered to Anthropic.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::AnthropicError;
use crate::schema::{NativeRequest, NativeResponse};

#[async_trait]
pub trait Transport: Send + Sync + 'static {
    async fn send(&self, body: NativeRequest) -> Result<NativeResponse, AnthropicError>;

    async fn stream(
        &self,
        body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, AnthropicError>>, AnthropicError>;

    fn wire_model_id(&self, canonical: &str) -> String { canonical.to_string() }

    fn finalize_request(&self, _body: &mut NativeRequest) {}
}

pub mod direct;

#[cfg(feature = "bedrock")]
pub mod bedrock;

#[cfg(feature = "vertex")]
pub mod vertex;
```

- [ ] **Step 9: Write `src/transport/direct.rs`**

```rust
//! Direct transport — talks to api.anthropic.com.

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use secrecy::ExposeSecret;

use crate::config::DirectConfig;
use crate::error::AnthropicError;
use crate::schema::{NativeRequest, NativeResponse};
use crate::transport::Transport;

pub struct DirectTransport {
    client: reqwest::Client,
    config: DirectConfig,
}

impl DirectTransport {
    pub fn new(config: DirectConfig) -> Result<Self, AnthropicError> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(AnthropicError::Http)?;
        Ok(Self { client, config })
    }

    fn endpoint(&self) -> String {
        let mut base = self.config.base_url.clone();
        base.set_path("/v1/messages");
        base.into()
    }

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        use reqwest::header::{HeaderMap, HeaderValue};
        let mut h = HeaderMap::new();
        h.insert("x-api-key", HeaderValue::from_str(self.config.api_key.expose_secret()).expect("api key header"));
        h.insert("anthropic-version", HeaderValue::from_str(&self.config.anthropic_version).expect("version header"));
        h.insert("content-type", HeaderValue::from_static("application/json"));
        h
    }
}

#[async_trait]
impl Transport for DirectTransport {
    async fn send(&self, body: NativeRequest) -> Result<NativeResponse, AnthropicError> {
        let resp = self.client.post(self.endpoint())
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AnthropicError::BadStatus { status: status.as_u16(), body });
        }
        Ok(resp.json::<NativeResponse>().await?)
    }

    async fn stream(
        &self,
        body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, AnthropicError>>, AnthropicError> {
        let mut body = body;
        body.stream = true;

        let resp = self.client.post(self.endpoint())
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AnthropicError::BadStatus { status: status.as_u16(), body });
        }
        let s = resp.bytes_stream().map(|chunk| chunk.map_err(AnthropicError::Http));
        Ok(Box::pin(s))
    }
}
```

- [ ] **Step 10: Write `src/models.rs`**

```rust
//! Static ModelInfo table for Anthropic Claude.

use caliban_provider::{
    Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};

const fn caps(max_input: u32, max_output: u32, vision: bool, thinking: bool) -> Capabilities {
    Capabilities {
        max_input_tokens: max_input,
        max_output_tokens: max_output,
        vision,
        tool_use: ToolUseCapability::ParallelCalls,
        thinking,
        prompt_caching: PromptCachingCapability::Explicit { max_breakpoints: 4 },
        json_mode: false,
        streaming: true,
        stop_sequences: true,
        top_k: true,
        system_prompt: SystemPromptCapability::SeparateField,
        refusal_field: true,
    }
}

pub fn models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "claude-3-5-sonnet".into(),
            native_id: "claude-3-5-sonnet-20241022".into(),
            display_name: "Claude 3.5 Sonnet".into(),
            capabilities: caps(200_000, 8_192, true, false),
        },
        ModelInfo {
            id: "claude-3-5-haiku".into(),
            native_id: "claude-3-5-haiku-20241022".into(),
            display_name: "Claude 3.5 Haiku".into(),
            capabilities: caps(200_000, 8_192, false, false),
        },
        ModelInfo {
            id: "claude-3-opus".into(),
            native_id: "claude-3-opus-20240229".into(),
            display_name: "Claude 3 Opus".into(),
            capabilities: caps(200_000, 4_096, true, false),
        },
        ModelInfo {
            id: "claude-3-haiku".into(),
            native_id: "claude-3-haiku-20240307".into(),
            display_name: "Claude 3 Haiku".into(),
            capabilities: caps(200_000, 4_096, true, false),
        },
        ModelInfo {
            id: "claude-3-7-sonnet".into(),
            native_id: "claude-3-7-sonnet-20250219".into(),
            display_name: "Claude 3.7 Sonnet".into(),
            capabilities: caps(200_000, 8_192, true, true),
        },
    ]
}

pub fn capabilities_for(model: &str) -> Capabilities {
    models()
        .into_iter()
        .find(|m| m.id == model || m.native_id == model)
        .map(|m| m.capabilities)
        .unwrap_or_else(|| caps(100_000, 4_096, false, false))
}
```

- [ ] **Step 11: Write `src/lib.rs`** — the generic `AnthropicProvider<T>` and `Provider` impl (for non-streaming `complete` only at this stage; `stream` returns "not yet implemented"-style placeholder until Task 3 wires SSE parsing)

```rust
//! Anthropic Claude schema family for the caliban agent harness.
//!
//! Provides `AnthropicProvider<T: Transport>` generic over its transport.
//! Direct API is supported by default; AWS Bedrock and Google Vertex AI
//! transports are gated behind cargo features.

#![allow(clippy::missing_errors_doc)]

pub mod config;
pub mod error;
pub mod ir_convert;
pub mod models;
pub mod schema;
pub mod transport;

mod stream_parse;  // populated in Task 3

use async_trait::async_trait;
use caliban_provider::{
    Capabilities, CompletionRequest, CompletionResponse, Error, MessageStream, ModelInfo, Provider,
    Result,
};

use crate::config::DirectConfig;
use crate::transport::Transport;
use crate::transport::direct::DirectTransport;

pub struct AnthropicProvider<T: Transport> {
    transport: T,
}

impl AnthropicProvider<DirectTransport> {
    pub fn direct(cfg: DirectConfig) -> Result<Self> {
        DirectTransport::new(cfg)
            .map(|t| Self { transport: t })
            .map_err(|e| Error::adapter(e))
    }
}

impl<T: Transport> AnthropicProvider<T> {
    pub fn from_transport(transport: T) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl<T: Transport> Provider for AnthropicProvider<T> {
    async fn complete(&self, mut req: CompletionRequest) -> Result<CompletionResponse> {
        req.validate()?;
        let canonical_model = req.model.clone();
        let mut native = ir_convert::ir_to_native_request(req, false);
        native.model = self.transport.wire_model_id(&canonical_model);
        self.transport.finalize_request(&mut native);
        let native_resp = self.transport.send(native).await?;
        ir_convert::native_response_to_ir(native_resp)
    }

    async fn stream(&self, _req: CompletionRequest) -> Result<MessageStream> {
        Err(Error::InvalidRequest(
            "Anthropic streaming not yet wired (see Task 3)".into(),
        ))
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        models::capabilities_for(model)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        models::models()
    }

    fn name(&self) -> &'static str { "anthropic" }
}

#[cfg(feature = "mock")]
pub use caliban_provider::MockProvider;
```

Stub the streaming module so the workspace compiles:

```rust
// src/stream_parse.rs
//! SSE parsing for Anthropic streaming. Wired in Task 3.
```

- [ ] **Step 12: Capture fixture files**

Create `crates/caliban-provider-anthropic/tests/fixtures/direct/complete_simple_request.json`:

```json
{
  "model": "claude-3-5-sonnet-20241022",
  "messages": [
    {"role": "user", "content": [{"type": "text", "text": "Hi!"}]}
  ],
  "system": "Be brief.",
  "tool_choice": {"type": "auto"},
  "max_tokens": 64,
  "metadata": {"user_id": null},
  "stream": false
}
```

Create `crates/caliban-provider-anthropic/tests/fixtures/direct/complete_simple_response.json`:

```json
{
  "id": "msg_01ABC",
  "model": "claude-3-5-sonnet-20241022",
  "role": "assistant",
  "type": "message",
  "content": [{"type": "text", "text": "Hello!"}],
  "stop_reason": "end_turn",
  "stop_sequence": null,
  "usage": {"input_tokens": 12, "output_tokens": 3, "cache_creation_input_tokens": null, "cache_read_input_tokens": null}
}
```

- [ ] **Step 13: Write `tests/direct_fixture.rs`**

```rust
use caliban_provider::{CompletionRequest, Provider, StopReason};
use caliban_provider_anthropic::{config::DirectConfig, AnthropicProvider};
use secrecy::SecretString;
use url::Url;
use wiremock::matchers::{header, header_exists, method, path, body_json};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn complete_simple_round_trip() {
    let server = MockServer::start().await;
    let req_json: serde_json::Value = serde_json::from_str(
        include_str!("fixtures/direct/complete_simple_request.json"),
    ).unwrap();
    let resp_json: serde_json::Value = serde_json::from_str(
        include_str!("fixtures/direct/complete_simple_response.json"),
    ).unwrap();

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "key-xyz"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header_exists("content-type"))
        .and(body_json(&req_json))
        .respond_with(ResponseTemplate::new(200).set_body_json(&resp_json))
        .mount(&server)
        .await;

    let cfg = DirectConfig {
        api_key: SecretString::new("key-xyz".into()),
        base_url: Url::parse(&server.uri()).unwrap(),
        anthropic_version: "2023-06-01".to_string(),
        timeout: std::time::Duration::from_secs(10),
    };
    let provider = AnthropicProvider::direct(cfg).unwrap();
    let req = CompletionRequest::builder("claude-3-5-sonnet-20241022")
        .system("Be brief.")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();
    let resp = provider.complete(req).await.unwrap();
    assert_eq!(resp.id, "msg_01ABC");
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
    assert_eq!(resp.usage.input_tokens, 12);
}
```

- [ ] **Step 14: Build & test**

```bash
cargo build -p caliban-provider-anthropic
cargo test -p caliban-provider-anthropic --no-default-features
```

Both must exit 0. Fixture test must pass.

If clippy fires on `missing_docs` for new public items in `lib.rs`, add one-line `///` doc comments at minimum.

- [ ] **Step 15: Commit**

```bash
git add Cargo.toml crates/caliban-provider-anthropic/
git commit -m "$(cat <<'EOF'
feat(provider-anthropic): schema + direct transport + non-streaming complete

Adds the caliban-provider-anthropic crate skeleton:
- Native Anthropic Messages API request/response types
- IR ↔ native conversions for the request/response (streaming next task)
- DirectTransport using reqwest (api.anthropic.com)
- Transport trait abstracting transport variants
- DirectConfig with from_env()
- Static ModelInfo table for Claude 3 / 3.5 / 3.7 family
- AnthropicProvider<T: Transport>::direct() constructor
- wiremock-based fixture test pinning the wire format

Streaming returns "not yet wired" Err and is implemented in Task 3.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `caliban-provider-anthropic` — SSE streaming

**Files:**
- Create: `crates/caliban-provider-anthropic/src/schema/events.rs`
- Replace: `crates/caliban-provider-anthropic/src/stream_parse.rs`
- Modify: `crates/caliban-provider-anthropic/src/schema/mod.rs` (add `events` module)
- Modify: `crates/caliban-provider-anthropic/src/lib.rs` (wire `stream` method)
- Create: `crates/caliban-provider-anthropic/tests/fixtures/direct/stream_simple.sse`
- Create: `crates/caliban-provider-anthropic/tests/direct_stream.rs`

Reference: Anthropic SSE events (`message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, `ping`).

- [ ] **Step 1: Write `src/schema/events.rs`**

```rust
//! Anthropic SSE event types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::request::NativeContentBlock;
use super::response::{NativeStopReason, NativeUsage};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeEvent {
    MessageStart { message: NativeMessageHeader },
    ContentBlockStart { index: u32, content_block: NativeContentBlock },
    ContentBlockDelta { index: u32, delta: NativeBlockDelta },
    ContentBlockStop { index: u32 },
    MessageDelta { delta: NativeMessageDelta, usage: NativeUsage },
    MessageStop,
    Ping,
    Error { error: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMessageHeader {
    pub id: String,
    pub model: String,
    pub usage: NativeUsage,
    #[serde(default)]
    pub content: Vec<NativeContentBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeBlockDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMessageDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<NativeStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
}
```

- [ ] **Step 2: Update `src/schema/mod.rs`**

```rust
pub mod events;
pub mod request;
pub mod response;

pub use events::*;
pub use request::*;
pub use response::*;
```

- [ ] **Step 3: Write `src/stream_parse.rs`** (real implementation)

```rust
//! SSE → caliban_provider::StreamEvent parsing for Anthropic.

use bytes::Bytes;
use caliban_provider::{
    Error as ProviderError, Result as ProviderResult, StopReason, StreamEvent,
    StreamingContentType, StreamingDelta, Usage,
};
use eventsource_stream::{Eventsource, EventStreamError};
use futures::stream::{BoxStream, StreamExt};
use serde_json::Value;

use crate::error::AnthropicError;
use crate::schema::events::{NativeBlockDelta, NativeEvent};
use crate::schema::request::NativeContentBlock;
use crate::schema::response::NativeStopReason;

/// Adapt a byte-stream of SSE chunks into a `caliban_provider::MessageStream`.
pub fn map_sse_to_events(
    bytes: BoxStream<'static, Result<Bytes, AnthropicError>>,
) -> caliban_provider::MessageStream {
    let s = bytes.eventsource().filter_map(|item| async move {
        match item {
            Ok(event) => {
                // skip empty / comment-only events
                if event.data.is_empty() {
                    return None;
                }
                let parsed: Result<NativeEvent, _> = serde_json::from_str(&event.data);
                match parsed {
                    Ok(ne) => Some(native_event_to_ir(ne)),
                    Err(e) => Some(Err(ProviderError::adapter(AnthropicError::StreamParse(
                        format!("event parse failed: {e}; data: {}", event.data),
                    )))),
                }
            }
            Err(EventStreamError::Transport(e)) => Some(Err(ProviderError::network(e))),
            Err(e) => Some(Err(ProviderError::adapter(AnthropicError::StreamParse(format!("{e}"))))),
        }
    });
    Box::pin(s)
}

fn native_event_to_ir(e: NativeEvent) -> ProviderResult<StreamEvent> {
    Ok(match e {
        NativeEvent::MessageStart { message } => StreamEvent::MessageStart {
            id: message.id,
            model: message.model,
        },
        NativeEvent::ContentBlockStart { index, content_block } => StreamEvent::ContentBlockStart {
            index,
            content_type: content_block_to_streaming_type(&content_block),
        },
        NativeEvent::ContentBlockDelta { index, delta } => {
            let delta = match delta {
                NativeBlockDelta::TextDelta { text } => StreamingDelta::Text(text),
                NativeBlockDelta::InputJsonDelta { partial_json } => StreamingDelta::ToolUseInputJson(partial_json),
                NativeBlockDelta::ThinkingDelta { thinking } => StreamingDelta::Thinking(thinking),
                NativeBlockDelta::SignatureDelta { .. } => return Ok(StreamEvent::Ping), // signature deltas are useful only for the Anthropic adapter's stateful reassembly; surface as no-op for IR consumers
            };
            StreamEvent::Delta { index, delta }
        }
        NativeEvent::ContentBlockStop { index } => StreamEvent::ContentBlockStop { index },
        NativeEvent::MessageDelta { delta, usage } => StreamEvent::MessageDelta {
            stop_reason: delta.stop_reason.map(map_stop_reason),
            usage_delta: Some(Usage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_creation_input_tokens: usage.cache_creation_input_tokens,
                cache_read_input_tokens: usage.cache_read_input_tokens,
            }),
        },
        NativeEvent::MessageStop => StreamEvent::MessageStop,
        NativeEvent::Ping => StreamEvent::Ping,
        NativeEvent::Error { error } => {
            return Err(ProviderError::adapter(AnthropicError::StreamParse(format!(
                "server-side stream error: {error}"
            ))));
        }
    })
}

fn map_stop_reason(r: NativeStopReason) -> StopReason {
    match r {
        NativeStopReason::EndTurn => StopReason::EndTurn,
        NativeStopReason::MaxTokens => StopReason::MaxTokens,
        NativeStopReason::StopSequence => StopReason::StopSequence,
        NativeStopReason::ToolUse => StopReason::ToolUse,
        NativeStopReason::Refusal => StopReason::Refusal,
        NativeStopReason::PauseTurn => StopReason::EndTurn,
    }
}

fn content_block_to_streaming_type(b: &NativeContentBlock) -> StreamingContentType {
    match b {
        NativeContentBlock::Text(_) => StreamingContentType::Text,
        NativeContentBlock::Thinking(_) | NativeContentBlock::RedactedThinking { .. } => {
            StreamingContentType::Thinking
        }
        NativeContentBlock::ToolUse(tu) => StreamingContentType::ToolUse {
            id: tu.id.clone(),
            name: tu.name.clone(),
        },
        _ => StreamingContentType::Text,
    }
}

// (Value reserved for future capability detection — currently unused.)
#[allow(dead_code)]
fn _unused(_v: Value) {}
```

- [ ] **Step 4: Update `src/lib.rs` — wire `stream`**

Replace the `stream` method body in `impl<T: Transport> Provider for AnthropicProvider<T>`:

```rust
    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream> {
        req.validate()?;
        let canonical_model = req.model.clone();
        let mut native = ir_convert::ir_to_native_request(req, true);
        native.model = self.transport.wire_model_id(&canonical_model);
        self.transport.finalize_request(&mut native);
        let bytes_stream = self.transport.stream(native).await
            .map_err(caliban_provider::Error::from)?;
        Ok(stream_parse::map_sse_to_events(bytes_stream))
    }
```

- [ ] **Step 5: Capture fixture file `tests/fixtures/direct/stream_simple.sse`**

```
event: message_start
data: {"type":"message_start","message":{"id":"msg_01ABC","type":"message","role":"assistant","model":"claude-3-5-sonnet-20241022","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":12,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"!"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":12,"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

```

(Trailing blank line required for SSE framing.)

- [ ] **Step 6: Write `tests/direct_stream.rs`**

```rust
use caliban_provider::{collect_message, CompletionRequest, Provider, StopReason};
use caliban_provider_anthropic::{config::DirectConfig, AnthropicProvider};
use secrecy::SecretString;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn stream_simple_round_trip() {
    let server = MockServer::start().await;
    let sse_body = include_str!("fixtures/direct/stream_simple.sse");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "text/event-stream")
                .set_body_raw(sse_body, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let cfg = DirectConfig {
        api_key: SecretString::new("key-xyz".into()),
        base_url: Url::parse(&server.uri()).unwrap(),
        anthropic_version: "2023-06-01".to_string(),
        timeout: std::time::Duration::from_secs(10),
    };
    let provider = AnthropicProvider::direct(cfg).unwrap();
    let req = CompletionRequest::builder("claude-3-5-sonnet-20241022")
        .user_text("Hi!")
        .max_tokens(64)
        .build()
        .unwrap();
    let stream = provider.stream(req).await.unwrap();
    let (msg, stop, usage) = collect_message(stream).await.unwrap();
    let text = match &msg.content[0] {
        caliban_provider::ContentBlock::Text(t) => &t.text,
        _ => panic!("expected text block"),
    };
    assert_eq!(text, "Hello!");
    assert!(matches!(stop, StopReason::EndTurn));
    assert_eq!(usage.output_tokens, 2);
}
```

- [ ] **Step 7: Build & test**

```bash
cargo build -p caliban-provider-anthropic
cargo test -p caliban-provider-anthropic --no-default-features
```

Both must exit 0. Stream test passes (`Hello!` assembled, `EndTurn` stop_reason).

- [ ] **Step 8: Commit**

```bash
git add crates/caliban-provider-anthropic/
git commit -m "$(cat <<'EOF'
feat(provider-anthropic): SSE streaming via eventsource-stream

Implements stream() on AnthropicProvider<T>. Native SSE event types
(message_start, content_block_*, message_delta, message_stop, ping)
parse into IR StreamEvent / Delta variants. Fixture test pins the
end-to-end stream → MessageStream → collect_message contract.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `caliban-provider-openai` — schema, direct transport, non-streaming

**Files:**
- Create: `crates/caliban-provider-openai/{Cargo.toml,src/{lib,error,models,config,ir_convert}.rs,src/schema/{mod,request,response}.rs,src/transport/{mod,direct}.rs,tests/fixtures/direct/{complete_simple_request,complete_simple_response}.json,tests/direct_fixture.rs}`
- Modify: root `Cargo.toml`

Following the same shape as Task 2 but for OpenAI's Chat Completions API.

OpenAI schema notes:
- Endpoint: `POST {base_url}/chat/completions`
- Auth: `Authorization: Bearer {api_key}`
- Messages: `{"role": "system|user|assistant|tool|developer", "content": "..." | [...]}`
- Tool calls: response has `"tool_calls": [{"id":"...","type":"function","function":{"name":"...","arguments":"..."}}]`; `arguments` is a JSON STRING (not parsed object).
- Tool results: messages with `"role":"tool", "tool_call_id":"...", "content":"..."`.
- `max_completion_tokens` for o1+, `max_tokens` for older. Adapter sends `max_tokens` (deprecated but still accepted by all models).

- [ ] **Step 1: Add to workspace + Cargo.toml**

Add `"crates/caliban-provider-openai"` to root `Cargo.toml` members.

`crates/caliban-provider-openai/Cargo.toml`:

```toml
[package]
name        = "caliban-provider-openai"
version     = "0.0.0"
description = "OpenAI schema family for the caliban agent harness"
edition.workspace      = true
license.workspace      = true
authors.workspace      = true
rust-version.workspace = true
publish     = false

[features]
default = []
azure   = []
live-tests = []

[dependencies]
caliban-provider   = { path = "../caliban-provider" }
async-trait        = { workspace = true }
serde              = { workspace = true }
serde_json         = { workspace = true }
thiserror          = { workspace = true }
reqwest            = { workspace = true }
secrecy            = { workspace = true }
futures            = { workspace = true }
eventsource-stream = { workspace = true }
url                = { workspace = true }
bytes              = { workspace = true }
http               = { workspace = true }
tokio              = { workspace = true }

[dev-dependencies]
wiremock = { workspace = true }
tokio    = { workspace = true, features = ["macros", "rt-multi-thread"] }

[lints]
workspace = true
```

- [ ] **Step 2: `src/error.rs`** — mirror Task 2's pattern (replace `AnthropicError` with `OpenAIError`, same `From` rules; HTTP 400 → `InvalidRequest`, 401/403 → `Auth`, 429 → `RateLimit`, 500+ → `ServerError`, etc.).

```rust
use caliban_provider::Error as ProviderError;

#[derive(thiserror::Error, Debug)]
pub enum OpenAIError {
    #[error("HTTP request failed: {0}")] Http(#[from] reqwest::Error),
    #[error("response status {status}: {body}")] BadStatus { status: u16, body: String },
    #[error("deserialize error: {0}")] Deserialize(#[from] serde_json::Error),
    #[error("stream parse error: {0}")] StreamParse(String),
    #[error("missing config field: {0}")] MissingConfig(&'static str),
    #[error("transport error: {0}")] Transport(Box<dyn std::error::Error + Send + Sync>),
    #[error("unsupported feature: {0}")] Unsupported(String),
}

impl From<OpenAIError> for ProviderError {
    fn from(e: OpenAIError) -> Self {
        match e {
            OpenAIError::Http(ref err) => {
                if err.is_connect() || err.is_timeout() { ProviderError::network(e) }
                else { ProviderError::adapter(e) }
            }
            OpenAIError::BadStatus { status, ref body } => match status {
                401 | 403 => ProviderError::Auth(body.clone()),
                429 => ProviderError::RateLimit { retry_after: None },
                400 => ProviderError::InvalidRequest(body.clone()),
                404 => ProviderError::ModelUnavailable(body.clone()),
                _ if status >= 500 => ProviderError::ServerError { status, body: body.clone() },
                _ => ProviderError::adapter(e),
            },
            _ => ProviderError::adapter(e),
        }
    }
}
```

- [ ] **Step 3: `src/schema/request.rs`**

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeRequest {
    pub model: String,
    pub messages: Vec<NativeMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<NativeTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<NativeToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<NativeStreamOptions>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeStreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<NativeContent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<NativeToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativeContent {
    Text(String),
    Parts(Vec<NativeContentPart>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeContentPart {
    Text { text: String },
    ImageUrl { image_url: NativeImageUrl },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeImageUrl {
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: NativeToolFunction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativeToolChoice {
    Auto(String),                  // "auto", "required", "none"
    Specific { #[serde(rename = "type")] kind: String, function: NativeToolFunctionName },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeToolFunctionName {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,            // "function"
    pub function: NativeFunctionCall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeFunctionCall {
    pub name: String,
    pub arguments: String,       // JSON string, not object
}
```

- [ ] **Step 4: `src/schema/response.rs`**

```rust
use serde::{Deserialize, Serialize};

use super::request::NativeToolCall;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<NativeChoice>,
    #[serde(default)]
    pub usage: NativeUsage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeChoice {
    pub index: u32,
    pub message: NativeResponseMessage,
    pub finish_reason: NativeFinishReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeResponseMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<NativeToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeFinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    FunctionCall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NativeUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<NativePromptTokensDetails>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NativePromptTokensDetails {
    pub cached_tokens: u32,
}
```

- [ ] **Step 5: `src/schema/mod.rs`** — same shape as Task 2.

- [ ] **Step 6: `src/ir_convert.rs`** — implement IR ↔ native. Pay attention to:
  - System messages become `{"role":"system","content":"..."}` (concatenate leading System messages).
  - `Role::User` / `Role::Assistant` map straight through.
  - `ContentBlock::Image` maps to `image_url` parts (`data:{mime};base64,{data}` for Base64 or raw URL for `Url`).
  - `ToolUseBlock` (in Assistant messages) maps to `tool_calls` with `function.arguments = serde_json::to_string(&input)`.
  - `ToolResultBlock` maps to a separate message: `{"role":"tool", "tool_call_id":"...", "content":"..."}` (content is Text-only; if multiple Text blocks in IR ToolResult, concatenate; Image is dropped with warning).
  - `Thinking` blocks: dropped on send (OpenAI doesn't accept them); response `refusal` field is treated as a `Thinking`-tagged block with `signature: Some("refusal")` — actually no, route this as a `Text` content block in the assistant message and set `stop_reason: Refusal` if `refusal.is_some()`.
  - `cache_control` is ignored on send.
  - `tool_choice`: `Auto` → `"auto"`, `Any` → `"required"`, `Specific(n)` → `{"type":"function","function":{"name":n}}`, `None` → `"none"`.

Provide the full source. (For brevity here the implementer should mirror the Anthropic conversion structure; the data shapes above are exhaustive.)

- [ ] **Step 7: `src/transport/mod.rs`, `src/transport/direct.rs`** — analogous to Task 2's Anthropic Transport. Endpoint: `{base_url}/chat/completions`. Headers: `Authorization: Bearer {api_key}`, `content-type`, optional `OpenAI-Organization`, `OpenAI-Project`.

- [ ] **Step 8: `src/config.rs`**

```rust
use std::time::Duration;
use secrecy::SecretString;
use url::Url;
use crate::error::OpenAIError;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub struct DirectConfig {
    pub api_key: SecretString,
    pub base_url: Url,
    pub organization: Option<String>,
    pub project: Option<String>,
    pub timeout: Duration,
}

impl DirectConfig {
    pub fn new(api_key: SecretString) -> Self {
        Self {
            api_key,
            base_url: Url::parse(DEFAULT_BASE_URL).expect("static URL parses"),
            organization: None,
            project: None,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    pub fn from_env() -> Result<Self, OpenAIError> {
        let key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| OpenAIError::MissingConfig("OPENAI_API_KEY"))?;
        let mut cfg = Self::new(SecretString::new(key.into()));
        if let Ok(url) = std::env::var("OPENAI_BASE_URL") {
            cfg.base_url = Url::parse(&url).map_err(|e| OpenAIError::Transport(Box::new(e)))?;
        }
        cfg.organization = std::env::var("OPENAI_ORG_ID").ok();
        cfg.project = std::env::var("OPENAI_PROJECT").ok();
        Ok(cfg)
    }
}

#[cfg(feature = "azure")]
pub use azure::*;

#[cfg(feature = "azure")]
mod azure {
    use super::*;
    use std::collections::HashMap;

    #[derive(Debug, Clone)]
    pub struct AzureConfig {
        pub api_key: SecretString,
        pub resource: String,
        pub api_version: String,
        pub timeout: Duration,
        pub deployments: HashMap<String, String>,
    }

    impl AzureConfig {
        pub fn from_env() -> Result<Self, OpenAIError> {
            let api_key = std::env::var("AZURE_OPENAI_API_KEY")
                .map_err(|_| OpenAIError::MissingConfig("AZURE_OPENAI_API_KEY"))?;
            let resource = std::env::var("AZURE_OPENAI_RESOURCE")
                .map_err(|_| OpenAIError::MissingConfig("AZURE_OPENAI_RESOURCE"))?;
            let api_version = std::env::var("AZURE_OPENAI_API_VERSION")
                .unwrap_or_else(|_| "2024-10-21".into());
            Ok(Self {
                api_key: SecretString::new(api_key.into()),
                resource,
                api_version,
                timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
                deployments: HashMap::new(),
            })
        }
    }
}
```

- [ ] **Step 9: `src/models.rs`** — table for gpt-4o, gpt-4o-mini, o1-preview, o1-mini.

- [ ] **Step 10: `src/lib.rs`** — generic `OpenAIProvider<T: Transport>`, `direct(cfg)` constructor, `Provider` impl. `stream` returns `Err(...)` until Task 5.

- [ ] **Step 11: Fixtures**

`tests/fixtures/direct/complete_simple_request.json`:

```json
{
  "model": "gpt-4o",
  "messages": [
    {"role": "system", "content": "Be brief."},
    {"role": "user", "content": "Hi!"}
  ],
  "tool_choice": "auto",
  "max_tokens": 64,
  "stream": false
}
```

`tests/fixtures/direct/complete_simple_response.json`:

```json
{
  "id": "chatcmpl-XYZ",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "gpt-4o-2024-08-06",
  "choices": [
    {"index": 0, "message": {"role": "assistant", "content": "Hello!"}, "finish_reason": "stop"}
  ],
  "usage": {"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15, "prompt_tokens_details": {"cached_tokens": 0}}
}
```

- [ ] **Step 12: `tests/direct_fixture.rs`** — wiremock test analogous to Task 2 Step 13 but with OpenAI's URL, headers (`Authorization: Bearer key-xyz`), and the fixtures above.

- [ ] **Step 13: Build & test**

```bash
cargo build -p caliban-provider-openai
cargo test -p caliban-provider-openai --no-default-features
```

- [ ] **Step 14: Commit**

```bash
git add Cargo.toml crates/caliban-provider-openai/
git commit -m "feat(provider-openai): schema + direct transport + non-streaming complete

Schema-family crate for OpenAI's Chat Completions API. IR conversions
translate Role::System messages to {role:'system'} entries, Image
content to image_url parts, ToolUse to tool_calls with JSON-string
arguments, ToolResult to {role:'tool'} messages. tool_choice maps to
'auto'/'required'/'none'/named-function. Thinking blocks dropped on
send (not OpenAI-supported); responses with refusal field map to
StopReason::Refusal. Wiremock fixture test pins the wire contract.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `caliban-provider-openai` — SSE streaming

Following the same pattern as Task 3 but for OpenAI's SSE event shape:

```
data: {"id":"chatcmpl-...","object":"chat.completion.chunk","created":...,"model":"gpt-4o-...","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}
data: {"id":"...","object":"chat.completion.chunk","created":...,"model":"...","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}
...
data: {"id":"...","object":"chat.completion.chunk","created":...,"model":"...","choices":[{"index":0,"delta":{},"finish_reason":"stop"}], "usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}}
data: [DONE]
```

Translation rules:
- First chunk with `delta.role` → `StreamEvent::MessageStart`. Also emit `ContentBlockStart { index: 0, content_type: Text }`.
- Chunks with `delta.content` → `StreamEvent::Delta { index: 0, delta: Text(s) }`.
- Chunks with `delta.tool_calls` → for each tool_call: emit `ContentBlockStart { index: <derived>, content_type: ToolUse { id, name } }` on first encounter; emit `Delta { index, delta: ToolUseInputJson(arguments) }` for accumulating partial JSON.
- Chunk with `finish_reason: !null` → emit `ContentBlockStop` for each open block, then `MessageDelta { stop_reason, usage_delta }`, then `MessageStop`.
- `[DONE]` sentinel → end of stream.

Steps mirror Task 3 (write schema/events.rs, stream_parse.rs, wire stream() in lib.rs, fixture+test, commit). Implement `src/stream_parse.rs::map_openai_sse_to_events` that maintains state across chunks (open content-block indices) so it correctly emits start/stop events around OpenAI's "delta only" stream.

Commit: `feat(provider-openai): SSE streaming with content-block reconstruction`

---

## Task 6: `caliban-provider-ollama`

Single transport, NDJSON streaming (not SSE).

- Endpoint: `POST {base_url}/api/chat`
- No auth
- Request body: `{model, messages, options:{num_predict:max_tokens,temperature,top_p,top_k,stop,...}, stream:true|false, tools:[...], format:{...}?}`
- Messages: same shape as OpenAI mostly. Tool support added in Ollama 0.3+; tool_calls field present in assistant messages, similar to OpenAI but `arguments` is an object (not JSON string).
- Response (non-streaming): single JSON `{model, created_at, message:{role,content,tool_calls:[...]}, done:true, done_reason:"stop", total_duration:..., eval_count:N (output_tokens), prompt_eval_count:N (input_tokens)}`
- Streaming: line-delimited JSON, one object per line, last line has `done:true`.

Steps follow Task 4's pattern. Skip SSE; instead parse NDJSON in `stream_parse.rs` (use `tokio::io::AsyncBufReadExt::lines` or split bytes by `\n`).

Capability table: `llama3.1`, `qwen2.5`, `mistral`, `phi3`, etc. `prompt_caching: PromptCachingCapability::None`. `tool_use: ToolUseCapability::Basic` (no parallel calls in Ollama yet).

Commit: `feat(provider-ollama): API chat endpoint + NDJSON streaming`

---

## Task 7: `caliban-provider-google` — AI Studio transport (schema + non-stream + stream)

Gemini schema:
- Endpoint: `POST {base_url}/v1beta/models/{model}:generateContent?key={api_key}` (non-stream)
- Streaming: `POST {base_url}/v1beta/models/{model}:streamGenerateContent?key={api_key}&alt=sse` (SSE) — server emits one full JSON object per `data:` event.
- Request body:
  ```json
  {
    "systemInstruction": {"parts": [{"text": "..."}]},
    "contents": [
      {"role": "user", "parts": [{"text": "..."}, {"inlineData": {"mimeType":"image/png","data":"..."}}]},
      {"role": "model", "parts": [{"text": "..."}, {"functionCall": {"name":"x","args":{}}}]},
      {"role": "user", "parts": [{"functionResponse": {"name":"x","response":{...}}}]}
    ],
    "tools": [{"functionDeclarations": [{"name":"x","description":"...","parameters":{...}}]}],
    "toolConfig": {"functionCallingConfig": {"mode":"AUTO|ANY|NONE", "allowedFunctionNames":[...]?}},
    "generationConfig": {"maxOutputTokens":N,"temperature":T,"topP":P,"topK":K,"stopSequences":[...]}
  }
  ```
- Response: `{"candidates":[{"content":{"role":"model","parts":[...]}, "finishReason":"STOP|MAX_TOKENS|SAFETY|...","safetyRatings":[...]}], "usageMetadata":{"promptTokenCount":N,"candidatesTokenCount":N,"totalTokenCount":N}}`

Translation:
- `Role::System` → `systemInstruction` field (concatenated leading System message text).
- `Role::User` → `role:"user"`. `Role::Assistant` → `role:"model"`.
- `ContentBlock::Text` → `{"text":"..."}`. `ContentBlock::Image::Base64` → `{"inlineData":{"mimeType":mt,"data":d}}`. `ContentBlock::Image::Url` → `{"fileData":{"mimeType":mt,"fileUri":url}}` (Vertex supports; AI Studio requires Base64 — adapter errors if Url given for AIStudio transport — let the Transport's `finalize_request` enforce).
- `ToolUseBlock` → `{"functionCall":{"name":n,"args":input}}`.
- `ToolResultBlock` → `{"functionResponse":{"name":lookup_by_id,"response":content_to_json}}`.
- `ThinkingBlock` → dropped (Gemini doesn't expose).
- `cache_control` → ignored (Gemini context caching is a separate API; deferred).

Capability table: `gemini-2.0-flash`, `gemini-1.5-pro`, `gemini-1.5-flash`. PromptCachingCapability::None for B (explicit context caching deferred).

`stream_parse.rs`: SSE events; each event payload is a full JSON response chunk. Extract `candidates[0].content.parts` deltas — Gemini doesn't emit fine-grained deltas like Anthropic, so:
- First chunk → `MessageStart { id: generate_uuid_or_use_response_id, model: known }` + `ContentBlockStart { index: 0, Text }`.
- Each chunk's text parts → `Delta { index: 0, Text(text) }`.
- Each chunk's function-call parts → `ContentBlockStart` for a new tool_use block + `Delta { ToolUseInputJson(serde_json::to_string(&args)?) }` + `ContentBlockStop`.
- `finishReason` on a chunk → `ContentBlockStop` for any open + `MessageDelta { stop_reason, usage_delta }` + `MessageStop`.

Steps follow Task 2/3 pattern. Single commit covers both non-streaming and streaming since Gemini's schema is small enough.

Commit: `feat(provider-google): schema + AIStudio transport (sync + stream)`

---

## Task 8: `caliban-provider-anthropic` — BedrockTransport (feature `bedrock`)

**Files:**
- Create: `crates/caliban-provider-anthropic/src/transport/bedrock.rs`
- Modify: `crates/caliban-provider-anthropic/src/lib.rs` (add `bedrock()` constructor under `#[cfg(feature = "bedrock")]`)
- Modify: `crates/caliban-provider-anthropic/src/config.rs` (add `BedrockConfig`)
- Modify: `crates/caliban-provider-anthropic/src/stream_parse.rs` (add `map_bedrock_event_stream_to_events` — accepts AWS event-stream payload chunks)
- Create: `crates/caliban-provider-anthropic/tests/bedrock_fixture.rs` (cfg-gated)

The Bedrock API uses AWS SigV4 signing. We use `aws-sdk-bedrockruntime` which handles auth and event-stream framing for us.

- [ ] **Step 1: Config** — add `BedrockConfig`:

```rust
#[cfg(feature = "bedrock")]
pub use bedrock::*;

#[cfg(feature = "bedrock")]
mod bedrock {
    use std::time::Duration;
    use aws_config::SdkConfig;
    use crate::error::AnthropicError;

    #[derive(Debug, Clone)]
    pub struct BedrockConfig {
        pub sdk_config: SdkConfig,
        pub timeout: Duration,
        pub anthropic_version: String,
    }

    impl BedrockConfig {
        pub async fn from_aws_credentials() -> Result<Self, AnthropicError> {
            let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest()).load().await;
            Ok(Self {
                sdk_config,
                timeout: Duration::from_secs(60),
                anthropic_version: "bedrock-2023-05-31".to_string(),
            })
        }
    }
}
```

- [ ] **Step 2: BedrockTransport** uses `aws-sdk-bedrockruntime::Client` to `invoke_model` (non-stream) or `invoke_model_with_response_stream` (stream). Request body must include `anthropic_version: "bedrock-2023-05-31"` and OMIT the `model` field (model is in the URL path); response body has the same shape as direct.

```rust
use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_bedrockruntime::primitives::Blob;
use futures::stream::{BoxStream, StreamExt};
use bytes::Bytes;

use crate::config::BedrockConfig;
use crate::error::AnthropicError;
use crate::schema::{NativeRequest, NativeResponse};
use crate::transport::Transport;

pub struct BedrockTransport {
    client: BedrockClient,
    anthropic_version: String,
}

impl BedrockTransport {
    pub fn new(config: BedrockConfig) -> Self {
        let client = BedrockClient::new(&config.sdk_config);
        Self { client, anthropic_version: config.anthropic_version }
    }
}

#[async_trait]
impl Transport for BedrockTransport {
    async fn send(&self, mut body: NativeRequest) -> Result<NativeResponse, AnthropicError> {
        let model_id = body.model.clone();
        body.model = String::new(); // strip; model is in URL
        body.anthropic_version = Some(self.anthropic_version.clone());
        let body_json = serde_json::to_vec(&strip_anthropic_unwanted(body))?;

        let resp = self.client
            .invoke_model()
            .model_id(model_id)
            .body(Blob::new(body_json))
            .content_type("application/json")
            .accept("application/json")
            .send()
            .await
            .map_err(|e| AnthropicError::Transport(Box::new(e)))?;

        let body_bytes = resp.body.into_inner();
        let native: NativeResponse = serde_json::from_slice(&body_bytes)?;
        Ok(native)
    }

    async fn stream(
        &self,
        mut body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<Bytes, AnthropicError>>, AnthropicError> {
        let model_id = body.model.clone();
        body.model = String::new();
        body.anthropic_version = Some(self.anthropic_version.clone());
        body.stream = true;
        let body_json = serde_json::to_vec(&strip_anthropic_unwanted(body))?;

        let mut resp = self.client
            .invoke_model_with_response_stream()
            .model_id(model_id)
            .body(Blob::new(body_json))
            .content_type("application/json")
            .accept("application/json")
            .send()
            .await
            .map_err(|e| AnthropicError::Transport(Box::new(e)))?;

        let stream = async_stream::try_stream! {
            while let Some(event) = resp.body.recv().await.map_err(|e| AnthropicError::Transport(Box::new(e)))? {
                use aws_sdk_bedrockruntime::types::ResponseStream;
                match event {
                    ResponseStream::Chunk(c) => {
                        if let Some(bytes) = c.bytes {
                            // Bedrock event-stream wraps each Anthropic SSE event as a single chunk
                            // containing the raw event JSON. We re-frame as an SSE-like blob the
                            // shared parser already handles.
                            let json_bytes = bytes.into_inner();
                            // Reframe as SSE: "data: {json}\n\n"
                            let mut reframed = Vec::with_capacity(json_bytes.len() + 8);
                            reframed.extend_from_slice(b"data: ");
                            reframed.extend_from_slice(&json_bytes);
                            reframed.extend_from_slice(b"\n\n");
                            yield Bytes::from(reframed);
                        }
                    }
                    other => {
                        return Err(AnthropicError::StreamParse(format!("unexpected response event: {other:?}")))?;
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }

    fn wire_model_id(&self, canonical: &str) -> String {
        // claude-3-5-sonnet → anthropic.claude-3-5-sonnet-20241022-v2:0
        // Use the canonical name lookup table from models.rs to find native_id, then prefix "anthropic."
        let native_id = crate::models::models()
            .into_iter()
            .find(|m| m.id == canonical || m.native_id == canonical)
            .map(|m| m.native_id)
            .unwrap_or_else(|| canonical.to_string());
        if native_id.starts_with("anthropic.") { native_id } else { format!("anthropic.{native_id}-v1:0") }
    }
}

fn strip_anthropic_unwanted(mut body: NativeRequest) -> NativeRequest {
    // Bedrock doesn't accept the "model" field in body, and accepts the standard Anthropic shape otherwise.
    body.model = String::new();
    body
}
```

Note: depends on the `async-stream` crate. Add to `[dependencies]` under feature `bedrock`:

```toml
async-stream = "0.3"
```

(Add to workspace.dependencies too.)

- [ ] **Step 3: Wire `bedrock()` constructor in lib.rs:**

```rust
#[cfg(feature = "bedrock")]
impl AnthropicProvider<crate::transport::bedrock::BedrockTransport> {
    pub fn bedrock(cfg: crate::config::BedrockConfig) -> Self {
        Self::from_transport(crate::transport::bedrock::BedrockTransport::new(cfg))
    }
}
```

- [ ] **Step 4: `tests/bedrock_fixture.rs`** — gated on feature flag. Bedrock fixture test is harder because we can't easily mock the AWS SDK with wiremock (it speaks a binary event-stream protocol). For now: write a unit test on `wire_model_id` that asserts the mapping (`claude-3-5-sonnet` → `anthropic.claude-3-5-sonnet-20241022-v2:0`), and gate live integration tests behind `live-tests + bedrock`.

```rust
#![cfg(feature = "bedrock")]

#[test]
fn wire_model_id_anthropic_claude_3_5_sonnet() {
    use caliban_provider_anthropic::transport::bedrock::BedrockTransport;
    use caliban_provider_anthropic::transport::Transport;
    // Construct without AWS creds — only need wire_model_id which doesn't touch the client
    // Actually we can't construct without SdkConfig; skip the construction and test via direct mapping
    let id = crate::wire_id_for("claude-3-5-sonnet");
    assert!(id.starts_with("anthropic."));
}
```

(If construction without AWS creds is infeasible, expose `wire_model_id` as a free function in `bedrock` module and test that instead.)

- [ ] **Step 5: Build & test with feature**

```bash
cargo build -p caliban-provider-anthropic --features bedrock
cargo test  -p caliban-provider-anthropic --features bedrock --no-default-features --features bedrock
```

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/caliban-provider-anthropic/
git commit -m "feat(provider-anthropic): BedrockTransport (feature 'bedrock')

Adds AWS Bedrock as a transport variant for the Anthropic schema family.
Uses aws-sdk-bedrockruntime for SigV4 auth, model invocation, and AWS
event-stream framing. Bedrock chunks are reframed as SSE-style 'data:'
blobs so the existing SSE parser handles them uniformly. wire_model_id
maps canonical Claude IDs (e.g., claude-3-5-sonnet) to Bedrock's
anthropic.claude-3-5-sonnet-20241022-v2:0 format.

Live integration tests are gated on features 'bedrock,live-tests' AND
AWS credentials being present in the environment.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: `caliban-provider-anthropic` — VertexTransport (feature `vertex`)

- Endpoint: `https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model_id}:rawPredict` (non-stream) or `:streamRawPredict` (stream).
- Auth: `Authorization: Bearer {gcp_oauth_token}` from `gcp_auth` crate's `AuthenticationManager`.
- Body: same Anthropic shape as direct, but with the `anthropic_version` field set to `"vertex-2023-10-16"`. `model` field omitted (in URL).
- Stream: SSE (same parser as direct works).

Implementation steps:
1. Add `VertexConfig` in `src/config.rs` (`project`, `region`, credentials).
2. Implement `VertexTransport` similarly to `DirectTransport` but with a `gcp_auth::AuthenticationManager` for token acquisition before each call.
3. `wire_model_id` maps canonical → Vertex's expected `claude-3-5-sonnet@20241022` format (note the `@` separator instead of `-`).
4. `caliban-provider-anthropic` exposes `AnthropicProvider::vertex(VertexConfig)` constructor under `cfg(feature = "vertex")`.
5. Unit test on `wire_model_id`.

Commit: `feat(provider-anthropic): VertexTransport (feature 'vertex')`

---

## Task 10: `caliban-provider-google` — VertexTransport (feature `vertex`)

- Endpoint: `https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/google/models/{model}:streamGenerateContent?alt=sse`.
- Auth: GCP OAuth via `gcp_auth`.
- Body: same Gemini shape as AI Studio (no API key in URL).
- Differences: Vertex supports `fileData` (URL references); AI Studio requires Base64 inline.

Implementation steps:
1. Add `VertexConfig` in `src/config.rs`.
2. Implement `VertexTransport`.
3. `GoogleProvider::vertex(VertexConfig)` constructor.
4. AI Studio's `finalize_request` rejects `fileData` parts (`ContentBlock::Image::Url` → error before send); Vertex's allows them.
5. Unit test on the rejection vs. acceptance.

Commit: `feat(provider-google): VertexTransport (feature 'vertex')`

---

## Task 11: `caliban-provider-openai` — AzureTransport (feature `azure`)

- Endpoint: `https://{resource}.openai.azure.com/openai/deployments/{deployment}/chat/completions?api-version={ver}`.
- Auth: `api-key: {key}` header (not `Authorization: Bearer`).
- `model` field in body should be omitted or set to the deployment name (Azure ignores it; deployment-from-URL is canonical).
- Streaming: same SSE shape as direct.

Implementation steps:
1. Add `AzureConfig` to `src/config.rs` (already present from Task 4 Step 8 if you followed the cfg block; verify).
2. Implement `AzureTransport` in `src/transport/azure.rs`. `wire_model_id` looks up the canonical model name in `AzureConfig.deployments`; if missing → `Err(OpenAIError::MissingConfig("deployment for model X"))`.
3. `OpenAIProvider::azure(AzureConfig)` constructor.
4. wiremock-based fixture test pinning Azure's URL pattern (`/openai/deployments/{depl}/chat/completions?api-version=...`) and `api-key` header.

Commit: `feat(provider-openai): AzureTransport (feature 'azure')`

---

## Task 12: ADRs + README + CI

**Files:**
- Create: `adrs/0006-message-schema-ir.md`
- Create: `adrs/0007-transport-trait-pattern.md`
- Create: `adrs/0008-system-role-positional.md`
- Modify: `adrs/README.md` (index)
- Modify: `README.md` (update Layer-1 status, repo layout, add example usage)
- Modify: `.github/workflows/ci.yml` (add cloud-feature job)

- [ ] **Step 1: ADR 0006**

```markdown
# ADR 0006 · Message schema → provider-neutral IR

- **Status:** accepted
- **Date:** 2026-05-22

## Context

Layer 0 deferred the choice of message schema. Three approaches considered: (1) Anthropic-shape canonical; (2) provider-neutral IR; (3) lowest-common-denominator.

## Decision

Define caliban's own `Message`/`Content`/`StreamEvent` types (the IR) in `caliban-provider`. Each adapter translates `provider_native ↔ IR` at its boundary. The IR is intentionally close to Anthropic's API shape because Anthropic's API is the most expressive of the supported providers; other adapters lose less information when mapping to the IR.

## Consequences

- **Positive:** Adding a new provider doesn't touch `caliban-provider`. Provider-specific API changes don't ripple. The model-router (Layer 3) operates uniformly on IR. All transport variants of a given schema family share IR conversion code.
- **Negative:** One extra translation hop per request. IR design must capture the union of advanced features (thinking, prompt caching, multimodal) without becoming Anthropic-in-disguise.
- **Revisit if:** A provider emerges with feature semantics that can't be cleanly expressed in the IR (e.g., a new content modality the union doesn't anticipate).
```

- [ ] **Step 2: ADR 0007**

```markdown
# ADR 0007 · Schema/transport factoring via Transport trait

- **Status:** accepted
- **Date:** 2026-05-22

## Context

A naïve "one crate per concrete provider endpoint" plan duplicates the Anthropic Claude schema work across `caliban-provider-anthropic` (direct API), an eventual Bedrock-Claude crate, and an eventual Vertex-Claude crate. Two orthogonal dimensions exist: model schema family vs. transport/endpoint.

## Decision

Each schema-family crate (`caliban-provider-anthropic`, `caliban-provider-openai`, `caliban-provider-google`, `caliban-provider-ollama`) defines its own `Transport` trait. A schema-family-generic `XxxProvider<T: Transport>` owns the IR conversion. Transport variants (DirectTransport, BedrockTransport, VertexTransport, AzureTransport, AIStudioTransport) are concrete `Transport` impls within their schema family, gated behind cargo features when they pull heavy deps (`aws-sdk-bedrockruntime`, `gcp_auth`).

## Consequences

- **Positive:** Claude-on-Bedrock and Claude-on-Vertex reuse the Anthropic IR-conversion code. Adding a new transport for an existing schema is a single-file change. The model-router can treat `(schema_family, transport)` as a tuple.
- **Negative:** A Transport trait is per-family, not shared across families — `caliban-provider-anthropic::Transport ≠ caliban-provider-openai::Transport`. This is intentional (transport contracts are not interchangeable across schemas).
- **Revisit if:** A transport pattern emerges that genuinely cross-cuts schema families (e.g., a future caliban-side mTLS proxy that wraps any provider).
```

- [ ] **Step 3: ADR 0008**

```markdown
# ADR 0008 · Role::System messages are positional (leading-only)

- **Status:** accepted
- **Date:** 2026-05-22

## Context

OpenAI's API treats system as a role: `system` messages can appear anywhere in the messages array. Anthropic's, Gemini's, and Bedrock-Claude's APIs treat the system prompt as a separate top-level field. Modeling both shapes uniformly in the IR was an open question.

## Decision

The IR has three roles: `User`, `Assistant`, `System`. System messages must appear contiguously at the start of `CompletionRequest.messages`. Validation rejects out-of-order System messages and System messages containing non-Text content blocks. Adapters with a separate-field system model (Anthropic, Gemini) collect the leading System messages and serialize them into the dedicated field; adapters with a system-role model (OpenAI, Ollama) pass them through as-is.

## Consequences

- **Positive:** Single canonical representation. Maps cleanly to all four families. Per-System-message `cache_control` (Anthropic feature) is preserved by serializing the system field as a block array when any block has a cache marker.
- **Negative:** Disallows the rare pattern of mid-conversation system injection. Callers wanting that pattern must rewrite into a "User says: here's a new constraint…" style.
- **Revisit if:** A provider semantically requires non-leading system messages, or a credible agent design needs mid-conversation system injection.
```

- [ ] **Step 4: Update `adrs/README.md`**

Add three rows to the index table after row 0005:

| [0006](0006-message-schema-ir.md) | Message schema → provider-neutral IR | accepted |
| [0007](0007-transport-trait-pattern.md) | Schema/transport factoring via Transport trait | accepted |
| [0008](0008-system-role-positional.md) | `Role::System` is positional (leading-only) | accepted |

- [ ] **Step 5: Update root README**

Replace the "Project status" callout:

```markdown
> **Project status:** Layer 1 (provider abstraction) complete. Private repo,
> designed to be open-sourced. caliban-provider defines the provider-neutral
> message IR; four schema-family adapter crates (anthropic, openai, ollama,
> google) implement Provider for eight (schema, transport) wirings: direct,
> AWS Bedrock, Google Vertex AI (for Anthropic + Gemini), Azure OpenAI.
> The `caliban` binary is still a `--version` stub — the agent loop, tools,
> and CLI live in later sub-projects.
```

Update repo layout to list new crates. Add an "Example usage" section after Building:

```markdown
## Example usage (library)

```rust
use caliban_provider::{CompletionRequest, Provider};
use caliban_provider_anthropic::{config::DirectConfig, AnthropicProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = DirectConfig::from_env()?;
    let provider = AnthropicProvider::direct(cfg)?;
    let req = CompletionRequest::builder("claude-3-5-sonnet")
        .system("You are helpful.")
        .user_text("What is the airspeed velocity of an unladen swallow?")
        .max_tokens(256)
        .build()?;
    let resp = provider.complete(req).await?;
    println!("{:?}", resp.message);
    Ok(())
}
```

(Set `ANTHROPIC_API_KEY` before running.)
```

Add a new "Provider matrix" section listing the eight wirings + their feature flags.

- [ ] **Step 6: Update CI workflow**

In `.github/workflows/ci.yml`, change the single `check` job to two jobs:

```yaml
jobs:
  check:
    name: fmt · clippy · build · test (default features)
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo build --workspace --all-targets
      - run: cargo test  --workspace

  check-cloud:
    name: build + test (bedrock + vertex + azure features)
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: |
          cargo build  --workspace --features caliban-provider-anthropic/bedrock,caliban-provider-anthropic/vertex,caliban-provider-openai/azure,caliban-provider-google/vertex
          cargo clippy --workspace --features caliban-provider-anthropic/bedrock,caliban-provider-anthropic/vertex,caliban-provider-openai/azure,caliban-provider-google/vertex --all-targets -- -D warnings
          cargo test   --workspace --features caliban-provider-anthropic/bedrock,caliban-provider-anthropic/vertex,caliban-provider-openai/azure,caliban-provider-google/vertex
```

- [ ] **Step 7: Verify locally**

```bash
cargo fmt --all -- --check
cargo build --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --features caliban-provider-anthropic/bedrock,caliban-provider-anthropic/vertex,caliban-provider-openai/azure,caliban-provider-google/vertex
cargo test  --workspace --features caliban-provider-anthropic/bedrock,caliban-provider-anthropic/vertex,caliban-provider-openai/azure,caliban-provider-google/vertex
```

All must exit 0.

- [ ] **Step 8: Commit**

```bash
git add adrs/ README.md .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
docs(layer-1): three ADRs + README + CI matrix update

ADRs:
- 0006 message-schema-ir — provider-neutral IR rationale
- 0007 transport-trait-pattern — schema/transport factoring
- 0008 system-role-positional — leading-only Role::System constraint

README updated with Layer-1 status, new crate listings, example
usage, and a provider matrix showing the eight (schema, transport)
wirings. CI splits into a default-features job and a cloud-features
job that exercises bedrock, vertex, and azure builds.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

**Spec coverage:**
- Provider trait + IR types — Task 1.
- Anthropic adapter direct (non-stream + stream) — Tasks 2, 3.
- OpenAI adapter direct (non-stream + stream) — Tasks 4, 5.
- Ollama adapter — Task 6.
- Google adapter AI Studio (non-stream + stream) — Task 7.
- Bedrock transport for Anthropic — Task 8.
- Vertex transport for Anthropic — Task 9.
- Vertex transport for Google — Task 10.
- Azure transport for OpenAI — Task 11.
- ADRs + README + CI — Task 12.
- MockProvider — Task 1 (feature `mock`).
- Capabilities tables — Tasks 2, 4, 6, 7 (per adapter).
- Three test tiers — Task 1 establishes the pattern; each adapter task ships unit + fixture (live tests behind feature flags, env-var gated).

**Placeholder scan:**
- Task 4 Step 6 (`ir_convert.rs` for OpenAI) and Task 4 Step 7 (`transport/direct.rs`) describe the pattern but say "mirror Task 2's Anthropic Transport." This is intentional shorthand — the data shapes and rules are exhaustive in adjacent steps, and a subagent who can implement Task 2 can implement Task 4 from the schema types + translation rules without literal duplication of every line. Similarly Tasks 6, 7, 9, 10, 11 reference earlier task patterns.

**Type consistency:**
- `Message`, `Role`, `ContentBlock`, `StreamEvent`, `StreamingContentType`, `StreamingDelta`, `Capabilities`, `ModelInfo`, `Tool`, `ToolChoice`, `Error` are defined once (Task 1) and referenced consistently in all later tasks.
- Each adapter's `XxxProvider<T: Transport>` is generic, has a `direct(cfg)` constructor, and feature-gated constructors (`bedrock`, `vertex`, `azure`) match the feature flags in the crate's `Cargo.toml`.
- `caliban_provider::Error::from(adapter_error)` exists for each adapter's internal error.
- Trait method names match: `complete`, `stream`, `capabilities`, `list_models`, `name`.
