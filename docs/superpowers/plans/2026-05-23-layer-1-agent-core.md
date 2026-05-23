# Layer 1 / C (Agent-Core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `caliban-agent-core` — one crate that drives the LLM agent loop on top of `caliban-provider`. Includes `Tool` trait, `ToolRegistry`, stateless `Agent` + stateful `Session`, single-turn / multi-turn / streaming entry points, cancellation, retry, compaction, and hooks.

**Architecture:** `stream_until_done` is the single source of truth (consumes `Provider::stream`, dispatches tools, emits high-level `TurnEvent`s). `run_until_done` and `run_turn` are thin consumers of the stream. Stateless `Agent` holds `Arc<dyn Provider>` + `ToolRegistry` + config + `Box<dyn Compactor>` + `RetryPolicy` + `Arc<dyn Hooks>`. `Session` wraps `Arc<Agent>` + message history for interactive flows.

**Tech Stack:** Rust 1.85.0 (edition 2024), tokio (sync/time/macros/rt), tokio-util (CancellationToken), async-trait, futures (Stream), serde/serde_json, thiserror, tracing.

**Spec:** [`docs/superpowers/specs/2026-05-23-layer-1-agent-core-design.md`](../specs/2026-05-23-layer-1-agent-core-design.md)

---

## File Structure

```
crates/caliban-agent-core/
├── Cargo.toml                       Task 1
├── src/
│   ├── lib.rs                       Task 1 (stub); evolves through Tasks 2-7
│   ├── error.rs                     Task 1
│   ├── tool.rs                      Task 1
│   ├── registry.rs                  Task 1
│   ├── agent.rs                     Task 2
│   ├── hooks.rs                     Task 2
│   ├── retry.rs                     Task 3
│   ├── compact.rs                   Task 4
│   ├── stream.rs                    Task 5
│   ├── turn.rs                      Task 6
│   └── session.rs                   Task 7
└── tests/
    ├── tool_registry.rs             Task 1
    ├── retry_backoff.rs             Task 3
    ├── compactors.rs                Task 4
    ├── integration.rs               Task 8 (most tests)
    └── streaming.rs                 Task 8 (stream event coverage)
adrs/
└── 0009-agent-core-design.md        Task 9
README.md                            Task 9 (modified)
adrs/README.md                       Task 9 (modified — index)
Cargo.toml                           Task 1 (modified — workspace member)
```

---

## Task 1: Crate skeleton + Tool/ToolRegistry/Error/ToolContext

**Files:**
- Modify: `Cargo.toml` (root — add `crates/caliban-agent-core` member)
- Create: `crates/caliban-agent-core/Cargo.toml`
- Create: `crates/caliban-agent-core/src/{lib,error,tool,registry}.rs`
- Create: `crates/caliban-agent-core/tests/tool_registry.rs`

- [ ] **Step 1: Add workspace member**

In root `Cargo.toml`, add `"crates/caliban-agent-core"` to `members` (after the other Layer 1 crates).

If `tokio-util` is not in `[workspace.dependencies]`, add `tokio-util = { version = "0.7", features = ["sync"] }`. Same for `tracing = "0.1"` if missing.

- [ ] **Step 2: `crates/caliban-agent-core/Cargo.toml`**

```toml
[package]
name        = "caliban-agent-core"
version     = "0.0.0"
description = "Agent loop, tool dispatch, cancellation, retry, compaction, and hooks for the caliban agent harness"
edition.workspace      = true
license.workspace      = true
authors.workspace      = true
rust-version.workspace = true
publish     = false

[features]
default = []

[dependencies]
caliban-provider = { path = "../caliban-provider" }
async-trait      = { workspace = true }
serde            = { workspace = true }
serde_json       = { workspace = true }
thiserror        = { workspace = true }
futures          = { workspace = true }
tokio            = { workspace = true }
tokio-util       = { workspace = true }
tracing          = { workspace = true }
rand             = "0.8"

[dev-dependencies]
caliban-provider = { path = "../caliban-provider", features = ["mock"] }
tokio            = { workspace = true, features = ["macros", "rt-multi-thread", "time"] }
proptest         = { workspace = true }

[lints]
workspace = true
```

(`rand` is for retry jitter. Add `rand = "0.8"` to root workspace.dependencies if absent.)

- [ ] **Step 3: `src/error.rs`**

```rust
//! Error type for caliban-agent-core.

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("provider error: {0}")]
    Provider(#[from] caliban_provider::Error),

    #[error("tool '{tool}' execution failed: {source}")]
    ToolExecution {
        tool: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("compaction failed: {0}")]
    Compaction(String),

    #[error("hook failed: {0}")]
    HookFailed(String),

    #[error("max turns reached ({0}); the model did not naturally stop")]
    MaxTurnsReached(u32),

    #[error("operation cancelled")]
    Cancelled,

    #[error("agent misconfigured: {0}")]
    Misconfigured(String),
}

pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 4: `src/tool.rs`**

```rust
//! Tool trait — implementations live in caliban-tools-builtin (D) and downstream.

use async_trait::async_trait;
use caliban_provider::ContentBlock;
use tokio_util::sync::CancellationToken;

/// Context passed to a Tool's `invoke` method.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// The model-assigned tool_use_id this invocation corresponds to.
    pub tool_use_id: String,
    /// Cancellation token; tools must honor this for long-running work.
    pub cancel: CancellationToken,
}

/// Errors a `Tool::invoke` can return.
#[derive(thiserror::Error, Debug)]
pub enum ToolError {
    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("execution failed: {0}")]
    Execution(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("cancelled")]
    Cancelled,
}

impl ToolError {
    /// Construct an `Execution` variant from any error type.
    pub fn execution(e: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Execution(Box::new(e))
    }

    /// Construct an `InvalidInput` variant.
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }
}

/// Tool implementations register with `ToolRegistry`; the agent dispatches
/// `Provider`-emitted tool_use blocks to the matching `Tool::invoke`.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable, unique-within-registry name. Must match the model's
    /// expected tool name in the system prompt or schema.
    fn name(&self) -> &str;

    /// Description sent to the model.
    fn description(&self) -> &str;

    /// JSON Schema for the input. Returned by reference to avoid cloning
    /// per request.
    fn input_schema(&self) -> &serde_json::Value;

    /// Execute the tool. Returns the content blocks to splice into the
    /// `ToolResult` message.
    async fn invoke(
        &self,
        input: serde_json::Value,
        cx: ToolContext,
    ) -> std::result::Result<Vec<ContentBlock>, ToolError>;
}
```

- [ ] **Step 5: `src/registry.rs`**

```rust
//! Tool registry — maps tool name → impl.

use std::collections::HashMap;
use std::sync::Arc;

use crate::tool::Tool;

/// Registry of tools by name.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ToolRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Register a tool. If a tool with the same name already exists,
    /// this replaces it and logs a `tracing::warn!`.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> &mut Self {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) {
            tracing::warn!("ToolRegistry::register replacing existing tool '{name}'");
        }
        self.tools.insert(name, tool);
        self
    }

    /// Remove a tool by name.
    pub fn unregister(&mut self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.remove(name)
    }

    /// Look up a tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// Iterator over registered names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    /// Snapshot the registry as a Vec of `caliban_provider::Tool` for
    /// inclusion in a `CompletionRequest`.
    #[must_use]
    pub fn to_caliban_tools(&self) -> Vec<caliban_provider::Tool> {
        self.tools
            .values()
            .map(|t| caliban_provider::Tool {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema().clone(),
                cache_control: None,
            })
            .collect()
    }
}
```

- [ ] **Step 6: `src/lib.rs`** (initial scaffold; will gain more re-exports through Tasks 2-7)

```rust
//! Agent loop, tool dispatch, cancellation, retry, compaction, and hooks
//! for the caliban agent harness. Drives an LLM conversation on top of
//! `caliban-provider`.

