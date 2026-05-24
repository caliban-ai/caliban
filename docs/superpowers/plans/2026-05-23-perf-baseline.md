# Performance Baseline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the highest-value low-risk performance work: Anthropic prompt caching enabled, OpenAI cache hits surfaced, HTTP/2 + hickory-dns transport, TTFT/TBT measurement, and a few cleanups.

**Architecture:** Most changes are localized. The largest is in `crates/caliban-agent-core`: an `apply_prompt_cache` helper that marks the last system text block + last tool def with `cache_control: Ephemeral`, threaded through `AgentBuilder.prompt_cache(bool)` and called from the turn build. TUI changes extend `TranscriptLine::UsageSummary` with cache + TTFT fields. Transport changes are workspace `Cargo.toml` feature toggles.

**Tech Stack:** Rust 1.95, tokio, reqwest (with http2 + hickory-dns), `eventsource-stream`, tracing.

**Spec:** `docs/superpowers/specs/2026-05-23-perf-baseline-design.md`

---

## File map

| File | Change | Approx LOC |
|---|---|---|
| `Cargo.toml` (workspace) | Add `"http2"`, `"hickory-dns"` to reqwest features | ~1 |
| `caliban/src/main.rs` | Add `--no-prompt-cache` flag; replace 2 `std::fs` with `tokio::fs`; thread flag into Agent builder | ~15 |
| `crates/caliban-agent-core/src/agent.rs` | `AgentBuilder::prompt_cache(bool)` + `Agent.prompt_cache: bool` field | ~15 |
| `crates/caliban-agent-core/src/cache.rs` (new) | `apply_prompt_cache` helper + tests | ~120 |
| `crates/caliban-agent-core/src/lib.rs` | `pub(crate) mod cache;` | +1 |
| `crates/caliban-agent-core/src/stream.rs` | Call `apply_prompt_cache`; add `TurnTiming` capture; emit cache/timing tracing; extend `TurnEvent::TurnEnd` with `ttft` + `tbt` | ~60 |
| `crates/caliban-agent-core/tests/cache.rs` (new) | wiremock integration: assert wire JSON has `cache_control` markers | ~120 |
| `caliban/src/tui.rs` | Extend `TranscriptLine::UsageSummary` with `cache_read`, `cache_creation`, `last_turn_ttft_ms`; update render; track per-turn TTFT for UsageSummary | ~50 |
| `crates/caliban-provider-google/src/ir_convert.rs` | One-line TODO comment near hardcoded cache fields | +3 |

Total: ~390 LOC including tests.

---

## Task 1: Enable HTTP/2 + hickory-dns

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Edit workspace reqwest feature list**

Current line (`Cargo.toml`):
```toml
reqwest            = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
```

Change to:
```toml
reqwest            = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream", "http2", "hickory-dns"] }
```

- [ ] **Step 2: Build clean**

Run: `cargo build --workspace`
Expected: success. New deps will compile (hickory pulls ~5 small crates).

- [ ] **Step 3: Run full test suite**

Run: `cargo test --workspace`
Expected: all green. No behavior change at the test level; just transport features.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: enable HTTP/2 multiplexing + hickory-dns on reqwest"
```

---

## Task 2: tokio::fs cleanup + Gemini caching TODO

**Files:**
- Modify: `caliban/src/main.rs`
- Modify: `crates/caliban-provider-google/src/ir_convert.rs`

- [ ] **Step 1: Inspect the std::fs call sites**

Run: `grep -n "std::fs::" caliban/src/main.rs`
Expected: two hits around lines 319 and 321, inside a function that sets up the debug log.

- [ ] **Step 2: Replace std::fs::create_dir_all with tokio::fs**

Find:
```rust
std::fs::create_dir_all(parent)
```

Replace with:
```rust
tokio::fs::create_dir_all(parent).await
```

- [ ] **Step 3: Replace std::fs::OpenOptions with tokio::fs::OpenOptions**

Find the `std::fs::OpenOptions::new()...open(&path)` chain (one line). The `tokio::fs::OpenOptions` API mirrors `std::fs` but its `.open()` is async. Replace:

```rust
std::fs::OpenOptions::new().create(true).append(true).open(&path)
```

with:

```rust
tokio::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&path)
    .await
```

The returned `tokio::fs::File` is not directly compatible with `tracing_subscriber`'s `fmt` layer (which wants `std::io::Write`, not `tokio::io::AsyncWrite`). Convert it back to a std file:

```rust
let file = tokio::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&path)
    .await?
    .into_std()
    .await;
```

(`tokio::fs::File::into_std` returns a `std::fs::File` synchronously after offloading to the blocking pool. This preserves the existing tracing-subscriber wiring.)

- [ ] **Step 4: Verify caller is async**

The containing function must already be `async`. If not, the std::fs calls were running in sync context and the change is moot; revert. Read the surrounding function signature and confirm.

- [ ] **Step 5: Add Gemini TODO comment**

Open `crates/caliban-provider-google/src/ir_convert.rs`, find the line `cache_creation_input_tokens: None,` (around line 339). Add this block-comment immediately above:

```rust
// Gemini's context caching uses a separate `cachedContents` API resource
// rather than per-block markers. Not implemented; revisit when needed.
```

(Add the same comment near the equivalent line in `stream_parse.rs` if it's present.)

- [ ] **Step 6: Build + test**

Run: `cargo build --workspace && cargo test --workspace`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add caliban/src/main.rs crates/caliban-provider-google/src/ir_convert.rs crates/caliban-provider-google/src/stream_parse.rs
git commit -m "chore: tokio::fs in async startup path; TODO for Gemini caching"
```

