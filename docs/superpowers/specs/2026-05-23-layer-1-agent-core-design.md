# Layer 1 / C · Agent-Core · Design

- **Date:** 2026-05-23
- **Status:** Draft (pending implementation plan)
- **Sub-project of:** caliban Rust agent harness
- **Depends on:** Layer 1 / B (`caliban-provider` + adapters)
- **Next sub-project:** Layer 1 / D (`caliban-tools-builtin`) or Layer 4 (`caliban-cli`)

## Goals

Add a single new crate, `caliban-agent-core`, that drives an LLM agent loop on top of `caliban-provider`. Ships:

- `Tool` trait + `ToolRegistry` (so tool implementations live elsewhere — D will populate it with Read/Write/Edit/Bash/Grep/Glob).
- Stateless `Agent` (the primitive) and stateful `Session` (the convenience wrapper).
- Single-turn primitive (`run_turn`), multi-turn driver (`run_until_done`), and streaming variant (`stream_until_done`).
- Cooperative cancellation via `tokio_util::sync::CancellationToken`.
- `Compactor` trait + three built-in impls (`Noop`, `DropOldest`, `Summarizing`).
- `RetryPolicy` with exponential backoff + jitter; honors `RateLimit::retry_after`.
- `Hooks` trait with default no-op; supports tool-approval `Deny`.
- High-level `TurnEvent` stream for the CLI/TUI to render incremental progress.

**Acceptance:** A consumer can write:

```rust
let provider: Box<dyn Provider + Send + Sync> = ...;
let mut registry = ToolRegistry::new();
registry.register(Arc::new(MyEchoTool));

let agent = Agent::builder()
    .provider(provider)
    .tools(registry)
    .build();

let initial = vec![
    Message::system_text("You are helpful."),
    Message::user_text("Use the echo tool to say hi."),
];

let outcome = agent.run_until_done(initial, CancellationToken::new()).await?;
println!("{:?}", outcome.final_messages);
```

…and get back a fully-driven multi-turn conversation that called the registered tool, dispatched the result back to the model, and concluded with `stop_reason: EndTurn`.

## Non-goals (explicit deferrals)

- The actual filesystem/shell tool implementations (Read/Write/Edit/Bash/Grep/Glob) — sub-project D's deliverable.
- Subagent / nested-loop pattern (one agent calling another) — future work; current scope is flat.
- MCP client integration — Layer 2; `Tool` trait stays MCP-agnostic but `caliban-mcp-client` will provide an `McpTool` impl later.
- Concurrent tool execution within a single turn — Anthropic's API can emit parallel tool_use blocks; C dispatches them **sequentially** in the order received. Parallelism is a follow-on.
- Persistence (saving/loading sessions to disk) — Layer 4 / sessions sub-project.
- Cost ceiling / budget tracking — future enhancement.
- Caller-facing "edit history mid-run" hooks — out of scope.

## Crate structure

```
crates/caliban-agent-core/
├── Cargo.toml
└── src/
    ├── lib.rs              re-exports + module wiring
    ├── agent.rs            Agent, AgentBuilder, AgentConfig
    ├── session.rs          Session, SessionBuilder
    ├── tool.rs             Tool trait, ToolError, ToolContext
    ├── registry.rs         ToolRegistry
    ├── turn.rs             run_turn + run_until_done; TurnOutcome, RunOutcome
    ├── stream.rs           TurnEvent, TurnEventStream, stream adapter
    ├── compact.rs          Compactor trait + Noop / DropOldest / Summarizing
    ├── retry.rs            RetryPolicy + retry executor
    ├── hooks.rs            Hooks trait + NoopHooks + HookDecision
    └── error.rs            Error (wraps caliban_provider::Error + adds Agent-specific variants)
```

**Cargo deps:**
- `caliban-provider` (path)
- `async-trait`, `tokio` (with `sync`, `time`, `macros`, `rt`), `tokio-util` (for `CancellationToken`)
- `serde`, `serde_json` (for tool input/output JSON)
- `thiserror`
- `futures` (Stream traits)
- `tracing` (lightweight; `#[tracing::instrument]` annotations only, no required tracing setup)
- Dev: `caliban-provider/mock` feature (for MockProvider in tests)