pub mod error;
pub mod registry;
pub mod tool;

pub use error::{Error, Result};
pub use registry::ToolRegistry;
pub use tool::{Tool, ToolContext, ToolError};

// Re-export from caliban-provider so callers can construct messages without
// pulling that crate explicitly.
pub use caliban_provider::{
    CompletionRequest, ContentBlock, Message, Role, StopReason, TextBlock, Usage,
};
```

- [ ] **Step 7: `tests/tool_registry.rs`**

```rust
#![allow(missing_docs)]

use std::sync::Arc;

use async_trait::async_trait;
use caliban_agent_core::{ContentBlock, Tool, ToolContext, ToolError, ToolRegistry};
use serde_json::json;

struct EchoTool {
    schema: serde_json::Value,
}

impl EchoTool {
    fn new() -> Self {
        Self {
            schema: json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
        }
    }
}

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str { "echo" }
    fn description(&self) -> &str { "echo input back" }
    fn input_schema(&self) -> &serde_json::Value { &self.schema }
    async fn invoke(&self, input: serde_json::Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'text'"))?
            .to_string();
        Ok(vec![ContentBlock::Text(caliban_agent_core::TextBlock { text, cache_control: None })])
    }
}

#[test]
fn register_and_lookup() {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(EchoTool::new()));
    assert!(reg.get("echo").is_some());
    assert!(reg.get("nope").is_none());
    assert_eq!(reg.names().collect::<Vec<_>>(), vec!["echo"]);
}

#[test]
fn duplicate_register_replaces() {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(EchoTool::new()));
    reg.register(Arc::new(EchoTool::new()));
    assert_eq!(reg.names().count(), 1);
}

#[test]
fn unregister_removes() {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(EchoTool::new()));
    assert!(reg.unregister("echo").is_some());
    assert!(reg.get("echo").is_none());
}

#[test]
fn to_caliban_tools_snapshot() {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(EchoTool::new()));
    let tools = reg.to_caliban_tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
}

#[tokio::test]
async fn invoke_returns_text_block() {
    let tool = EchoTool::new();
    let cx = ToolContext {
        tool_use_id: "toolu_1".into(),
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    let out = tool.invoke(json!({"text": "hi"}), cx).await.unwrap();
    assert_eq!(out.len(), 1);
}

#[tokio::test]
async fn invoke_invalid_input_errors() {
    let tool = EchoTool::new();
    let cx = ToolContext {
        tool_use_id: "toolu_1".into(),
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    let err = tool.invoke(json!({}), cx).await.unwrap_err();
    assert!(matches!(err, ToolError::InvalidInput(_)));
}
```

- [ ] **Step 8: Build + test**

```bash
cargo build -p caliban-agent-core
cargo test  -p caliban-agent-core
cargo clippy -p caliban-agent-core --all-targets -- -D warnings
cargo fmt --all -- --check
```

All must exit 0.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml crates/caliban-agent-core/
git commit -m "$(cat <<'EOF'
feat(agent-core): crate skeleton + Tool/ToolRegistry/Error

Initial scaffold for caliban-agent-core. Defines the Tool trait
(name/description/input_schema/invoke with ToolContext), ToolError
enum (InvalidInput/Execution/Cancelled), and ToolRegistry (HashMap<
name, Arc<dyn Tool>> with register/unregister/get/names/to_caliban_tools).
The crate's Error enum wraps caliban_provider::Error and adds agent-
specific variants (ToolExecution, Compaction, HookFailed,
MaxTurnsReached, Cancelled, Misconfigured).

Six tests cover registry register/lookup/replace/unregister/snapshot
plus Tool::invoke smoke + invalid-input error.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Agent + AgentBuilder + Hooks

**Files:**
- Create: `crates/caliban-agent-core/src/{agent,hooks}.rs`
- Modify: `crates/caliban-agent-core/src/lib.rs`

- [ ] **Step 1: `src/hooks.rs`**

```rust
//! Hooks trait — pluggable callbacks for pre/post turn + pre/post tool.

use async_trait::async_trait;
use caliban_provider::{ContentBlock, Message};

use crate::error::Result;
use crate::tool::ToolError;
use crate::AgentConfig;

/// Decision returned by `before_tool`.
#[derive(Debug, Clone)]
pub enum HookDecision {
    /// Proceed with the tool invocation as normal.
    Allow,
    /// Skip the tool; synthesize a ToolResult with the given denial message.
    Deny(String),
}

/// Per-turn context passed to turn hooks.
#[derive(Debug)]
pub struct TurnCtx<'a> {
    pub turn_index: u32,
    pub messages: &'a [Message],
    pub config: &'a AgentConfig,
}

/// Per-tool context passed to tool hooks.
#[derive(Debug)]
pub struct ToolCtx<'a> {
    pub turn_index: u32,
    pub tool_use_id: &'a str,
    pub tool_name: &'a str,
    pub input: &'a serde_json::Value,
}

/// Pluggable lifecycle callbacks for the agent loop.
#[async_trait]
pub trait Hooks: Send + Sync {
    async fn before_turn(&self, _ctx: &TurnCtx<'_>) -> Result<()> { Ok(()) }

    async fn after_turn(&self, _ctx: &TurnCtx<'_>, _outcome: &crate::TurnOutcome) -> Result<()> { Ok(()) }

    async fn before_tool(&self, _ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        Ok(HookDecision::Allow)
    }

    async fn after_tool(
        &self,
        _ctx: &ToolCtx<'_>,
        _result: &std::result::Result<Vec<ContentBlock>, ToolError>,
    ) -> Result<()> {
        Ok(())
    }
}

/// Default no-op hooks. Use this when you don't need observability.
#[derive(Debug, Default)]
pub struct NoopHooks;

#[async_trait]
impl Hooks for NoopHooks {}
```

- [ ] **Step 2: `src/agent.rs`**

```rust
//! Agent struct + builder + config.

use std::sync::Arc;

use caliban_provider::{Provider, ThinkingConfig, ToolChoice};

use crate::error::{Error, Result};
use crate::hooks::{Hooks, NoopHooks};
use crate::registry::ToolRegistry;

/// Per-turn settings derived from the request.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stop_sequences: Vec<String>,
    pub thinking: Option<ThinkingConfig>,
    pub user_id: Option<String>,
    pub max_turns: u32,
    pub tool_choice: ToolChoice,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            stop_sequences: Vec::new(),
            thinking: None,
            user_id: None,
            max_turns: 50,
            tool_choice: ToolChoice::default(),
        }
    }
}

/// The agent: a provider + tools + config + compactor + retry + hooks.
pub struct Agent {
    pub(crate) provider: Arc<dyn Provider + Send + Sync>,
    pub(crate) tools: ToolRegistry,
    pub(crate) config: AgentConfig,
    pub(crate) compactor: Arc<dyn crate::compact::Compactor + Send + Sync>,
    pub(crate) retry: crate::retry::RetryPolicy,
    pub(crate) hooks: Arc<dyn Hooks + Send + Sync>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("provider", &self.provider.name())
            .field("tools", &self.tools)
            .field("config", &self.config)
            .field("retry", &self.retry)
            .finish_non_exhaustive()
    }
}