---

## Task 3: apply_prompt_cache helper (pure logic)

**Files:**
- Create: `crates/caliban-agent-core/src/cache.rs`
- Modify: `crates/caliban-agent-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/caliban-agent-core/src/cache.rs`:

```rust
//! Apply Anthropic-style prompt-cache markers to the system message and tools.
//!
//! `cache_control: Ephemeral` is set on:
//! - the last TextBlock of the system message (if any), AND
//! - the last `ToolDef` in the tools array (if any).
//!
//! For non-Anthropic providers the IR field is `Option<>` and serializes as
//! absent, so this is a no-op on the wire.

use caliban_provider::{CacheControl, ContentBlock, Message, Role, ToolDef};

/// Mutate `messages` and `tools` in place: set `cache_control: Ephemeral` on
/// the last system-message TextBlock and on the last tool definition.
pub(crate) fn apply_prompt_cache(messages: &mut [Message], tools: &mut [ToolDef]) {
    if let Some(sys) = messages.iter_mut().find(|m| m.role == Role::System) {
        if let Some(last_text) = sys
            .content
            .iter_mut()
            .rev()
            .find_map(|b| match b {
                ContentBlock::Text(t) => Some(t),
                _ => None,
            })
        {
            last_text.cache_control = Some(CacheControl::Ephemeral);
        }
    }
    if let Some(last_tool) = tools.last_mut() {
        last_tool.cache_control = Some(CacheControl::Ephemeral);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::{IrTextBlock, ToolDef};
    use serde_json::json;

    fn tool(name: &str) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: format!("{name} tool"),
            input_schema: json!({"type": "object"}),
            cache_control: None,
        }
    }

    #[test]
    fn empty_inputs_do_not_panic() {
        let mut msgs: Vec<Message> = Vec::new();
        let mut tools: Vec<ToolDef> = Vec::new();
        apply_prompt_cache(&mut msgs, &mut tools);
        assert!(msgs.is_empty());
        assert!(tools.is_empty());
    }

    #[test]
    fn marks_last_system_text_block() {
        let mut msgs = vec![Message {
            role: Role::System,
            content: vec![
                ContentBlock::Text(IrTextBlock {
                    text: "first".into(),
                    cache_control: None,
                }),
                ContentBlock::Text(IrTextBlock {
                    text: "second".into(),
                    cache_control: None,
                }),
            ],
        }];
        let mut tools: Vec<ToolDef> = Vec::new();
        apply_prompt_cache(&mut msgs, &mut tools);
        match (&msgs[0].content[0], &msgs[0].content[1]) {
            (ContentBlock::Text(a), ContentBlock::Text(b)) => {
                assert!(a.cache_control.is_none());
                assert!(matches!(b.cache_control, Some(CacheControl::Ephemeral)));
            }
            _ => panic!("expected two text blocks"),
        }
    }

    #[test]
    fn marks_last_tool() {
        let mut msgs: Vec<Message> = Vec::new();
        let mut tools = vec![tool("a"), tool("b"), tool("c")];
        apply_prompt_cache(&mut msgs, &mut tools);
        assert!(tools[0].cache_control.is_none());
        assert!(tools[1].cache_control.is_none());
        assert!(matches!(tools[2].cache_control, Some(CacheControl::Ephemeral)));
    }

    #[test]
    fn system_message_without_text_blocks_is_safe() {
        let mut msgs = vec![Message {
            role: Role::System,
            content: Vec::new(),
        }];
        let mut tools = vec![tool("a")];
        apply_prompt_cache(&mut msgs, &mut tools);
        assert!(matches!(tools[0].cache_control, Some(CacheControl::Ephemeral)));
    }

    #[test]
    fn user_messages_unchanged() {
        let mut msgs = vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::Text(IrTextBlock {
                    text: "sys".into(),
                    cache_control: None,
                })],
            },
            Message::user_text("hello"),
        ];
        let mut tools: Vec<ToolDef> = Vec::new();
        apply_prompt_cache(&mut msgs, &mut tools);
        let user = &msgs[1];
        match &user.content[0] {
            ContentBlock::Text(t) => assert!(t.cache_control.is_none()),
            _ => panic!("expected text"),
        }
    }
}
```

- [ ] **Step 2: Declare the module**

Edit `crates/caliban-agent-core/src/lib.rs`, add `pub(crate) mod cache;` alongside the other module declarations.

- [ ] **Step 3: Run tests**

Run: `cargo test -p caliban-agent-core cache::`
Expected: 5 tests pass.

(If `IrTextBlock` is the wrong type name, find the actual name: `grep -n "pub struct.*TextBlock" crates/caliban-provider/src/message.rs`. Use whatever is exported as the inner type of `ContentBlock::Text(...)`.)

- [ ] **Step 4: Commit**

```bash
git add crates/caliban-agent-core/src/cache.rs crates/caliban-agent-core/src/lib.rs
git commit -m "agent-core: apply_prompt_cache helper (pure logic + tests)"
```

---

## Task 4: AgentBuilder.prompt_cache + wire into turn loop