**Workspace member:** add `"crates/caliban-agent-core"` to root `Cargo.toml`.

## Public API

### `Agent` and `AgentBuilder`

```rust
pub struct Agent {
    provider: Arc<dyn caliban_provider::Provider + Send + Sync>,
    tools: ToolRegistry,
    config: AgentConfig,
    compactor: Arc<dyn Compactor + Send + Sync>,
    retry: RetryPolicy,
    hooks: Arc<dyn Hooks + Send + Sync>,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,                 // canonical model name
    pub max_tokens: u32,               // per-turn output limit
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stop_sequences: Vec<String>,
    pub thinking: Option<caliban_provider::ThinkingConfig>,
    pub user_id: Option<String>,
    pub max_turns: u32,                // safety cap on run_until_done loops (default 50)
    pub tool_choice: caliban_provider::ToolChoice,
}

pub struct AgentBuilder { ... }

impl Agent {
    pub fn builder() -> AgentBuilder;
    pub async fn run_turn(&self, messages: Vec<Message>, cancel: CancellationToken) -> Result<TurnOutcome>;
    pub async fn run_until_done(&self, messages: Vec<Message>, cancel: CancellationToken) -> Result<RunOutcome>;
    pub async fn stream_until_done(&self, messages: Vec<Message>, cancel: CancellationToken) -> Result<TurnEventStream>;
}

impl AgentBuilder {
    pub fn provider(self, p: impl Into<Arc<dyn Provider + Send + Sync>>) -> Self;
    pub fn tools(self, registry: ToolRegistry) -> Self;
    pub fn config(self, cfg: AgentConfig) -> Self;
    pub fn model(self, model: impl Into<String>) -> Self;
    pub fn max_tokens(self, n: u32) -> Self;
    pub fn max_turns(self, n: u32) -> Self;
    pub fn compactor(self, c: Arc<dyn Compactor + Send + Sync>) -> Self;
    pub fn retry_policy(self, p: RetryPolicy) -> Self;
    pub fn hooks(self, h: Arc<dyn Hooks + Send + Sync>) -> Self;
    pub fn build(self) -> Result<Agent>;   // validates required fields
}
```

### `TurnOutcome` and `RunOutcome`

```rust
pub struct TurnOutcome {
    pub assistant_message: Message,           // the model's response (Role::Assistant)
    pub tool_results: Vec<Message>,           // 0 or 1 Role::User message with ToolResult blocks
    pub stop_reason: StopReason,
    pub usage: Usage,
    pub continue_loop: bool,                  // true iff stop_reason == ToolUse
}

pub struct RunOutcome {
    pub final_messages: Vec<Message>,         // includes initial + all assistant + tool_result messages
    pub turn_count: u32,
    pub total_usage: Usage,
    pub stopped_for: StopCondition,
}

pub enum StopCondition {
    EndOfTurn,                                // model returned stop_reason: EndTurn
    MaxTurnsReached(u32),
    Cancelled,
    ProviderError(/* boxed for display */ String),
    HookDenied(String),
    CompactionFailed(String),
}
```

### `Tool` trait

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> &serde_json::Value;
    async fn invoke(&self, input: serde_json::Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError>;
}

pub struct ToolContext {
    pub tool_use_id: String,
    pub cancel: CancellationToken,
}

#[derive(thiserror::Error, Debug)]
pub enum ToolError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("execution failed: {0}")]
    Execution(Box<dyn std::error::Error + Send + Sync>),
    #[error("cancelled")]
    Cancelled,
}