impl Agent {
    #[must_use]
    pub fn builder() -> AgentBuilder { AgentBuilder::default() }

    #[must_use]
    pub fn config(&self) -> &AgentConfig { &self.config }

    #[must_use]
    pub fn tools(&self) -> &ToolRegistry { &self.tools }
}

/// Builder for `Agent`.
#[derive(Default)]
pub struct AgentBuilder {
    provider: Option<Arc<dyn Provider + Send + Sync>>,
    tools: ToolRegistry,
    config: AgentConfig,
    compactor: Option<Arc<dyn crate::compact::Compactor + Send + Sync>>,
    retry: Option<crate::retry::RetryPolicy>,
    hooks: Option<Arc<dyn Hooks + Send + Sync>>,
}

impl AgentBuilder {
    #[must_use]
    pub fn provider(mut self, p: Arc<dyn Provider + Send + Sync>) -> Self {
        self.provider = Some(p);
        self
    }

    #[must_use]
    pub fn tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    #[must_use]
    pub fn config(mut self, cfg: AgentConfig) -> Self {
        self.config = cfg;
        self
    }

    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = model.into();
        self
    }

    #[must_use]
    pub fn max_tokens(mut self, n: u32) -> Self {
        self.config.max_tokens = n;
        self
    }

    #[must_use]
    pub fn max_turns(mut self, n: u32) -> Self {
        self.config.max_turns = n;
        self
    }

    #[must_use]
    pub fn temperature(mut self, t: f32) -> Self {
        self.config.temperature = Some(t);
        self
    }

    #[must_use]
    pub fn compactor(mut self, c: Arc<dyn crate::compact::Compactor + Send + Sync>) -> Self {
        self.compactor = Some(c);
        self
    }

    #[must_use]
    pub fn retry_policy(mut self, p: crate::retry::RetryPolicy) -> Self {
        self.retry = Some(p);
        self
    }

    #[must_use]
    pub fn hooks(mut self, h: Arc<dyn Hooks + Send + Sync>) -> Self {
        self.hooks = Some(h);
        self
    }

    /// Finalize the builder. Returns `Err(Misconfigured(...))` if required
    /// fields are missing.
    ///
    /// # Errors
    /// Returns `Error::Misconfigured` if no provider was set or the model is empty.
    pub fn build(self) -> Result<Agent> {
        let provider = self.provider.ok_or_else(|| Error::Misconfigured("Agent::provider is required".into()))?;
        if self.config.model.is_empty() {
            return Err(Error::Misconfigured("Agent::model is required".into()));
        }
        if self.config.max_tokens == 0 {
            return Err(Error::Misconfigured("Agent::max_tokens must be > 0".into()));
        }
        Ok(Agent {
            provider,
            tools: self.tools,
            config: self.config,
            compactor: self.compactor.unwrap_or_else(|| Arc::new(crate::compact::NoopCompactor)),
            retry: self.retry.unwrap_or_default(),
            hooks: self.hooks.unwrap_or_else(|| Arc::new(NoopHooks)),
        })
    }
}
```

- [ ] **Step 3: Add forward declarations to `lib.rs`**

Add empty modules so `agent.rs` and `hooks.rs` compile (the actual `Compactor`, `RetryPolicy`, `TurnOutcome` types are added in Tasks 3-6 but stubs are needed now for `agent.rs` to compile).

```rust
pub mod error;
pub mod registry;
pub mod tool;
pub mod hooks;
pub mod agent;

// Empty stubs — populated by later tasks
pub mod compact { /* populated in Task 4 */ }
pub mod retry { /* populated in Task 3 */ }

// Forward declared so hooks.rs can reference TurnOutcome.
// This is a TEMPORARY placeholder for Task 2. Task 5 replaces it.
#[derive(Debug)]
pub struct TurnOutcome;

pub use agent::{Agent, AgentBuilder, AgentConfig};
pub use error::{Error, Result};
pub use hooks::{HookDecision, Hooks, NoopHooks, ToolCtx, TurnCtx};
pub use registry::ToolRegistry;
pub use tool::{Tool, ToolContext, ToolError};
pub use caliban_provider::{
    CompletionRequest, ContentBlock, Message, Role, StopReason, TextBlock, Usage,
};
```

To prevent Task 2 from forming a half-baked `TurnOutcome` (which Task 5 will define properly), wrap the placeholder:

Actually — simpler approach: skip `after_turn`'s `&TurnOutcome` parameter for Task 2 and add it in Task 5 when the real type lands. Replace the hooks.rs `after_turn` signature with a placeholder that takes `&dyn std::any::Any` (or even just `()`); Task 5 changes it to `&TurnOutcome`. To minimize churn, the cleanest path is:

**For Task 2 only**, define a minimal `TurnOutcome` placeholder in `lib.rs`:

```rust
#[derive(Debug)]
pub struct TurnOutcome {
    /// Placeholder. Real definition in Task 5.
    pub _placeholder: (),
}
```

Task 5 replaces it with the full struct from the spec.

Yes — that's the simplest path. Use it.

- [ ] **Step 4: Build + test**

```bash
cargo build -p caliban-agent-core
cargo test  -p caliban-agent-core
cargo clippy -p caliban-agent-core --all-targets -- -D warnings
```

All must exit 0. The existing registry tests still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/
git commit -m "feat(agent-core): Agent + AgentBuilder + Hooks trait

Adds the central Agent struct (provider + tools + config + compactor +
retry + hooks), its fluent AgentBuilder with required-field validation
(provider, model, max_tokens), and the Hooks trait with NoopHooks
default + HookDecision::Allow/Deny + TurnCtx/ToolCtx. The turn loop
itself is added in subsequent tasks; this lays out the API surface.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: RetryPolicy + retry executor

**Files:**
- Replace: `crates/caliban-agent-core/src/retry.rs` (replace the empty module stub)
- Create: `crates/caliban-agent-core/tests/retry_backoff.rs`

- [ ] **Step 1: `src/retry.rs`**

```rust
//! Retry policy and executor.

use std::time::Duration;

use caliban_provider::Error as ProviderError;
use tokio_util::sync::CancellationToken;

/// Configurable retry policy for provider calls.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub backoff_multiplier: f32,
    pub max_backoff: Duration,
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(500),
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(30),
            jitter: true,
        }
    }
}

impl RetryPolicy {
    /// Construct a policy that never retries (single attempt).
    #[must_use]
    pub fn no_retry() -> Self {
        Self { max_attempts: 1, ..Self::default() }
    }
}

/// Classify a provider error as retryable or not.
#[must_use]
pub fn is_retryable(e: &ProviderError) -> bool {
    matches!(
        e,
        ProviderError::RateLimit { .. }
            | ProviderError::Network(_)
            | ProviderError::ServerError { status: 502..=599, .. },
    )
}

/// Compute the backoff for attempt `n` (1-indexed).
///
/// `n = 1` is the first retry (after the initial attempt).
#[must_use]
pub fn compute_backoff(policy: &RetryPolicy, attempt: u32) -> Duration {
    let factor = policy.backoff_multiplier.powi(i32::try_from(attempt.saturating_sub(1)).unwrap_or(i32::MAX));
    let nominal_ms = (policy.initial_backoff.as_millis() as f64 * f64::from(factor)) as u64;
    let nominal = Duration::from_millis(nominal_ms).min(policy.max_backoff);
    if policy.jitter {
        // 50-100% of nominal
        let pct = 0.5 + rand::random::<f32>() * 0.5;
        let jittered_ms = (nominal.as_millis() as f64 * f64::from(pct)) as u64;
        Duration::from_millis(jittered_ms)
    } else {
        nominal
    }
}