**Files:**
- Modify: `crates/caliban-agent-core/src/agent.rs`
- Modify: `crates/caliban-agent-core/src/stream.rs`

- [ ] **Step 1: Add field + builder method**

In `crates/caliban-agent-core/src/agent.rs`, add to `Agent`:

```rust
pub struct Agent {
    pub(crate) provider: Arc<dyn Provider + Send + Sync>,
    pub(crate) tools: ToolRegistry,
    pub(crate) config: AgentConfig,
    pub(crate) compactor: Arc<dyn crate::compact::Compactor + Send + Sync>,
    pub(crate) retry: crate::retry::RetryPolicy,
    pub(crate) hooks: Arc<dyn Hooks + Send + Sync>,
    pub(crate) prompt_cache: bool,
}
```

Add to `AgentBuilder`:

```rust
pub struct AgentBuilder {
    provider: Option<Arc<dyn Provider + Send + Sync>>,
    tools: ToolRegistry,
    config: AgentConfig,
    compactor: Option<Arc<dyn crate::compact::Compactor + Send + Sync>>,
    retry: Option<crate::retry::RetryPolicy>,
    hooks: Option<Arc<dyn Hooks + Send + Sync>>,
    prompt_cache: bool,
}
```

The `Default` derive on `AgentBuilder` will give `prompt_cache: false` automatically. To make the *agent default* opt-in for callers using the builder without configuring it, set `Default` to `true`:

```rust
impl Default for AgentBuilder {
    fn default() -> Self {
        Self {
            provider: None,
            tools: ToolRegistry::default(),
            config: AgentConfig::default(),
            compactor: None,
            retry: None,
            hooks: None,
            prompt_cache: true,
        }
    }
}
```

(Remove `#[derive(Default)]` on the struct since we're hand-rolling it.)

Add a builder method:

```rust
/// Enable or disable Anthropic-style prompt cache markers on the system
/// prompt and last tool definition. Default: enabled.
#[must_use]
pub fn prompt_cache(mut self, on: bool) -> Self {
    self.prompt_cache = on;
    self
}
```

And update `build()` to thread the field:

```rust
Ok(Agent {
    provider,
    tools: self.tools,
    config: self.config,
    compactor: self
        .compactor
        .unwrap_or_else(|| Arc::new(crate::compact::NoopCompactor)),
    retry: self.retry.unwrap_or_default(),
    hooks: self.hooks.unwrap_or_else(|| Arc::new(NoopHooks)),
    prompt_cache: self.prompt_cache,
})
```

- [ ] **Step 2: Apply in the turn-build path**

Edit `crates/caliban-agent-core/src/stream.rs`. Find the spot where each turn's `CompletionRequest` is built (around line 412–426 per the audit). Just before constructing the request from `messages` and `tools`, call:

```rust
let mut messages = history.clone();
let mut tools: Vec<caliban_provider::ToolDef> = self.tools.tool_defs();
if self.prompt_cache {
    crate::cache::apply_prompt_cache(&mut messages, &mut tools);
}
```

(Adapt to the existing variable names. If `self.tools.tool_defs()` doesn't exist, use whatever the existing code uses to build the tool-def vec.)

Then pass `messages` and `tools` into `CompletionRequest` construction.

- [ ] **Step 3: Build**

Run: `cargo build --workspace`
Expected: clean.

- [ ] **Step 4: Run agent-core tests**

Run: `cargo test -p caliban-agent-core`
Expected: existing tests still pass. (No behavior change for non-Anthropic providers; Anthropic receives extra cache_control keys but our existing mock-provider tests don't assert on them.)

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/agent.rs crates/caliban-agent-core/src/stream.rs
git commit -m "agent-core: wire prompt_cache through builder + per-turn application"
```

---

## Task 5: Wiremock integration test for Anthropic wire format

**Files:**
- Create: `crates/caliban-agent-core/tests/cache.rs`

- [ ] **Step 1: Write the failing test**

The workspace already has `wiremock` as a dev-dep. Use it to spin up a mock Anthropic endpoint, send one turn through the agent with caching on, and assert the request body has `cache_control` on the expected positions.

Create `crates/caliban-agent-core/tests/cache.rs`:

```rust
//! End-to-end: assert that prompt_cache=true causes cache_control markers
//! to land in the outgoing Anthropic wire JSON.

use std::sync::Arc;

use caliban_agent_core::{Agent, ToolRegistry};
use caliban_provider::{Provider, Tool};
use caliban_provider_anthropic::{AnthropicProvider, config::DirectConfig};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Trivial dummy tool that returns an empty result.
struct Dummy {
    name: String,
}

#[async_trait::async_trait]
impl Tool for Dummy {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { "dummy" }
    fn input_schema(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _ctx: caliban_agent_core::ToolContext,
    ) -> std::result::Result<caliban_agent_core::ToolResult, caliban_agent_core::ToolError> {
        Ok(caliban_agent_core::ToolResult::text(""))
    }
}

#[tokio::test]
async fn prompt_cache_on_emits_cache_control_in_wire_json() {
    let server = MockServer::start().await;

    // Capture the request body.
    let captured: Arc<tokio::sync::Mutex<Option<Value>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    let captured_clone = Arc::clone(&captured);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(move |req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            *futures::executor::block_on(captured_clone.lock()) = Some(body);
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "msg_x",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "ok"}],
                "model": "claude-test",
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 1}
            }))
        })
        .mount(&server)
        .await;

    let cfg = DirectConfig::builder()
        .api_key(secrecy::SecretString::new("test".into()))
        .base_url(server.uri().parse().unwrap())
        .build()
        .unwrap();
    let provider: Arc<dyn Provider + Send + Sync> =
        Arc::new(AnthropicProvider::direct(cfg).unwrap());

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(Dummy { name: "a".into() }));
    registry.register(Arc::new(Dummy { name: "b".into() }));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider)
            .tools(registry)
            .model("claude-test")
            .max_tokens(64)
            .prompt_cache(true)
            .build()
            .unwrap(),
    );

    let mut session = caliban_agent_core::Session::new(agent);
    session
        .system("system prompt")
        .user_text("hi");
    let _ = session.run().await;

    let body = captured.lock().await.clone().expect("server received no request");

    // System message: last text block has cache_control.
    let system = &body["system"];
    // Anthropic wire format puts system as a string OR an array of blocks.
    // The IR converts system message to the array form when cache_control is present.
    let blocks = system.as_array().expect("expected array-form system");
    let last_block = blocks.last().unwrap();
    assert_eq!(last_block["cache_control"]["type"], "ephemeral");

    // Tools: last tool has cache_control.
    let tools = body["tools"].as_array().expect("expected tools array");
    let last_tool = tools.last().unwrap();
    assert_eq!(last_tool["cache_control"]["type"], "ephemeral");

    // No earlier tool has cache_control.
    for t in &tools[..tools.len() - 1] {
        assert!(t.get("cache_control").is_none(), "earlier tool was marked");
    }
}