impl ToolError {
    pub fn execution(e: impl std::error::Error + Send + Sync + 'static) -> Self;
    pub fn invalid_input(msg: impl Into<String>) -> Self;
}
```

**Conversion to caliban_provider::Tool:** the registry exposes `to_caliban_tools()` that snapshots names/descriptions/schemas into `Vec<caliban_provider::Tool>` for inclusion in `CompletionRequest`.

**Result handling rules:**
- `Ok(content)` → `ToolResult { tool_use_id, content, is_error: false }`.
- `Err(ToolError::InvalidInput(s))` or `Err(ToolError::Execution(e))` → `ToolResult { tool_use_id, content: vec![Text(format!("Error: {e}"))], is_error: true }`. The agent continues the loop — the model can react to the error.
- `Err(ToolError::Cancelled)` → propagates as `Error::Cancelled`; the loop stops.

### `ToolRegistry`

```rust
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> &mut Self;
    pub fn unregister(&mut self, name: &str) -> Option<Arc<dyn Tool>>;
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>>;
    pub fn names(&self) -> impl Iterator<Item = &str>;
    pub fn to_caliban_tools(&self) -> Vec<caliban_provider::Tool>;
}
```

Registering a tool whose `name()` collides with an existing one **replaces** the existing entry (last-write-wins), with a `tracing::warn!` log. Predictable; matches HashMap semantics.

### `TurnEvent` stream

```rust
pub enum TurnEvent {
    TurnStart { turn_index: u32, message_id: String, model: String },
    AssistantTextDelta { turn_index: u32, content_block_index: u32, text: String },
    AssistantThinkingDelta { turn_index: u32, content_block_index: u32, text: String },
    ToolCallStart { turn_index: u32, tool_use_id: String, name: String },
    ToolCallInputDelta { turn_index: u32, tool_use_id: String, partial_json: String },
    ToolCallEnd { turn_index: u32, tool_use_id: String, is_error: bool, content: Vec<ContentBlock> },
    TurnEnd { turn_index: u32, stop_reason: StopReason, usage: Usage },
    RunEnd { final_messages: Vec<Message>, total_usage: Usage, stopped_for: StopCondition },
}

pub type TurnEventStream = Pin<Box<dyn Stream<Item = Result<TurnEvent>> + Send + 'static>>;
```

The stream is produced by `Agent::stream_until_done`. Internally the agent consumes `caliban_provider::StreamEvent`s from the provider and re-emits at the `TurnEvent` level. Tool execution is bracketed by `ToolCallStart` / `ToolCallEnd` events; the model's tool_use input JSON is forwarded via `ToolCallInputDelta` (mirroring the streaming-event surface from B).

`run_until_done` is implemented in terms of `stream_until_done` internally — it consumes the stream and accumulates the final state. This is a single source of truth for the agent loop.

### `Compactor`

```rust
#[async_trait]
pub trait Compactor: Send + Sync {
    /// Decide whether to compact and produce a new message list.
    /// Returns the new messages if compaction was applied; None if no-op.
    async fn compact(
        &self,
        messages: &[Message],
        capabilities: &caliban_provider::Capabilities,
    ) -> Result<Option<Vec<Message>>>;
}

pub struct NoopCompactor;

pub struct DropOldestCompactor {
    pub target_fraction: f32,         // start compacting when estimated token count > target_fraction * max_input_tokens
    pub keep_recent_turns: u32,       // always preserve the last N turns
}

pub struct SummarizingCompactor {
    pub provider: Arc<dyn Provider + Send + Sync>,   // typically the same as the agent's
    pub summarizer_model: String,                    // the canonical model name to summarize with
    pub target_fraction: f32,
    pub keep_recent_turns: u32,
}
```

Token counting is approximate (no tokenizer crate dependency in v1): count chars and divide by 4. Good-enough heuristic; precise counting is a follow-up.

The agent calls `compact(messages, &capabilities).await?` before each provider call. If `Ok(Some(new))`, the new list replaces the old. If `Ok(None)`, no-op. If `Err(...)`, surface as `Error::Compaction(...)` and stop the loop with `StopCondition::CompactionFailed`.

Default `Agent` uses `NoopCompactor` — explicit opt-in for the other two.

### `RetryPolicy`

```rust
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,           // default 3 (1 initial + 2 retries)
    pub initial_backoff: Duration,   // default 500ms
    pub backoff_multiplier: f32,     // default 2.0
    pub max_backoff: Duration,       // default 30s
    pub jitter: bool,                // default true
}