/// Decide the actual sleep duration for a given error + attempt.
///
/// For RateLimit with `retry_after`, prefer that. Otherwise use exponential
/// backoff.
#[must_use]
pub fn sleep_for(policy: &RetryPolicy, error: &ProviderError, attempt: u32) -> Duration {
    if let ProviderError::RateLimit { retry_after: Some(d) } = error {
        return *d;
    }
    compute_backoff(policy, attempt)
}

/// Run `f` with retry semantics. Sleeps between attempts using the policy.
/// Cancellation aborts a pending sleep early.
///
/// # Errors
/// Returns the last error if all attempts exhausted, or `ProviderError::Cancelled`
/// if the cancel token fired during a sleep.
pub async fn with_retry<F, Fut, T>(
    policy: &RetryPolicy,
    cancel: &CancellationToken,
    mut f: F,
) -> std::result::Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, ProviderError>>,
{
    let mut last_err: Option<ProviderError> = None;
    for attempt in 1..=policy.max_attempts {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if !is_retryable(&e) || attempt == policy.max_attempts {
                    return Err(e);
                }
                let sleep_d = sleep_for(policy, &e, attempt);
                last_err = Some(e);
                tokio::select! {
                    () = tokio::time::sleep(sleep_d) => {}
                    () = cancel.cancelled() => {
                        return Err(ProviderError::Cancelled);
                    }
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| ProviderError::Adapter(
        Box::<dyn std::error::Error + Send + Sync>::from("retry exhausted")
    )))
}
```

- [ ] **Step 2: Replace the lib.rs stub for `pub mod retry`**

Replace `pub mod retry { /* populated in Task 3 */ }` with `pub mod retry;`. Re-export `RetryPolicy` from lib.rs.

```rust
pub use retry::RetryPolicy;
```

- [ ] **Step 3: `tests/retry_backoff.rs`**

```rust
#![allow(missing_docs)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use caliban_agent_core::retry::{compute_backoff, is_retryable, with_retry, RetryPolicy};
use caliban_provider::Error as ProviderError;
use tokio_util::sync::CancellationToken;

#[test]
fn default_policy_has_3_attempts() {
    let p = RetryPolicy::default();
    assert_eq!(p.max_attempts, 3);
}

#[test]
fn no_retry_has_1_attempt() {
    let p = RetryPolicy::no_retry();
    assert_eq!(p.max_attempts, 1);
}

#[test]
fn retryable_classification() {
    assert!(is_retryable(&ProviderError::Network(Box::<dyn std::error::Error + Send + Sync>::from("x"))));
    assert!(is_retryable(&ProviderError::RateLimit { retry_after: None }));
    assert!(is_retryable(&ProviderError::ServerError { status: 503, body: "".into() }));
    assert!(!is_retryable(&ProviderError::ServerError { status: 500, body: "".into() }));
    assert!(!is_retryable(&ProviderError::Auth("nope".into())));
    assert!(!is_retryable(&ProviderError::InvalidRequest("nope".into())));
}

#[test]
fn backoff_math_no_jitter() {
    let p = RetryPolicy { initial_backoff: Duration::from_millis(100), backoff_multiplier: 2.0, max_backoff: Duration::from_secs(60), jitter: false, ..Default::default() };
    assert_eq!(compute_backoff(&p, 1), Duration::from_millis(100));
    assert_eq!(compute_backoff(&p, 2), Duration::from_millis(200));
    assert_eq!(compute_backoff(&p, 3), Duration::from_millis(400));
}

#[test]
fn backoff_caps_at_max() {
    let p = RetryPolicy { initial_backoff: Duration::from_millis(1000), backoff_multiplier: 10.0, max_backoff: Duration::from_secs(5), jitter: false, ..Default::default() };
    assert_eq!(compute_backoff(&p, 10), Duration::from_secs(5));
}

#[tokio::test(start_paused = true)]
async fn retries_until_success() {
    let counter = Arc::new(AtomicU32::new(0));
    let cancel = CancellationToken::new();
    let policy = RetryPolicy { jitter: false, initial_backoff: Duration::from_millis(10), ..Default::default() };
    let counter_clone = counter.clone();
    let result: Result<u32, ProviderError> = with_retry(&policy, &cancel, move || {
        let c = counter_clone.clone();
        async move {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(ProviderError::Network(Box::<dyn std::error::Error + Send + Sync>::from("nope")))
            } else {
                Ok(42)
            }
        }
    }).await;
    assert_eq!(result.unwrap(), 42);
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn does_not_retry_on_auth_error() {
    let counter = Arc::new(AtomicU32::new(0));
    let cancel = CancellationToken::new();
    let policy = RetryPolicy::default();
    let counter_clone = counter.clone();
    let _result: Result<u32, ProviderError> = with_retry(&policy, &cancel, move || {
        let c = counter_clone.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::Auth("bad key".into()))
        }
    }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_backoff_returns_cancelled() {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let policy = RetryPolicy { jitter: false, initial_backoff: Duration::from_secs(10), ..Default::default() };

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_clone.cancel();
    });

    let result: Result<u32, ProviderError> = with_retry(&policy, &cancel, || async {
        Err::<u32, _>(ProviderError::Network(Box::<dyn std::error::Error + Send + Sync>::from("nope")))
    }).await;
    assert!(matches!(result, Err(ProviderError::Cancelled)));
}
```

- [ ] **Step 4: Build + test + commit**

```bash
cargo test  -p caliban-agent-core
cargo clippy -p caliban-agent-core --all-targets -- -D warnings
git add crates/caliban-agent-core/
git commit -m "feat(agent-core): RetryPolicy + retry executor

RetryPolicy with max_attempts, exponential backoff (multiplier + cap),
optional jitter. is_retryable classifies provider errors: Network,
RateLimit, ServerError 502-599 are retryable; Auth, InvalidRequest,
ContextTooLong, ContentFilter, Cancelled, Adapter, ModelUnavailable,
ServerError 500 are not. with_retry executor honors RateLimit::retry_after
when present, otherwise computes exponential backoff. tokio::select!
allows cancellation to abort a pending sleep.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Compactor trait + Noop/DropOldest/Summarizing

**Files:**
- Replace: `crates/caliban-agent-core/src/compact.rs`
- Modify: `crates/caliban-agent-core/src/lib.rs`
- Create: `crates/caliban-agent-core/tests/compactors.rs`

- [ ] **Step 1: `src/compact.rs`**