#[tokio::test]
async fn prompt_cache_off_omits_cache_control() {
    let server = MockServer::start().await;
    let captured: Arc<tokio::sync::Mutex<Option<Value>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    let captured_clone = Arc::clone(&captured);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(move |req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            *futures::executor::block_on(captured_clone.lock()) = Some(body);
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "msg_x",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "ok"}],
                "model": "claude-test",
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 1}
            }))
        })
        .mount(&server)
        .await;

    let cfg = DirectConfig::builder()
        .api_key(secrecy::SecretString::new("test".into()))
        .base_url(server.uri().parse().unwrap())
        .build()
        .unwrap();
    let provider: Arc<dyn Provider + Send + Sync> =
        Arc::new(AnthropicProvider::direct(cfg).unwrap());

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(Dummy { name: "a".into() }));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider)
            .tools(registry)
            .model("claude-test")
            .max_tokens(64)
            .prompt_cache(false)
            .build()
            .unwrap(),
    );

    let mut session = caliban_agent_core::Session::new(agent);
    session.system("system prompt").user_text("hi");
    let _ = session.run().await;

    let body = captured.lock().await.clone().expect("server received no request");
    let serialized = body.to_string();
    assert!(
        !serialized.contains("cache_control"),
        "no cache_control expected when prompt_cache=false; got: {serialized}"
    );
}
```

(Adapt API shapes if `Session::system`/`Session::run`/`AnthropicProvider::direct`/`DirectConfig::builder` have different signatures. The test's intent is what matters: configure a server that captures the body, send one turn, assert the captured JSON.)

- [ ] **Step 2: Run the tests**

Run: `cargo test -p caliban-agent-core --test cache`
Expected: 2 tests pass.

If the test reveals the IR's system-message conversion doesn't switch from "string form" to "blocks form" when cache_control is present, that's a wire-format bug in the Anthropic adapter. Trace it: `crates/caliban-provider-anthropic/src/ir_convert.rs`, look at how system messages are encoded; cache_control on a text block should force block-array form. Fix any mismatch there before continuing.

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-agent-core/tests/cache.rs
git commit -m "test(agent-core): wire-format assertions for prompt_cache on/off"
```

---

## Task 6: CLI flag --no-prompt-cache

**Files:**
- Modify: `caliban/src/main.rs`

- [ ] **Step 1: Add the Args field**

In `caliban/src/main.rs`, near the existing `--debug` flag in the `Args` struct (around line 124–126), add:

```rust
/// Disable Anthropic-style prompt caching (default: enabled).
#[arg(long, env = "CALIBAN_NO_PROMPT_CACHE")]
pub(crate) no_prompt_cache: bool,
```

- [ ] **Step 2: Thread the flag into the Agent builder**

Find the `Agent::builder()` call in `caliban/src/main.rs` (likely inside the function that builds the Agent). Add `.prompt_cache(!args.no_prompt_cache)`:

```rust
let agent = Agent::builder()
    .provider(provider)
    .tools(registry)
    .model(args.model.clone().unwrap_or_else(default_model_for))
    .max_tokens(args.max_tokens)
    .max_turns(args.max_turns)
    .prompt_cache(!args.no_prompt_cache)
    .build()?;
```

(Adapt to the actual builder chain. The point is: add one line that passes the negated flag.)

- [ ] **Step 3: Build**

Run: `cargo build --workspace`
Expected: clean.

- [ ] **Step 4: Verify the flag appears in --help**