impl Default for RetryPolicy { fn default() -> Self { ... } }
impl RetryPolicy {
    pub fn no_retry() -> Self;       // max_attempts = 1
}
```

**Retry classifier:**
- `Error::RateLimit { retry_after }` → retryable; use `retry_after` if `Some(d)`, else exponential backoff.
- `Error::Network(_)` → retryable; exponential backoff.
- `Error::ServerError { status: 502..=599, .. }` → retryable; exponential backoff.
- `Error::ServerError { status: 500, .. }` → NOT retryable by default (500 is "internal error" — likely a bug, not transient). Treat as adapter error.
- All others (`Auth`, `InvalidRequest`, `ContextTooLong`, `ContentFilter`, `Cancelled`, `Adapter`, `ModelUnavailable`) → not retryable.

Jitter: `actual_backoff = backoff * (0.5 + rand::random::<f32>() * 0.5)` (50–100% of computed). Uses `tokio::time::sleep`.

Retries wrap **only** the provider call (`Provider::complete` or `Provider::stream`). Tool dispatch failures are NOT retried — tools handle their own retry semantics if they need any.

### `Hooks`

```rust
#[async_trait]
pub trait Hooks: Send + Sync {
    async fn before_turn(&self, _ctx: &TurnCtx<'_>) -> Result<()> { Ok(()) }
    async fn after_turn(&self, _ctx: &TurnCtx<'_>, _outcome: &TurnOutcome) -> Result<()> { Ok(()) }
    async fn before_tool(&self, _ctx: &ToolCtx<'_>) -> Result<HookDecision> { Ok(HookDecision::Allow) }
    async fn after_tool(&self, _ctx: &ToolCtx<'_>, _result: &std::result::Result<Vec<ContentBlock>, ToolError>) -> Result<()> { Ok(()) }
}

pub enum HookDecision {
    Allow,
    Deny(String),                   // includes message presented to the model as a tool_result
}

pub struct TurnCtx<'a> {
    pub turn_index: u32,
    pub messages: &'a [Message],
    pub config: &'a AgentConfig,
}

pub struct ToolCtx<'a> {
    pub turn_index: u32,
    pub tool_use_id: &'a str,
    pub tool_name: &'a str,
    pub input: &'a serde_json::Value,
}

pub struct NoopHooks;
#[async_trait] impl Hooks for NoopHooks {}
```

**Deny semantics:** `before_tool` returning `HookDecision::Deny(msg)` causes the agent to NOT invoke the tool. Instead, it synthesizes a `ToolResult { tool_use_id, content: vec![Text(format!("Tool call denied: {msg}"))], is_error: true }` and continues the loop. The `after_tool` hook still fires with `Err(ToolError::Execution(...))` for symmetry.

If a hook's own `Result` is `Err`, the loop stops with `Error::HookFailed(...)` and `StopCondition::HookDenied`. (The naming `HookDenied` is slightly inconsistent — it covers both deny and error cases. Acceptable for v1; rename if it gets confusing.)

### `Error` type

```rust
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("provider error: {0}")]
    Provider(#[from] caliban_provider::Error),

    #[error("tool '{tool}' execution failed: {source}")]
    ToolExecution { tool: String, source: Box<dyn std::error::Error + Send + Sync> },

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

### `Session`

```rust
pub struct Session {
    agent: Arc<Agent>,
    messages: Vec<Message>,
    cancel: CancellationToken,
}

impl Session {
    pub fn new(agent: Arc<Agent>) -> Self;

    pub fn system(&mut self, text: impl Into<String>) -> &mut Self;
    pub fn user_text(&mut self, text: impl Into<String>) -> &mut Self;
    pub fn user_message(&mut self, msg: Message) -> &mut Self;
    pub fn extend_messages(&mut self, msgs: impl IntoIterator<Item = Message>) -> &mut Self;

    /// Run until the next stable state, append all generated messages to history, return the new tail.
    pub async fn run(&mut self) -> Result<&[Message]>;
    pub async fn stream(&mut self) -> Result<TurnEventStream>;

    pub fn messages(&self) -> &[Message];
    pub fn clear(&mut self);
    pub fn cancel(&self);   // signal the embedded cancel token; ongoing calls return Cancelled
}
```

The Session wraps an `Arc<Agent>` so multiple sessions can share one agent (sharing the same provider client / tool registry / configuration). The `cancel` token is per-session.

## Implementation notes

### Single source of truth: `stream_until_done`

`run_until_done` is implemented as:

```rust
pub async fn run_until_done(&self, messages: Vec<Message>, cancel: CancellationToken) -> Result<RunOutcome> {
    let mut stream = self.stream_until_done(messages, cancel).await?;
    let mut total_usage = Usage::default();
    let mut final_messages = None;
    let mut stopped_for = StopCondition::EndOfTurn;
    while let Some(event) = stream.next().await {
        match event? {
            TurnEvent::TurnEnd { usage, .. } => total_usage.merge(usage),
            TurnEvent::RunEnd { final_messages: fm, total_usage: tu, stopped_for: sc } => {
                final_messages = Some(fm);
                total_usage = tu;
                stopped_for = sc;
                break;
            }
            _ => {}  // ignore intermediate events
        }
    }
    let final_messages = final_messages.ok_or_else(|| Error::Misconfigured("stream ended without RunEnd".into()))?;
    Ok(RunOutcome { final_messages, turn_count: /* derived */ , total_usage, stopped_for })
}
```

`run_turn` is a thin wrapper that runs exactly one iteration of the inner loop.

### Provider-call shape

The agent constructs each `CompletionRequest` from the current message list:

```rust
let req = CompletionRequest {
    model: config.model.clone(),
    messages: messages.clone(),
    tools: tools.to_caliban_tools(),
    tool_choice: config.tool_choice.clone(),
    max_tokens: config.max_tokens,
    temperature: config.temperature,
    top_p: config.top_p,
    top_k: None,                                   // can be added in a follow-up
    stop_sequences: config.stop_sequences.clone(),
    thinking: config.thinking.clone(),
    metadata: caliban_provider::RequestMetadata { user_id: config.user_id.clone() },
};
```

### Cancellation insertion points

Before each:
- Provider call (`stream` invocation).
- Tool dispatch.
- Each stream-event yield.

Cancellation produces `Error::Cancelled` and `StopCondition::Cancelled`.

### Retry executor

Implemented as a helper:

```rust
async fn with_retry<F, Fut, T>(policy: &RetryPolicy, cancel: &CancellationToken, mut f: F) -> Result<T>
where F: FnMut() -> Fut, Fut: Future<Output = caliban_provider::Result<T>>
```

Iterates up to `policy.max_attempts`. Between attempts, calls `tokio::time::sleep` with the computed backoff, also racing against `cancel.cancelled()` so cancellation interrupts the wait.

## Testing strategy

### Unit tests
- `ToolRegistry` register/get/unregister/names/to_caliban_tools.
- `RetryPolicy` backoff math (deterministic; jitter disabled in tests).
- `DropOldestCompactor` truncation behavior (preserves system + last N).
- `SummarizingCompactor` happy path with a `MockProvider`.

### Integration tests
Use `caliban_provider::MockProvider` (feature `mock`) to script provider behavior.

- **single_turn_no_tools** — assistant responds with text only, `stop_reason: EndTurn`. `run_turn` returns one assistant message; `run_until_done` returns one assistant message and stops.
- **single_turn_with_tool_call** — assistant responds with a tool_use, mock tool returns content, next turn ends. Verify the tool was invoked, the tool_result was appended, and the loop completed.
- **tool_call_with_error** — tool returns `Err(Execution(...))`, agent inserts `is_error: true` tool_result, model continues.
- **multi_turn_tool_chain** — three turns: tool_use → tool_result → tool_use → tool_result → end.
- **cancellation_mid_turn** — caller cancels during a tool dispatch; verify `Error::Cancelled`.
- **max_turns_reached** — provider keeps emitting tool_use; verify `Error::MaxTurnsReached(50)`.
- **retry_on_rate_limit** — `MockProvider` errs twice with `RateLimit { retry_after: Some(Duration::from_millis(10)) }`, then succeeds. Verify total wait time is at least 20ms.
- **retry_not_attempted_on_auth** — `RateLimit` would retry; `Auth(...)` does not.
- **hook_denies_tool** — `before_tool` returns `Deny("not authorized")`; verify `tool_result` content includes "not authorized" and the loop continues.
- **compaction_triggered** — `DropOldestCompactor` activates when message history exceeds threshold; verify final messages list is shorter than the input.
- **stream_until_done_emits_all_events** — exercise the full TurnEvent enum coverage.

### Property tests
- After a tool call, the next message in the history is always `Role::User` with `ToolResult` blocks matching the assistant's `ToolUse` ids one-to-one.
- The `RunOutcome.final_messages` always starts with the same prefix as the input `messages` argument.

## Acceptance criteria

C is done when **all** of the following hold:

**Workspace**
- `crates/caliban-agent-core` exists as a workspace member.
- `cargo build --workspace` succeeds with no warnings.
- `cargo build --workspace --features caliban-provider/mock` succeeds.

**Crate quality**
- `cargo clippy --workspace --all-targets -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.

**Tests**
- `cargo test --workspace` passes — at least 15 new tests in `caliban-agent-core`, exercising all 11 integration scenarios from the strategy.
- `cargo test --workspace --features caliban-provider/mock` exercises MockProvider-based integration tests.
- Property tests pass with default proptest configuration.

**Public API**
- All types listed in this spec are exported from `caliban-agent-core`.
- `Box<dyn Tool + Send + Sync>` compiles. `Box<dyn Hooks + Send + Sync>` compiles. `Box<dyn Compactor + Send + Sync>` compiles. (Object safety verified.)

**Documentation**
- One new ADR at `adrs/0009-agent-core-design.md` capturing: stream-as-primitive, retry-only-on-provider-call, sequential tool execution, NoopCompactor default.
- README updated with a runnable example using `caliban-agent-core` + `MockProvider`.

**Cross-crate**
- The existing tests in `caliban-provider*` continue to pass unchanged.
- No new dependencies added to `caliban-provider` or its adapters.

## Risks

- **Stream-as-primitive complexity.** Implementing `stream_until_done` as the sole source of truth means the streaming code path is exercised for non-streaming consumers. If there's a subtle bug in the stream-to-collected-state conversion, both APIs are affected. Mitigation: comprehensive integration tests including streaming-specific events.
- **Token-counting heuristic accuracy.** `chars / 4` will mis-estimate non-English text and tool-heavy histories. Mitigation: `SummarizingCompactor` is opt-in; the default `NoopCompactor` means the user picks compaction explicitly. A future tokenizer integration sub-project can replace the heuristic.
- **Cancellation race conditions.** Cooperative cancellation requires checking the token at every yield point; missing a check causes graceful-degradation issues (e.g., a tool runs to completion after the user cancelled). Mitigation: dedicated cancellation test covering each insertion point.
- **Sequential tool execution.** Anthropic and Gemini can emit parallel `tool_use` blocks in one response. Running them sequentially is correct but slower than necessary. Mitigation: documented in this spec as deferred; a follow-on adds `Hooks::tool_dispatch_strategy() -> Sequential | Parallel`.
- **Hook re-entrancy.** `before_tool` can theoretically call back into the agent (via the registry held by Arc). The first version doesn't try to prevent this; documented as "don't do that."

## Open questions

None blocking. The deferrals above are conscious.