```rust
//! Compactor trait — strategies for truncating long histories.

use std::sync::Arc;

use async_trait::async_trait;
use caliban_provider::{Capabilities, Message, Provider, Role};

use crate::error::{Error, Result};

/// Compactor — strategy for keeping the message history under the model's
/// input window.
#[async_trait]
pub trait Compactor: Send + Sync {
    /// Decide whether to compact. Returns the new messages if compaction
    /// was applied; None if no-op.
    async fn compact(&self, messages: &[Message], capabilities: &Capabilities) -> Result<Option<Vec<Message>>>;
}

/// Estimate token count using a chars/4 heuristic.
#[must_use]
pub fn estimate_tokens(messages: &[Message]) -> u32 {
    let mut chars: usize = 0;
    for m in messages {
        for cb in &m.content {
            if let caliban_provider::ContentBlock::Text(t) = cb {
                chars += t.text.len();
            }
            if let caliban_provider::ContentBlock::ToolResult(tr) = cb {
                for inner in &tr.content {
                    if let caliban_provider::ContentBlock::Text(t) = inner {
                        chars += t.text.len();
                    }
                }
            }
            if let caliban_provider::ContentBlock::Thinking(t) = cb {
                chars += t.thinking.len();
            }
            if let caliban_provider::ContentBlock::ToolUse(tu) = cb {
                chars += tu.input.to_string().len();
                chars += tu.name.len();
            }
        }
    }
    u32::try_from(chars / 4).unwrap_or(u32::MAX)
}

/// Noop — never compacts.
#[derive(Debug, Default)]
pub struct NoopCompactor;

#[async_trait]
impl Compactor for NoopCompactor {
    async fn compact(&self, _messages: &[Message], _capabilities: &Capabilities) -> Result<Option<Vec<Message>>> {
        Ok(None)
    }
}

/// Drops messages from the front (preserving leading System messages) until
/// estimated tokens drop below `target_fraction * max_input_tokens`. Always
/// keeps the last `keep_recent_turns` (User+Assistant pairs).
#[derive(Debug)]
pub struct DropOldestCompactor {
    pub target_fraction: f32,
    pub keep_recent_turns: u32,
}

impl Default for DropOldestCompactor {
    fn default() -> Self {
        Self { target_fraction: 0.7, keep_recent_turns: 4 }
    }
}

#[async_trait]
impl Compactor for DropOldestCompactor {
    async fn compact(&self, messages: &[Message], capabilities: &Capabilities) -> Result<Option<Vec<Message>>> {
        let target = (capabilities.max_input_tokens as f32 * self.target_fraction) as u32;
        if estimate_tokens(messages) <= target {
            return Ok(None);
        }
        // Find leading System messages — preserved verbatim.
        let leading_system_count = messages.iter().take_while(|m| m.role == Role::System).count();
        let leading_systems = messages[..leading_system_count].to_vec();
        let body = &messages[leading_system_count..];

        // Keep the last keep_recent_turns × 2 messages of body (pairs of user+assistant).
        let keep = (self.keep_recent_turns as usize) * 2;
        let body_kept = if body.len() <= keep {
            body.to_vec()
        } else {
            body[body.len() - keep..].to_vec()
        };

        let mut new_messages = leading_systems;
        new_messages.extend(body_kept);
        if estimate_tokens(&new_messages) > capabilities.max_input_tokens {
            return Err(Error::Compaction(
                "DropOldestCompactor: kept tail still exceeds max_input_tokens".into(),
            ));
        }
        Ok(Some(new_messages))
    }
}

/// Summarizes older turns into a single System message using the given provider.
#[derive(Clone)]
pub struct SummarizingCompactor {
    pub provider: Arc<dyn Provider + Send + Sync>,
    pub summarizer_model: String,
    pub target_fraction: f32,
    pub keep_recent_turns: u32,
}

impl std::fmt::Debug for SummarizingCompactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SummarizingCompactor")
            .field("summarizer_model", &self.summarizer_model)
            .field("target_fraction", &self.target_fraction)
            .field("keep_recent_turns", &self.keep_recent_turns)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Compactor for SummarizingCompactor {
    async fn compact(&self, messages: &[Message], capabilities: &Capabilities) -> Result<Option<Vec<Message>>> {
        let target = (capabilities.max_input_tokens as f32 * self.target_fraction) as u32;
        if estimate_tokens(messages) <= target {
            return Ok(None);
        }
        let leading_system_count = messages.iter().take_while(|m| m.role == Role::System).count();
        let leading_systems = messages[..leading_system_count].to_vec();
        let body = &messages[leading_system_count..];
        let keep = (self.keep_recent_turns as usize) * 2;
        let (old, recent) = if body.len() <= keep {
            (&body[..0], body)
        } else {
            body.split_at(body.len() - keep)
        };

        if old.is_empty() {
            // Nothing to summarize.
            return Ok(None);
        }

        // Build a summary request.
        let summary_prompt = "Summarize the following conversation concisely, preserving any tool calls, user goals, and key decisions. Output only the summary text.";

        let mut summary_messages = vec![Message::system_text(summary_prompt)];
        // Concatenate old messages into one user message.
        let mut combined = String::new();
        for m in old {
            combined.push_str(&format!("[{:?}]\n", m.role));
            for cb in &m.content {
                if let caliban_provider::ContentBlock::Text(t) = cb {
                    combined.push_str(&t.text);
                    combined.push_str("\n\n");
                }
            }
        }
        summary_messages.push(Message::user_text(combined));

        let req = caliban_provider::CompletionRequest {
            model: self.summarizer_model.clone(),
            messages: summary_messages,
            tools: vec![],
            tool_choice: caliban_provider::ToolChoice::None,
            max_tokens: 1024,
            temperature: Some(0.3),
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: None,
            metadata: caliban_provider::RequestMetadata { user_id: None },
        };

        let resp = self.provider.complete(req).await
            .map_err(|e| Error::Compaction(format!("summarizer call failed: {e}")))?;

        let summary_text = resp.message.content.iter().filter_map(|cb| match cb {
            caliban_provider::ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        }).collect::<Vec<_>>().join("\n");

        let mut new_messages = leading_systems;
        new_messages.push(Message::system_text(format!("Summary of earlier conversation:\n{summary_text}")));
        new_messages.extend(recent.iter().cloned());

        if estimate_tokens(&new_messages) > capabilities.max_input_tokens {
            return Err(Error::Compaction(
                "SummarizingCompactor: result still exceeds max_input_tokens".into(),
            ));
        }
        Ok(Some(new_messages))
    }
}
```

- [ ] **Step 2: Replace stub in `src/lib.rs`**

Replace `pub mod compact { /* populated in Task 4 */ }` with `pub mod compact;`. Add to re-exports:

```rust
pub use compact::{Compactor, DropOldestCompactor, NoopCompactor, SummarizingCompactor};
```

- [ ] **Step 3: `tests/compactors.rs`**

Write tests covering:
- `NoopCompactor` always returns `None`.
- `estimate_tokens` returns sensible values for known inputs.
- `DropOldestCompactor` preserves system + last N when over threshold.
- `DropOldestCompactor` is a no-op under threshold.
- (`SummarizingCompactor` is covered in Task 8 integration tests with `MockProvider`.)