Run: `cargo run -- --help | grep -A1 prompt-cache`
Expected: shows `--no-prompt-cache` with the env var hint.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/main.rs
git commit -m "cli: --no-prompt-cache flag (CALIBAN_NO_PROMPT_CACHE env var)"
```

---

## Task 7: Cache visibility in UsageSummary + tracing event

**Files:**
- Modify: `crates/caliban-agent-core/src/stream.rs` (emit cache tracing event at TurnEnd)
- Modify: `caliban/src/tui.rs` (extend UsageSummary variant + render)

- [ ] **Step 1: Tracing event for cache hits at TurnEnd**

In `stream.rs`, find the spot where a turn ends and `total_usage.merge(turn_usage)` is called (around line 660 or 674 per the audit). Just after the merge, emit:

```rust
let cache_read = turn_usage.cache_read_input_tokens.unwrap_or(0);
let cache_creation = turn_usage.cache_creation_input_tokens.unwrap_or(0);
if cache_read > 0 || cache_creation > 0 {
    tracing::info!(
        target: "caliban::cache",
        cache_read,
        cache_creation,
        "prompt cache stats"
    );
}
```

- [ ] **Step 2: Extend UsageSummary variant**

In `caliban/src/tui.rs`, find `TranscriptLine::UsageSummary`:

```rust
UsageSummary {
    input_tokens: u32,
    output_tokens: u32,
    turn_count: u32,
},
```

Replace with:

```rust
UsageSummary {
    input_tokens: u32,
    output_tokens: u32,
    cache_read: Option<u32>,
    cache_creation: Option<u32>,
    last_turn_ttft_ms: Option<u64>,
    turn_count: u32,
},
```

(The `last_turn_ttft_ms` field is wired in Task 9; declare it now so the variant doesn't need a second edit.)

- [ ] **Step 3: Update construction site**

Find every `TranscriptLine::UsageSummary { ... }` construction. There should be one in `tui.rs` near where `TurnEvent::RunEnd` is handled (around line 1293). Set the new fields:

```rust
app.transcript.push(TranscriptLine::UsageSummary {
    input_tokens: total_usage.input_tokens,
    output_tokens: total_usage.output_tokens,
    cache_read: total_usage.cache_read_input_tokens,
    cache_creation: total_usage.cache_creation_input_tokens,
    last_turn_ttft_ms: None, // populated in Task 9
    turn_count,
});
```

- [ ] **Step 4: Update the render arm**

Find the `TranscriptLine::UsageSummary { ... } =>` arm in `render_transcript` (around line 729–740). Replace with:

```rust
TranscriptLine::UsageSummary {
    input_tokens,
    output_tokens,
    cache_read,
    cache_creation,
    last_turn_ttft_ms,
    turn_count,
} => {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("{turn_count} turns"));
    let mut tok = format!("{input_tokens}\u{2191}");
    let cache_suffix = match (cache_read, cache_creation) {
        (Some(r), Some(c)) if *r > 0 && *c > 0 => format!(" ({r} cached, {c} write)"),
        (Some(r), _) if *r > 0 => format!(" ({r} cached)"),
        (_, Some(c)) if *c > 0 => format!(" ({c} cache write)"),
        _ => String::new(),
    };
    tok.push_str(&cache_suffix);
    tok.push_str(&format!(" {output_tokens}\u{2193} tokens"));
    parts.push(tok);
    if let Some(ttft) = last_turn_ttft_ms {
        parts.push(format!("TTFT {ttft}ms"));
    }
    lines.push(Line::styled(
        format!("[caliban: {}]", parts.join(" \u{00B7} ")),
        Style::default().add_modifier(Modifier::DIM),
    ));
}
```

- [ ] **Step 5: Update the non-TUI session-save print site**

In `caliban/src/main.rs` (around line 1494), find the `eprintln!` showing token totals when a session is saved. Extend it to mention cache hits when present:

```rust
let cache_extra = match (s.total_usage.cache_read_input_tokens, s.total_usage.cache_creation_input_tokens) {
    (Some(r), Some(c)) if r > 0 && c > 0 => format!(" ({r} cached, {c} write)"),
    (Some(r), _) if r > 0 => format!(" ({r} cached)"),
    (_, Some(c)) if c > 0 => format!(" ({c} cache write)"),
    _ => String::new(),
};
eprintln!(
    "[caliban: saved session '{}' ({} turns, {} tokens{})]",
    s.name,
    s.turn_count(),
    s.total_usage.input_tokens + s.total_usage.output_tokens,
    cache_extra,
);
```

- [ ] **Step 6: Add a unit test for the render formatter**

To avoid coupling tests to ratatui's render state, extract the cache-suffix logic into a pure helper and test it:

In `caliban/src/tui.rs`, just below the `format_bytes` helper, add:

```rust
fn format_cache_suffix(cache_read: Option<u32>, cache_creation: Option<u32>) -> String {
    match (cache_read, cache_creation) {
        (Some(r), Some(c)) if r > 0 && c > 0 => format!(" ({r} cached, {c} write)"),
        (Some(r), _) if r > 0 => format!(" ({r} cached)"),
        (_, Some(c)) if c > 0 => format!(" ({c} cache write)"),
        _ => String::new(),
    }
}
```

Replace the inline expression in the render arm with `format_cache_suffix(cache_read, cache_creation)`.

In the existing `#[cfg(test)] mod tests` of `tui.rs`, add:

```rust
#[test]
fn cache_suffix_omitted_when_no_cache() {
    assert_eq!(format_cache_suffix(None, None), "");
    assert_eq!(format_cache_suffix(Some(0), Some(0)), "");
}

#[test]
fn cache_suffix_read_only() {
    assert_eq!(format_cache_suffix(Some(42), None), " (42 cached)");
    assert_eq!(format_cache_suffix(Some(42), Some(0)), " (42 cached)");
}

#[test]
fn cache_suffix_write_only() {
    assert_eq!(format_cache_suffix(None, Some(100)), " (100 cache write)");
    assert_eq!(format_cache_suffix(Some(0), Some(100)), " (100 cache write)");
}

#[test]
fn cache_suffix_both() {
    assert_eq!(
        format_cache_suffix(Some(42), Some(100)),
        " (42 cached, 100 write)"
    );
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p caliban --lib tests::cache_suffix`
Expected: 4 tests pass.

Run: `cargo test --workspace`
Expected: everything green.

- [ ] **Step 8: Commit**

```bash
git add crates/caliban-agent-core/src/stream.rs caliban/src/tui.rs caliban/src/main.rs
git commit -m "feat(tui): surface prompt-cache hit/write counts; tracing event"
```

---

## Task 8: TurnTiming capture + tracing event

**Files:**
- Modify: `crates/caliban-agent-core/src/stream.rs`
- Modify: `crates/caliban-agent-core/src/lib.rs` (if `TurnTiming` lives in a new module)

- [ ] **Step 1: Add TurnTiming struct + tests**

At the top of `crates/caliban-agent-core/src/stream.rs` (or in a new sibling file if preferred), add:

```rust
use std::time::{Duration, Instant};

#[derive(Debug)]
struct TurnTiming {
    request_sent_at: Instant,
    first_delta_at: Option<Instant>,
    last_delta_at: Option<Instant>,
    delta_count: u32,
}

impl TurnTiming {
    fn start() -> Self {
        Self {
            request_sent_at: Instant::now(),
            first_delta_at: None,
            last_delta_at: None,
            delta_count: 0,
        }
    }

    fn observe_delta(&mut self) {
        let now = Instant::now();
        self.first_delta_at.get_or_insert(now);
        self.last_delta_at = Some(now);
        self.delta_count += 1;
    }

    fn ttft(&self) -> Option<Duration> {
        self.first_delta_at
            .map(|t| t.saturating_duration_since(self.request_sent_at))
    }

    fn tbt(&self) -> Option<Duration> {
        match (self.first_delta_at, self.last_delta_at, self.delta_count) {
            (Some(f), Some(l), n) if n >= 2 => {
                Some(l.saturating_duration_since(f) / (n - 1))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod turn_timing_tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn no_delta_means_no_ttft() {
        let t = TurnTiming::start();
        assert!(t.ttft().is_none());
        assert!(t.tbt().is_none());
    }

    #[test]
    fn single_delta_gives_ttft_no_tbt() {
        let mut t = TurnTiming::start();
        sleep(Duration::from_millis(5));
        t.observe_delta();
        assert!(t.ttft().unwrap() >= Duration::from_millis(4));
        assert!(t.tbt().is_none());
    }

    #[test]
    fn multi_delta_gives_ttft_and_tbt() {
        let mut t = TurnTiming::start();
        sleep(Duration::from_millis(5));
        t.observe_delta();
        sleep(Duration::from_millis(10));
        t.observe_delta();
        sleep(Duration::from_millis(10));
        t.observe_delta();
        assert!(t.ttft().unwrap() >= Duration::from_millis(4));
        // Two intervals of ~10ms each → mean ~10ms.
        let tbt = t.tbt().unwrap();
        assert!(tbt >= Duration::from_millis(8) && tbt <= Duration::from_millis(30));
    }
}
```

- [ ] **Step 2: Wire TurnTiming into the per-turn loop**

In `stream.rs`, find the spot where a turn's request is sent and the per-event loop begins (around `provider_stream.next().await`, line 455). Just before sending, create `let mut timing = TurnTiming::start();`. Inside the event loop, every time a `StreamEvent::Delta { .. }` arrives, call `timing.observe_delta();`.

At the end of the turn (after the assistant message is built, around line 574), capture timings:

```rust
let ttft = timing.ttft();
let tbt = timing.tbt();
if let Some(ttft) = ttft {
    tracing::info!(
        target: "caliban::timing",
        ttft_ms = ttft.as_millis() as u64,
        tbt_ms = tbt.map(|d| d.as_millis() as u64),
        delta_count = timing.delta_count,
        "turn timing"
    );
}
```

- [ ] **Step 3: Extend TurnEvent::TurnEnd payload**

Find `TurnEvent::TurnEnd` in `stream.rs`. Add fields:

```rust
TurnEnd {
    // ... existing fields ...
    ttft: Option<Duration>,
    tbt: Option<Duration>,
},
```