```rust
#![allow(missing_docs)]

use caliban_agent_core::compact::{estimate_tokens, Compactor, DropOldestCompactor, NoopCompactor};
use caliban_provider::{Capabilities, Message, PromptCachingCapability, SystemPromptCapability, ToolUseCapability};

fn fake_caps(max_input: u32) -> Capabilities {
    Capabilities {
        max_input_tokens: max_input,
        max_output_tokens: 1024,
        vision: false,
        tool_use: ToolUseCapability::Basic,
        thinking: false,
        prompt_caching: PromptCachingCapability::None,
        json_mode: false,
        streaming: true,
        stop_sequences: true,
        top_k: false,
        system_prompt: SystemPromptCapability::SystemRole,
        refusal_field: false,
    }
}

#[tokio::test]
async fn noop_always_returns_none() {
    let c = NoopCompactor;
    let result = c.compact(&[Message::user_text("hi")], &fake_caps(100_000)).await.unwrap();
    assert!(result.is_none());
}

#[test]
fn estimate_tokens_smoke() {
    let m = Message::user_text("x".repeat(4000));
    assert!(estimate_tokens(&[m]) >= 999);
}

#[tokio::test]
async fn drop_oldest_is_noop_below_threshold() {
    let c = DropOldestCompactor::default();
    let messages = vec![Message::user_text("hi"), Message::assistant_text("hi back")];
    let result = c.compact(&messages, &fake_caps(100_000)).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn drop_oldest_truncates_above_threshold() {
    let c = DropOldestCompactor { target_fraction: 0.5, keep_recent_turns: 1 };
    let mut messages = vec![Message::system_text("rules")];
    for i in 0..20 {
        messages.push(Message::user_text(format!("q{i}: {}", "x".repeat(200))));
        messages.push(Message::assistant_text(format!("a{i}: {}", "x".repeat(200))));
    }
    let result = c.compact(&messages, &fake_caps(2000)).await.unwrap().unwrap();
    // Should preserve leading System + at most 2 most-recent messages (1 turn = 2 messages).
    assert!(result.len() <= 3);
    assert_eq!(result[0].role, caliban_provider::Role::System);
}
```

- [ ] **Step 4: Build + test + commit**

```bash
cargo test  -p caliban-agent-core
cargo clippy -p caliban-agent-core --all-targets -- -D warnings
git add crates/caliban-agent-core/
git commit -m "feat(agent-core): Compactor trait + three built-in implementations

NoopCompactor (never compacts), DropOldestCompactor (preserves leading
System + last N turns; chars/4 token heuristic), and SummarizingCompactor
(uses the provided Provider to summarize older turns into a system
message). Compaction triggers when estimated tokens exceed
target_fraction * max_input_tokens.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: stream_until_done + TurnEvent

**Files:**
- Create: `crates/caliban-agent-core/src/stream.rs`
- Modify: `crates/caliban-agent-core/src/lib.rs` (remove placeholder TurnOutcome; add stream re-exports; replace `TurnOutcome` import in hooks.rs)
- Modify: `crates/caliban-agent-core/src/hooks.rs` (change after_turn signature to use real TurnOutcome)

- [ ] **Step 1: Write `src/stream.rs`**

The core of the agent loop. The implementer should structure this as:

1. **TurnEvent enum** matching the spec — `TurnStart`, `AssistantTextDelta`, `AssistantThinkingDelta`, `ToolCallStart`, `ToolCallInputDelta`, `ToolCallEnd`, `TurnEnd`, `RunEnd`.

2. **TurnOutcome + RunOutcome + StopCondition** structs from the spec.

3. **TurnEventStream type alias** — `Pin<Box<dyn Stream<Item = Result<TurnEvent>> + Send + 'static>>`.

4. **`stream_until_done` impl on Agent** — uses `async_stream::try_stream!` to build the event stream. Internally:
   - For each turn (up to max_turns):
     - Call `Hooks::before_turn`.
     - Call `compactor.compact` → maybe replace messages.
     - Call `with_retry(&policy, &cancel, || provider.stream(req))`.
     - Pump the provider's StreamEvents, re-emitting as TurnEvent variants.
     - Collect the assistant Message + final stop_reason + usage.
     - If stop_reason is ToolUse: for each ToolUseBlock, look up the Tool, call `Hooks::before_tool`, then dispatch (or deny), then `Hooks::after_tool`. Build a User message with ToolResult blocks, append to messages.
     - Call `Hooks::after_turn`.
     - Emit TurnEnd.
     - If stop_reason != ToolUse: break with stop_condition = EndOfTurn.
   - After loop: emit RunEnd with final messages + total usage + stop_condition.

5. **The async_stream macro** does the yielding. Cancellation checks happen at each loop iteration boundary.

Refer to the spec section "Implementation notes / Single source of truth: stream_until_done" for the high-level structure. Take care with:
- The `Provider::stream` returns a `MessageStream`; consume each StreamEvent and convert it to the right TurnEvent.
- Tool invocations are bracketed: emit `ToolCallStart` before dispatch, `ToolCallEnd` after (with the result content).
- HookDecision::Deny short-circuits the actual dispatch but still emits ToolCallStart and ToolCallEnd (with the denial message as the result).
- Errors during tool dispatch get caught and turned into is_error ToolResult content + emitted as ToolCallEnd with is_error=true.

This is the biggest piece of code in the plan. The implementer should:
- Structure the code into helper functions where it helps clarity (e.g., `dispatch_tool`, `pump_provider_stream`).
- Use `#[allow(clippy::too_many_lines)]` if needed.
- Add `#[tracing::instrument]` annotations on the entry points + key helpers.

- [ ] **Step 2: Replace `TurnOutcome` placeholder in `lib.rs`**

Remove the placeholder TurnOutcome struct from lib.rs (Task 2 added it). The real one comes from `stream.rs`. Update re-exports.

- [ ] **Step 3: Update `hooks.rs` to use real TurnOutcome**

`after_turn`'s signature: `async fn after_turn(&self, _ctx: &TurnCtx<'_>, _outcome: &crate::stream::TurnOutcome) -> Result<()>`. The hooks.rs uses `crate::TurnOutcome` from re-export which now points to the real type.

- [ ] **Step 4: Build verifies compile**

```bash
cargo build -p caliban-agent-core
```

There are no integration tests yet for this module (those land in Task 8). For now, just verify it compiles + clippy clean.

```bash
cargo clippy -p caliban-agent-core --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/
git commit -m "feat(agent-core): stream_until_done + TurnEvent

Implements the core agent loop driver. stream_until_done consumes
Provider::stream events, re-emits high-level TurnEvents
(TurnStart/AssistantTextDelta/AssistantThinkingDelta/
ToolCallStart/ToolCallInputDelta/ToolCallEnd/TurnEnd/RunEnd),
dispatches tools sequentially, applies retry + compaction +
hooks at the appropriate boundaries, and honors cancellation
at each yield point.

TurnOutcome, RunOutcome, StopCondition types defined here are
the public surface for callers.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: run_turn + run_until_done

**Files:**
- Create: `crates/caliban-agent-core/src/turn.rs`
- Modify: `crates/caliban-agent-core/src/lib.rs`
- Modify: `crates/caliban-agent-core/src/agent.rs` — add the entry methods

- [ ] **Step 1: `src/turn.rs`**

Implement `run_turn` and `run_until_done` as thin consumers of `stream_until_done`:

```rust
//! Non-streaming agent entry points.

use caliban_provider::Message;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::agent::Agent;
use crate::error::Result;
use crate::stream::{RunOutcome, StopCondition, TurnEvent, TurnOutcome};

impl Agent {
    /// Run a single turn (one provider call + any tool dispatches it triggered).
    ///
    /// # Errors
    /// Returns `Error::Cancelled`, `Error::Provider`, `Error::ToolExecution`, etc.
    pub async fn run_turn(&self, messages: Vec<Message>, cancel: CancellationToken) -> Result<TurnOutcome> {
        let mut stream = self.stream_until_done(messages, cancel).await?;
        let mut turn_outcome: Option<TurnOutcome> = None;
        let mut current_turn_index: Option<u32> = None;
        while let Some(event) = stream.next().await {
            let event = event?;
            match event {
                TurnEvent::TurnStart { turn_index, .. } => {
                    current_turn_index = Some(turn_index);
                }
                TurnEvent::TurnEnd { turn_index, stop_reason, usage } => {
                    // Build a TurnOutcome from the events.
                    // Note: the assistant_message and tool_results require accumulated state.
                    // For now, run_turn relies on the stream having complete TurnOutcome info
                    // in RunEnd; we return only the *first* turn's data.
                    let _ = turn_index;
                    turn_outcome = Some(TurnOutcome::synthesize(stop_reason, usage));
                    break;
                }
                _ => {}
            }
        }
        let _ = current_turn_index;
        turn_outcome.ok_or_else(|| crate::Error::Misconfigured("stream ended without TurnEnd".into()))
    }

    /// Run the agent loop to completion.
    pub async fn run_until_done(&self, messages: Vec<Message>, cancel: CancellationToken) -> Result<RunOutcome> {
        let mut stream = self.stream_until_done(messages, cancel).await?;
        let mut run_outcome: Option<RunOutcome> = None;
        while let Some(event) = stream.next().await {
            if let TurnEvent::RunEnd { final_messages, total_usage, stopped_for } = event? {
                run_outcome = Some(RunOutcome {
                    final_messages,
                    turn_count: 0,   // populated below
                    total_usage,
                    stopped_for,
                });
                break;
            }
        }
        run_outcome.ok_or_else(|| crate::Error::Misconfigured("stream ended without RunEnd".into()))
    }
}
```