Update all construction sites and pattern matches to include the new fields. The consumer in `caliban/src/tui.rs` will be updated in the next task.

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-agent-core turn_timing`
Expected: 3 tests pass. (Sleep-based tests are slow but reliable enough; this is one of the few legitimate use cases.)

Run: `cargo build --workspace`
Expected: TUI may fail to compile if it pattern-matched `TurnEnd` exhaustively. Patch the match arms in `caliban/src/tui.rs` to bind or ignore the new fields:

```rust
TurnEvent::TurnEnd { ttft, tbt, .. } => {
    // existing handling, plus: stash ttft for the next UsageSummary
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/stream.rs
git commit -m "feat(agent-core): per-turn TTFT/TBT capture + tracing event"
```

---

## Task 9: TTFT in UsageSummary line

**Files:**
- Modify: `caliban/src/tui.rs`

- [ ] **Step 1: Track the most recent turn's TTFT in App**

Add a field to `App`:

```rust
/// Most recent turn's TTFT in milliseconds, populated on each TurnEnd
/// and consumed when building the UsageSummary line at RunEnd.
pub(crate) last_turn_ttft_ms: Option<u64>,
```

Initialize to `None` in `App::new`. Reset to `None` on `/clear` (find the existing `clear_in_memory_history` or equivalent function).

- [ ] **Step 2: Capture TTFT on TurnEnd**

In the existing `TurnEvent::TurnEnd { .. }` arm in `tui.rs` (the one updated in Task 8 Step 4), capture:

```rust
TurnEvent::TurnEnd { ttft, .. } => {
    if let Some(ttft) = ttft {
        let millis = u64::try_from(ttft.as_millis()).unwrap_or(u64::MAX);
        app.last_turn_ttft_ms = Some(millis);
    }
    // existing handling...
}
```

- [ ] **Step 3: Populate the UsageSummary field at RunEnd**

In the `TurnEvent::RunEnd { .. }` arm where `TranscriptLine::UsageSummary { ... }` is constructed, replace `last_turn_ttft_ms: None` (set as a placeholder in Task 7) with the actual value:

```rust
last_turn_ttft_ms: app.last_turn_ttft_ms,
```

Reset `app.last_turn_ttft_ms = None;` after pushing the summary so a subsequent `/clear` + new run starts fresh.

(The render arm already shows the TTFT line — wired in Task 7 Step 4.)

- [ ] **Step 4: Build + test**

Run: `cargo build --workspace && cargo test --workspace`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui.rs
git commit -m "feat(tui): show last turn's TTFT in the per-run UsageSummary line"
```

---

## Task 10: End-to-end token-accuracy verification

This task verifies that token usage (input_tokens, output_tokens, cache_read_input_tokens, cache_creation_input_tokens) flows correctly from the provider response through every aggregation step to the UsageSummary the user sees.

**Files:** none modified — this is verification, not implementation.

- [ ] **Step 1: Trace the pipeline by reading code**

For each of the four token counters, identify and document the exact transform/aggregation site in:

1. Provider parser (where the field is extracted from wire JSON): e.g., `crates/caliban-provider-anthropic/src/stream_parse.rs:100` for `cache_creation_input_tokens`.
2. Provider IR converter (where it's mapped into `caliban_provider::Usage`): e.g., `crates/caliban-provider-anthropic/src/ir_convert.rs:187`.
3. Per-turn accumulator inside `stream.rs` (`acc.usage.merge(u)`, line ~565).
4. Per-run accumulator (`total_usage.merge(turn_usage)`, line ~660 or ~674).
5. RunEnd payload construction.
6. UsageSummary transcript-line construction (`tui.rs:1293`).
7. UsageSummary render arm (`tui.rs:729`).

Write a short Markdown doc at `docs/verification/2026-05-23-token-accuracy.md` listing each counter and the file:line for each of the 7 hops. If any field is dropped or zeroed at one of the hops, **call it out as a bug** and fix it before continuing.

- [ ] **Step 2: Confirm `Usage::merge` is the only aggregator**

Run: `grep -rn "input_tokens\s*+=\|output_tokens\s*+=" crates/`
Expected: no manual aggregation outside `crates/caliban-provider/src/response.rs:60-77` (the `merge` impl). If any other site adds tokens manually, it's almost certainly skipping cache fields. Document or fix.

- [ ] **Step 3: Behavior test — sum across two turns**

Add to `crates/caliban-agent-core/tests/cache.rs` (or a new test file):

```rust
#[tokio::test]
async fn token_usage_aggregates_across_turns() {
    let server = MockServer::start().await;

    // First turn: 10 input (5 cache_creation), 3 output.
    // Second turn: 7 input (5 cache_read), 4 output.
    let bodies = vec![
        serde_json::json!({
            "id": "m1", "type": "message", "role": "assistant",
            "content": [{"type": "tool_use", "id": "t1", "name": "a", "input": {}}],
            "model": "claude-test",
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 3, "cache_creation_input_tokens": 5}
        }),
        serde_json::json!({
            "id": "m2", "type": "message", "role": "assistant",
            "content": [{"type": "text", "text": "done"}],
            "model": "claude-test",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 7, "output_tokens": 4, "cache_read_input_tokens": 5}
        }),
    ];
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter_c = Arc::clone(&counter);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(move |_req: &Request| {
            let i = counter_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_json(bodies[i.min(bodies.len() - 1)].clone())
        })
        .mount(&server)
        .await;

    let cfg = DirectConfig::builder()
        .api_key(secrecy::SecretString::new("test".into()))
        .base_url(server.uri().parse().unwrap())
        .build()
        .unwrap();
    let provider: Arc<dyn Provider + Send + Sync> =
        Arc::new(AnthropicProvider::direct(cfg).unwrap());
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(Dummy { name: "a".into() }));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider)
            .tools(registry)
            .model("claude-test")
            .max_tokens(64)
            .prompt_cache(true)
            .build()
            .unwrap(),
    );
    let mut session = caliban_agent_core::Session::new(agent);
    session.system("sys").user_text("go");
    let new_msgs = session.run().await.unwrap();

    let total = session.total_usage();
    assert_eq!(total.input_tokens, 17, "input_tokens did not aggregate");
    assert_eq!(total.output_tokens, 7, "output_tokens did not aggregate");
    assert_eq!(
        total.cache_creation_input_tokens,
        Some(5),
        "cache_creation_input_tokens did not aggregate"
    );
    assert_eq!(
        total.cache_read_input_tokens,
        Some(5),
        "cache_read_input_tokens did not aggregate"
    );
    let _ = new_msgs;
}
```

(Adapt API names where they differ. The point: send two known-usage responses to the same agent in one run and assert all four counters aggregate.)

- [ ] **Step 4: Run all tests**

Run: `cargo test --workspace`
Expected: all green, including the new aggregation test.

- [ ] **Step 5: Manual end-to-end smoke**

Launch caliban interactively with a real Anthropic key:

```bash
ANTHROPIC_API_KEY=$KEY cargo run -- --debug
```

Send one short prompt. The TUI UsageSummary line should appear with `X↑ (C cache write) Y↓ tokens · TTFT XXXms`. Send a second prompt. The new UsageSummary should show `(R cached)`.

In a second terminal: `tail -f ~/Library/Caches/caliban/debug.log` (or the Linux equivalent). Confirm `caliban::cache` and `caliban::timing` events fire on each turn with non-zero values.

- [ ] **Step 6: Commit verification doc and aggregation test**

```bash
git add docs/verification/ crates/caliban-agent-core/tests/cache.rs
git commit -m "verify: end-to-end token aggregation + add per-counter trace doc"
```

---

## Task 11: Open the PR

- [ ] **Step 1: Push the branch**

```bash
git push -u origin jf/feat/perf/baseline
```

- [ ] **Step 2: Open the PR**

```bash
gh pr create --title "perf: baseline (Anthropic caching, HTTP/2, hickory, TTFT)" --body "$(cat <<'EOF'
## Summary

Closes the highest-value gaps from a recent performance audit. Five focused changes:

- **Anthropic prompt caching enabled.** `cache_control: Ephemeral` on the last system text block and last tool def. Default-on; `--no-prompt-cache` to disable. 90% discount on cached input tokens + faster TTFT after turn 1.
- **OpenAI cache hits visible.** Already auto-cached + extracted; surfaced in the TUI footer and as a `tracing::info!` event.
- **HTTP/2 + hickory-dns** on workspace `reqwest`. Connection multiplexing + async DNS with caching.
- **TTFT / TBT measurement.** Per-turn capture; tracing event + the most recent turn's TTFT in the per-run UsageSummary line.
- **`tokio::fs`** for two `std::fs` calls in `main.rs` debug-log startup.

## Verified

End-to-end token accounting traced from provider response → IR Usage → per-turn merge → per-run merge → RunEnd payload → UsageSummary transcript line → render. See `docs/verification/2026-05-23-token-accuracy.md`. New regression test (`crates/caliban-agent-core/tests/cache.rs::token_usage_aggregates_across_turns`) asserts that all four token counters (input, output, cache_creation, cache_read) sum correctly across two turns.

## Deferred

- Google Gemini context caching — separate `cachedContents` resource API; not a "simple addition."
- Parallel tool execution (ADR 0009 follow-up; needs its own design).
- Pre-dispatch of tool calls mid-stream (modest complexity).
- Hedged requests, circuit breakers (await router subsystem).

## Test plan

- [ ] `cargo test --workspace` clean (incl. new tests in `cache.rs`)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --all -- --check` clean
- [ ] Manual smoke: one Anthropic turn shows cache write counts in UsageSummary; second turn shows cache read; TTFT visible.
- [ ] Manual smoke: `caliban --no-prompt-cache` produces wire JSON without any `cache_control` markers (verified via debug logs or wiremock test).
EOF
)"
```

- [ ] **Step 3: Verify CI passes**

Run: `gh run watch --exit-status`
Expected: green. If clippy nitpicks something on 1.95 we missed, fix and push.

---

## Self-review

**Spec coverage:**

- ✅ Anthropic `cache_control` on system + last tool: Tasks 3, 4, 5.
- ✅ OpenAI cache hits in TUI + tracing: Task 7.
- ✅ HTTP/2 + hickory-dns: Task 1.
- ✅ TTFT/TBT capture + tracing + TUI: Tasks 8, 9.
- ✅ `tokio::fs` cleanup: Task 2.
- ✅ Gemini deferred (TODO comment): Task 2.
- ✅ Token-accuracy verification: Task 10.
- ✅ PR opened: Task 11.

**Placeholder scan:** No "TBD"/"TODO"/"placeholder" in plan body except the legitimate TODO comment being added to Gemini code. Every step has real code.

**Type consistency:**
- `TurnTiming` fields and methods consistent across Tasks 8, 9, 10.
- `TranscriptLine::UsageSummary` extended in Task 7; new fields populated in Task 9; rendered in Task 7. Match.
- `apply_prompt_cache(&mut [Message], &mut [ToolDef])` signature consistent in Tasks 3, 4.
- `AgentBuilder::prompt_cache(bool)` consistent in Tasks 4, 5, 6.