(Implementer: tighten this — `run_turn` should return the actual `TurnOutcome` populated from accumulating events. The simplest path is to add an internal helper method `stream_one_turn` that runs exactly one turn and returns the populated TurnOutcome. Alternatively, the TurnEnd event itself carries `assistant_message` and `tool_results` as fields — that's the cleanest. Decide based on implementation feel.)

Recommendation: extend `TurnEvent::TurnEnd` to carry `assistant_message: Message` and `tool_results: Vec<Message>` so `run_turn` can directly construct a `TurnOutcome` from a single event.

- [ ] **Step 2: Modify `lib.rs` to include `pub mod turn;`**

```rust
pub mod turn;
```

- [ ] **Step 3: Build + commit**

```bash
cargo build -p caliban-agent-core
cargo test -p caliban-agent-core
cargo clippy -p caliban-agent-core --all-targets -- -D warnings
git add crates/caliban-agent-core/
git commit -m "feat(agent-core): run_turn + run_until_done non-streaming wrappers

Thin consumers of stream_until_done. run_turn returns a TurnOutcome
for the first turn observed; run_until_done returns RunOutcome with
the accumulated final_messages and total_usage.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Session wrapper

**Files:**
- Create: `crates/caliban-agent-core/src/session.rs`
- Modify: `crates/caliban-agent-core/src/lib.rs`

- [ ] **Step 1: `src/session.rs`**

```rust
//! Session — stateful wrapper around an Arc<Agent>.

use std::sync::Arc;

use caliban_provider::Message;
use tokio_util::sync::CancellationToken;

use crate::agent::Agent;
use crate::error::Result;
use crate::stream::TurnEventStream;

/// Stateful conversation session sharing an Arc<Agent>.
pub struct Session {
    agent: Arc<Agent>,
    messages: Vec<Message>,
    cancel: CancellationToken,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("messages", &self.messages.len())
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Create a new session backed by `agent`.
    #[must_use]
    pub fn new(agent: Arc<Agent>) -> Self {
        Self { agent, messages: Vec::new(), cancel: CancellationToken::new() }
    }

    /// Add a leading system message.
    pub fn system(&mut self, text: impl Into<String>) -> &mut Self {
        // Maintains the agent-core invariant: System messages must precede User/Assistant.
        let insertion_index = self.messages
            .iter()
            .position(|m| m.role != caliban_provider::Role::System)
            .unwrap_or(self.messages.len());
        self.messages.insert(insertion_index, Message::system_text(text));
        self
    }

    /// Append a user message.
    pub fn user_text(&mut self, text: impl Into<String>) -> &mut Self {
        self.messages.push(Message::user_text(text));
        self
    }

    /// Append an arbitrary message.
    pub fn user_message(&mut self, msg: Message) -> &mut Self {
        self.messages.push(msg);
        self
    }

    /// Append several messages at once.
    pub fn extend_messages(&mut self, msgs: impl IntoIterator<Item = Message>) -> &mut Self {
        self.messages.extend(msgs);
        self
    }

    /// Run the agent until done; append generated messages to history; return slice of new messages.
    ///
    /// # Errors
    /// Propagates errors from the underlying `Agent::run_until_done`.
    pub async fn run(&mut self) -> Result<&[Message]> {
        let original_len = self.messages.len();
        let messages = self.messages.clone();
        let outcome = self.agent.run_until_done(messages, self.cancel.clone()).await?;
        self.messages = outcome.final_messages;
        Ok(&self.messages[original_len..])
    }

    /// Run the agent with the streaming surface. Caller must drain the stream to update history themselves; this method does NOT mutate `self.messages` automatically because the stream yields events incrementally.
    ///
    /// # Errors
    /// Propagates from `Agent::stream_until_done`.
    pub async fn stream(&self) -> Result<TurnEventStream> {
        self.agent.stream_until_done(self.messages.clone(), self.cancel.clone()).await
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] { &self.messages }

    /// Clear all history.
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Signal cancellation for any in-flight or future `run`/`stream` call.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}
```

- [ ] **Step 2: Update `lib.rs`**

```rust
pub mod session;
pub use session::Session;
```

- [ ] **Step 3: Build + commit**

```bash
cargo build -p caliban-agent-core
cargo clippy -p caliban-agent-core --all-targets -- -D warnings
git add crates/caliban-agent-core/
git commit -m "feat(agent-core): Session wrapper

Stateful conversation session sharing an Arc<Agent>. Multiple sessions
can share one agent. fluent builders (system, user_text, user_message,
extend_messages), run/stream entry points, cancel() for cooperative
shutdown.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Integration tests

**Files:**
- Create: `crates/caliban-agent-core/tests/integration.rs`
- Create: `crates/caliban-agent-core/tests/streaming.rs`

Implement the 11 integration scenarios listed in the spec's Testing Strategy. Use `caliban_provider::MockProvider` (behind feature `mock`) to script provider responses.

Pattern for each test:

```rust
#![allow(missing_docs)]

use std::sync::Arc;

use async_trait::async_trait;
use caliban_agent_core::{Agent, ToolRegistry, /* etc */};
use caliban_provider::{ContentBlock, MessageProvider, MockProvider, /* etc */};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn single_turn_no_tools() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_complete(Ok(/* scripted response */));
    // ... (or enqueue_stream for streaming tests)

    let agent = Agent::builder()
        .provider(mock.clone() as Arc<dyn caliban_provider::Provider + Send + Sync>)
        .model("mock-model")
        .max_tokens(64)
        .build()
        .unwrap();

    let initial = vec![Message::user_text("hi")];
    let outcome = agent.run_until_done(initial, CancellationToken::new()).await.unwrap();
    // assertions on outcome.final_messages
}
```

**Each of the 11 scenarios needs a test function**:
1. `single_turn_no_tools` — assistant text only, EndTurn stop.
2. `single_turn_with_tool_call` — one tool_use → result → end.
3. `tool_call_with_error` — tool returns Err, model gets is_error=true.
4. `multi_turn_tool_chain` — two tool uses in sequence.
5. `cancellation_mid_turn` — caller cancels during tool dispatch; verify Cancelled.
6. `max_turns_reached` — keep emitting tool_use; verify MaxTurnsReached(50).
7. `retry_on_rate_limit` — script two RateLimit then success; verify the call succeeded after retries.
8. `retry_not_attempted_on_auth` — script Auth(...) error; verify single attempt.
9. `hook_denies_tool` — custom Hooks impl returns Deny; verify result contains "denied".
10. `compaction_triggered` — use DropOldestCompactor with low threshold; verify final history is trimmed.
11. `stream_emits_all_events` (in `streaming.rs`) — full TurnEvent coverage.

Each test should have its own scripted MockProvider setup. Helper functions for common patterns (script-a-response-with-tool-use, script-a-rate-limit) are encouraged.

Aim for ~15+ tests total.

- [ ] **Step N: Build + test + commit**

```bash
cargo test  -p caliban-agent-core --features caliban-provider/mock
cargo clippy -p caliban-agent-core --all-targets --features caliban-provider/mock -- -D warnings
git add crates/caliban-agent-core/
git commit -m "test(agent-core): 15+ integration tests via MockProvider

Covers the 11 scenarios from the spec: single_turn_no_tools,
single_turn_with_tool_call, tool_call_with_error, multi_turn_tool_chain,
cancellation_mid_turn, max_turns_reached, retry_on_rate_limit,
retry_not_attempted_on_auth, hook_denies_tool, compaction_triggered,
stream_emits_all_events.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: ADR 0009 + README update

**Files:**
- Create: `adrs/0009-agent-core-design.md`
- Modify: `adrs/README.md` (index)
- Modify: `README.md`

- [ ] **Step 1: ADR 0009**

```markdown
# ADR 0009 · Agent-core design (stream-as-primitive, sequential tools, opt-in compaction)

- **Status:** accepted
- **Date:** 2026-05-23

## Context

Layer 1 / C adds the agent loop. Three design dimensions had real
trade-offs: where the streaming surface lives, whether tool calls in
one response are dispatched concurrently or sequentially, and what
the default compaction strategy is.

## Decision

- **`stream_until_done` is the single source of truth.** Non-streaming
  `run_turn` and `run_until_done` are thin consumers of the stream.
  This means the streaming code path is always exercised; bugs surface
  through unit + integration tests of either surface.
- **Tool calls are dispatched sequentially within a single turn.**
  Anthropic and Gemini can emit multiple `tool_use` blocks in one
  response; we run them in the order received. Parallelism is a
  follow-on (Hooks-pluggable dispatch strategy).
- **Default compactor is `NoopCompactor`.** Compaction strategies
  (`DropOldest`, `Summarizing`) are explicit opt-ins. The library
  doesn't silently mutate the user's message history; callers decide.
- **Retries only on the provider call.** Tool failures don't retry —
  tools manage their own retry semantics. Retryable provider errors:
  `RateLimit`, `Network`, `ServerError 502-599`. NOT retryable:
  `Auth`, `InvalidRequest`, `ContextTooLong`, `ContentFilter`,
  `Cancelled`, `Adapter`, `ModelUnavailable`, `ServerError 500`.

## Consequences

- **Positive:** Single source of truth → simpler correctness story.
  Sequential tool dispatch → predictable behavior, easier debugging.
  Opt-in compaction → no surprise history mutation. Retry policy
  classifier is conservative and stable.
- **Negative:** Sequential dispatch is slower than parallel for
  independent tools. Token-counting heuristic (chars/4) is approximate.
- **Revisit if:** Real workloads show sequential dispatch as a
  bottleneck (add parallel strategy); a non-English language is
  consistently mis-estimated (integrate a tokenizer crate).
```

- [ ] **Step 2: Update `adrs/README.md`**

Append a row to the index table:

```
| [0009](0009-agent-core-design.md) | Agent-core design (stream-as-primitive, sequential tools, opt-in compaction) | accepted |
```

- [ ] **Step 3: Update root `README.md`**

Update Project status:

```markdown
> **Project status:** Layer 1 (provider abstraction + agent-core) complete.
> Private repo, designed to be open-sourced. caliban-agent-core drives an
> LLM agent loop on top of caliban-provider, with Tool dispatch, cancellation,
> retry, compaction, hooks, and a TurnEvent stream. The `caliban` binary is
> still a stub — the built-in tools (Read/Write/Edit/Bash/Grep/Glob) and CLI
> are coming in subsequent sub-projects.
```

Add `caliban-agent-core/` to the repo layout. Add example:

```markdown
## Example usage (library, with caliban-agent-core)

\```rust
use std::sync::Arc;

use caliban_agent_core::{Agent, ToolRegistry, Session};
use caliban_provider::Provider;
use caliban_provider_anthropic::{config::DirectConfig, AnthropicProvider};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider: Arc<dyn Provider + Send + Sync> = Arc::new(
        AnthropicProvider::direct(DirectConfig::from_env()?)?,
    );

    let agent = Arc::new(Agent::builder()
        .provider(provider)
        .tools(ToolRegistry::new())  // populate with caliban-tools-builtin (D) once it exists
        .model("claude-3-5-sonnet")
        .max_tokens(1024)
        .build()?);

    let mut session = Session::new(agent);
    session.system("You are helpful.").user_text("Hello!");
    let new_msgs = session.run().await?;
    for m in new_msgs { println!("{m:?}"); }
    Ok(())
}
\```
```

- [ ] **Step 4: Build + verify + commit**

```bash
cargo build  --workspace
cargo test   --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

git add adrs/ README.md
git commit -m "docs(layer-1): ADR 0009 + README update for caliban-agent-core

Adds ADR 0009 capturing the three big agent-core design decisions:
stream_until_done as single source of truth, sequential tool dispatch,
opt-in compaction. README updated with Layer-1-complete status,
new crate listing, and a runnable Session-based example.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**
- Tool trait + ToolError + ToolContext + ToolRegistry → Task 1.
- Agent + Builder + AgentConfig + Error + Hooks → Task 2.
- RetryPolicy + retry executor → Task 3.
- Compactor + three implementations + estimate_tokens → Task 4.
- TurnEvent + stream_until_done → Task 5.
- run_turn + run_until_done → Task 6.
- Session → Task 7.
- 11 integration scenarios + streaming event coverage → Task 8.
- ADR 0009 + README → Task 9.

**Placeholder scan:** Task 5 leaves `stream_until_done` implementation as prose-described rather than fully spelled out — this is intentional because (a) the full code is several hundred lines, (b) the implementer follows the spec's "Implementation notes / Single source of truth" section which has the exact algorithm, (c) writing 500 lines of Rust into a plan doc has diminishing returns. The data flow and key helper structure are described concretely. Implementer is free to factor the helpers as feel-best.

**Type consistency:** All cross-task types match: `Tool`/`ToolContext`/`ToolError` defined in Task 1 used by Task 2 (Hooks::after_tool param), Task 5 (stream emits ToolCallStart/End), Task 8 (test mocks). `RetryPolicy` Task 3 → Task 5 (used in stream_until_done). `Compactor` Task 4 → Task 5. `TurnOutcome`/`RunOutcome` Task 5 → Tasks 6, 7, 8. `Hooks` Task 2 → Task 5 (called at each lifecycle point).
