# TUI slash & UX polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the stub slash commands (`/cost`, `/doctor`, `/effort`, `/model`, `/resume`, `/context`) with real implementations; add `/export`; fix the `/clear` context-tracker bug; extend the permission modal with always-allow/reject; add custom statusline command support.

**Architecture:** Most work lives in `caliban/src/tui/slash/` — one file per command, following the existing `SlashCommand` trait shape. `/effort` adds a typed enum + `ArcSwap` shared state on `AgentConfig`, plumbed through OpenAI and Anthropic provider request builders. Custom statusline is a new tiny module in `caliban-settings` that spawns the configured command after each `TurnEnd`. Permission modal gains 4 buttons via state-machine extension; "Always" branches add to a session-scoped `RuntimeRule` store. `/doctor` reuses existing health-check surfaces (settings, MCP client, sandbox, stores).

**Tech Stack:** Rust 1.85.0 (edition 2024), `tokio`, `arc-swap`, `serde_json`, `crossterm` (TUI), `directories`, `clipboard` or `arboard` (clipboard), `rust_decimal` (cost formatting).

**Spec:** [`docs/superpowers/specs/2026-05-26-tui-slash-ux-design.md`](../specs/2026-05-26-tui-slash-ux-design.md)

---

## File Structure

```
caliban/src/tui/slash/
├── basic.rs         MODIFY: ClearCommand resets context_window
├── cost.rs          CREATE: CostCommand overlay
├── doctor.rs        MODIFY: real health checks
├── effort.rs        MODIFY: real ArcSwap writeback + parsing
├── model.rs         MODIFY: real runtime swap + picker (ModelCommand only; siblings unchanged)
├── resume.rs        MODIFY: picker overlay + in-place swap
├── context.rs       MODIFY: stacked-bar + top-N visualization
└── export.rs        CREATE: ExportCommand markdown/json/clipboard

caliban/src/tui/
├── ask.rs           MODIFY: 4-button permission modal
├── overlay.rs       MODIFY: add Cost / Resume / Context / Export overlay variants

caliban/src/main.rs  MODIFY: add `doctor` subcommand

crates/caliban-agent-core/src/
├── config.rs        MODIFY: add `effort: Arc<ArcSwap<Effort>>`
├── agent.rs         MODIFY: add `active_model: Arc<ArcSwap<String>>`, `active_model()`, `try_swap_model()`
└── permissions.rs   MODIFY: add `RuntimeRule` + add_runtime_rule

crates/caliban-provider-openai/src/ir_convert.rs    MODIFY: read effort → reasoning.effort
crates/caliban-provider-anthropic/src/ir_convert.rs MODIFY: read effort → thinking.budget_tokens

crates/caliban-settings/src/
├── schema.json      MODIFY: + statusLine, + tui.showCostInStatusline, + effort
├── lib.rs           MODIFY: parse + propagate
└── statusline.rs    CREATE: StatuslineRunner

caliban/src/tui/
├── app.rs           MODIFY: hold StatuslineRunner; refresh after TurnEnd
└── render.rs        MODIFY: prefix statusline with runner's last value
```

---

## Task 1: `/clear` context-window reset (one-line fix)

**Files:**
- Modify: `caliban/src/tui/slash/basic.rs:25-33`
- Test: `caliban/src/tui/slash/basic.rs` (inline)

- [ ] **Step 1: Write the failing test**

In `caliban/src/tui/slash/basic.rs`, add:

```rust
#[cfg(test)]
mod clear_tests {
    use super::*;
    use crate::tui::app::App;
    use caliban_provider::Message;

    #[tokio::test]
    async fn clear_resets_context_window() {
        let mut app = App::for_tests();  // existing or new test helper
        app.context_window.record_history(&[Message::user_text(&"x".repeat(20_000))]);
        let used_before = app.context_window.utilization();
        assert!(used_before > 0.0, "precondition");
        let mut ctx = app.slash_ctx_for_tests();
        ClearCommand.execute("", &mut ctx).await.unwrap();
        assert_eq!(app.context_window.utilization(), 0.0);
    }
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p caliban clear_tests`
Expected: FAIL.

- [ ] **Step 3: Add the reset**

```rust
async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
    ctx.app.transcript.clear();
    ctx.app.messages.clear();
    ctx.app.last_turn_ttft_ms = None;
    if let Some(sess) = ctx.app.session.as_mut() { sess.messages.clear(); }
    ctx.app.context_window.record_history(&[]);   // ← NEW
    Ok(SlashOutcome::Continue)
}
```

- [ ] **Step 4: Run test**

Run: `cargo test -p caliban clear_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/slash/basic.rs
git commit -m "fix(tui): reset context_window tracker on /clear"
```

---

## Task 2: `Effort` enum + `AgentConfig.effort`

**Files:**
- Modify: `crates/caliban-agent-core/src/config.rs`
- Modify: `crates/caliban-agent-core/src/lib.rs` (re-export)
- Test: `crates/caliban-agent-core/src/config.rs` (inline)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod effort_tests {
    use super::*;

    #[test]
    fn openai_mapping() {
        assert_eq!(Effort::Low.as_openai(),    Some("low"));
        assert_eq!(Effort::Medium.as_openai(), Some("medium"));
        assert_eq!(Effort::High.as_openai(),   Some("high"));
        assert_eq!(Effort::Max.as_openai(),    Some("high"));
        assert_eq!(Effort::Auto.as_openai(),   None);
    }
    #[test]
    fn anthropic_budget_mapping() {
        assert_eq!(Effort::Low.as_anthropic_budget(),    Some(2_048));
        assert_eq!(Effort::Medium.as_anthropic_budget(), Some(8_192));
        assert_eq!(Effort::High.as_anthropic_budget(),   Some(24_576));
        assert_eq!(Effort::Max.as_anthropic_budget(),    Some(64_000));
        assert_eq!(Effort::Auto.as_anthropic_budget(),   None);
    }
    #[test]
    fn config_default_effort_is_auto() {
        let cfg = AgentConfig::default();
        assert_eq!(*cfg.effort.load_full(), Effort::Auto);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p caliban-agent-core effort_tests`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
use arc_swap::ArcSwap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Max,
    Auto,
}

impl Effort {
    #[must_use]
    pub fn as_openai(self) -> Option<&'static str> {
        match self {
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High | Self::Max => Some("high"),
            Self::Auto => None,
        }
    }
    #[must_use]
    pub fn as_anthropic_budget(self) -> Option<u32> {
        match self {
            Self::Low => Some(2_048),
            Self::Medium => Some(8_192),
            Self::High => Some(24_576),
            Self::Max => Some(64_000),
            Self::Auto => None,
        }
    }
}

// In AgentConfig:
pub effort: Arc<ArcSwap<Effort>>,

// In Default:
effort: Arc::new(ArcSwap::from_pointee(Effort::Auto)),
```

Re-export from `crates/caliban-agent-core/src/lib.rs`:

```rust
pub use config::Effort;
```

Add `arc-swap = "1"` to `crates/caliban-agent-core/Cargo.toml` if not already present.

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-agent-core effort_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-agent-core/src/config.rs crates/caliban-agent-core/src/lib.rs crates/caliban-agent-core/Cargo.toml
git commit -m "feat(agent-core): add Effort enum + AgentConfig.effort (ArcSwap)"
```

---

## Task 3: `/effort` slash command

**Files:**
- Modify: `caliban/src/tui/slash/effort.rs`
- Test: `caliban/src/tui/slash/effort.rs` (inline)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::App;
    use caliban_agent_core::Effort;

    #[tokio::test]
    async fn effort_low_updates_shared_state() {
        let mut app = App::for_tests();
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = EffortCommand.execute("low", &mut ctx).await.unwrap();
        assert!(matches!(outcome, SlashOutcome::StatusMessage(_)));
        assert_eq!(*ctx.app.agent_config.effort.load_full(), Effort::Low);
    }

    #[tokio::test]
    async fn effort_invalid_returns_error_message() {
        let mut app = App::for_tests();
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = EffortCommand.execute("turbo", &mut ctx).await.unwrap();
        match outcome {
            SlashOutcome::StatusMessage(s) => assert!(s.contains("expected low|medium|high|max|auto")),
            _ => panic!("unexpected outcome"),
        }
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p caliban effort::tests`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
//! `/effort <level>` — adjusts reasoning effort at runtime.

use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use caliban_agent_core::Effort;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};

pub(crate) struct EffortCommand;

#[async_trait]
impl SlashCommand for EffortCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/effort",
            description: "set reasoning effort (low|medium|high|max|auto)",
            args_hint: "<level>",
            hidden: false,
        }
    }
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let level = match args.trim().to_ascii_lowercase().as_str() {
            "low"    => Effort::Low,
            "medium" => Effort::Medium,
            "high"   => Effort::High,
            "max"    => Effort::Max,
            "auto"   => Effort::Auto,
            other    => return Ok(SlashOutcome::StatusMessage(
                format!("/effort: unknown level `{other}` (expected low|medium|high|max|auto)")
            )),
        };
        ctx.app.agent_config.effort.store(Arc::new(level));
        Ok(SlashOutcome::StatusMessage(format!("effort \u{2192} {level:?}")))
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(EffortCommand));
}
```

(If `App` doesn't hold a reference to `agent_config`, plumb one through — usually a single `Arc<AgentConfig>` field on `App` is the right shape, mirroring how `permission_mode` is exposed.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban effort::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/slash/effort.rs caliban/src/tui/app.rs caliban/src/tui/slash/mod.rs
git commit -m "feat(tui): /effort sets reasoning effort at runtime via ArcSwap"
```

---

## Task 4: Provider plumbing — OpenAI `reasoning.effort`

**Files:**
- Modify: `crates/caliban-provider-openai/src/ir_convert.rs`
- Test: `crates/caliban-provider-openai/src/ir_convert.rs` (inline or new test file)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod effort_plumbing {
    use super::*;
    use caliban_agent_core::Effort;
    // (assume the existing ir_convert tests have a fixture builder)

    #[test]
    fn effort_low_sets_reasoning_effort_low() {
        let mut req = build_test_request();
        req.effort = Some(Effort::Low);   // or however the IR carries it
        let json = ir_to_native(&req).unwrap();
        assert_eq!(json["reasoning"]["effort"], "low");
    }

    #[test]
    fn effort_auto_omits_reasoning_field() {
        let mut req = build_test_request();
        req.effort = Some(Effort::Auto);
        let json = ir_to_native(&req).unwrap();
        assert!(json.get("reasoning").is_none(), "reasoning field omitted on Auto");
    }
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p caliban-provider-openai effort_plumbing`
Expected: FAIL.

- [ ] **Step 3: Wire through the request build**

In `caliban-provider/src/request.rs`, add to `CompletionRequest`:

```rust
pub effort: Option<caliban_agent_core::Effort>,
```

…wait — that creates a cyclic dependency. Better: define `Effort` in `caliban-provider` instead, since it's just a small enum, and re-export it from `caliban-agent-core`. **Reverse Tasks 2's home:** move `Effort` to `caliban-provider/src/effort.rs`, keep the `as_openai`/`as_anthropic_budget` impls, then `AgentConfig.effort: Arc<ArcSwap<caliban_provider::Effort>>`.

(If the cycle was already considered when Task 2 was implemented, skip this re-home note.)

In `crates/caliban-provider-openai/src/ir_convert.rs`, where the native request is assembled:

```rust
if let Some(level) = req.effort.and_then(|e| e.as_openai()) {
    json["reasoning"] = serde_json::json!({ "effort": level });
}
```

- [ ] **Step 4: Plumb the value in the agent loop**

In `crates/caliban-agent-core/src/stream/mod.rs`, where the per-turn `CompletionRequest` is built, copy `self.config.effort.load_full()` into `request.effort`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p caliban-provider-openai effort_plumbing`
Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-provider/src/effort.rs crates/caliban-provider/src/lib.rs crates/caliban-provider/src/request.rs \
        crates/caliban-provider-openai/src/ir_convert.rs crates/caliban-agent-core/src/{config,stream/mod}.rs
git commit -m "feat(provider-openai): emit reasoning.effort from AgentConfig.effort"
```

---

## Task 5: Provider plumbing — Anthropic `thinking.budget_tokens`

**Files:**
- Modify: `crates/caliban-provider-anthropic/src/ir_convert.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn effort_low_sets_thinking_budget_2048() {
    let mut req = build_test_request();
    req.effort = Some(caliban_provider::Effort::Low);
    let json = ir_to_native(&req).unwrap();
    assert_eq!(json["thinking"]["budget_tokens"], 2048);
}
#[test]
fn effort_auto_omits_thinking_field() {
    let mut req = build_test_request();
    req.effort = Some(caliban_provider::Effort::Auto);
    let json = ir_to_native(&req).unwrap();
    assert!(json.get("thinking").is_none());
}
```

- [ ] **Step 2: Run test** → FAIL.

- [ ] **Step 3: Implement**

```rust
if let Some(budget) = req.effort.and_then(|e| e.as_anthropic_budget()) {
    json["thinking"] = serde_json::json!({ "type": "enabled", "budget_tokens": budget });
}
```

- [ ] **Step 4: Run tests** → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-provider-anthropic/src/ir_convert.rs
git commit -m "feat(provider-anthropic): emit thinking.budget_tokens from AgentConfig.effort"
```

---

## Task 5a: `Agent::active_model` + `try_swap_model`

**Files:**
- Modify: `crates/caliban-agent-core/src/agent.rs`
- Modify: `crates/caliban-agent-core/src/stream/mod.rs` (read `active_model()` in the 3 hot sites)
- Modify: `crates/caliban-agent-core/src/lib.rs` (re-export `ModelSwapError`)
- Test: `crates/caliban-agent-core/tests/model_swap.rs` (new)

- [ ] **Step 1: Write the failing test**

```rust
#![cfg(feature = "mock")]

use caliban_agent_core::{Agent, AgentConfig, ModelSwapError};
use caliban_provider::MockProvider;
use std::sync::Arc;

#[test]
fn active_model_starts_from_config_model() {
    let cfg = AgentConfig { model: "model-A".into(), ..Default::default() };
    let agent = Agent::new(Arc::new(MockProvider::for_tests()), cfg).unwrap();
    assert_eq!(agent.active_model().as_str(), "model-A");
}

#[test]
fn try_swap_model_same_provider_succeeds() {
    let cfg = AgentConfig { model: "model-A".into(), ..Default::default() };
    let provider = MockProvider::for_tests_with_models(&["model-A", "model-B"]);
    let agent = Agent::new(Arc::new(provider), cfg).unwrap();
    agent.try_swap_model("model-B").expect("same-provider swap should succeed");
    assert_eq!(agent.active_model().as_str(), "model-B");
}

#[test]
fn try_swap_model_unsupported_returns_error() {
    let cfg = AgentConfig { model: "model-A".into(), ..Default::default() };
    let provider = MockProvider::for_tests_with_models(&["model-A"]);
    let agent = Agent::new(Arc::new(provider), cfg).unwrap();
    let err = agent.try_swap_model("never-heard-of-it").unwrap_err();
    assert!(matches!(err, ModelSwapError::UnsupportedByProvider(ref s) if s == "never-heard-of-it"));
    assert_eq!(agent.active_model().as_str(), "model-A");
}
```

(If `MockProvider::for_tests_with_models(&[…])` doesn't exist, add a one-line helper that overrides `capabilities()` to return `unknown: false` for any string in the list and `unknown: true` otherwise.)

- [ ] **Step 2: Run tests**

Run: `cargo test -p caliban-agent-core --features mock --test model_swap`
Expected: FAIL — `active_model`, `try_swap_model`, and `ModelSwapError` don't exist.

- [ ] **Step 3: Add `active_model` field + helpers**

In `crates/caliban-agent-core/src/agent.rs`:

```rust
use arc_swap::ArcSwap;

pub struct Agent {
    pub(crate) provider: Arc<dyn Provider + Send + Sync>,
    pub(crate) tools: ToolRegistry,
    pub(crate) config: AgentConfig,
    pub(crate) active_model: Arc<ArcSwap<String>>,
    // …existing fields…
}

#[derive(Debug, thiserror::Error, Clone)]
pub enum ModelSwapError {
    #[error("model `{0}` is not available on the active provider")]
    UnsupportedByProvider(String),
    #[error("model `{0}` requires provider `{1}`, but active provider is `{2}`; restart with --provider {1}")]
    CrossProvider(String, String, String),
}

impl Agent {
    /// Current model id (lock-free snapshot).
    #[must_use]
    pub fn active_model(&self) -> Arc<String> { self.active_model.load_full() }

    /// Swap the active model. Same-provider only.
    pub fn try_swap_model(&self, new_model: &str) -> Result<(), ModelSwapError> {
        let caps = self.provider.capabilities(new_model);
        if caps.unknown {
            return Err(ModelSwapError::UnsupportedByProvider(new_model.to_string()));
        }
        self.active_model.store(Arc::new(new_model.to_string()));
        Ok(())
    }
}
```

In `Agent::new` (or wherever the struct is constructed), initialise:

```rust
active_model: Arc::new(ArcSwap::from_pointee(config.model.clone())),
```

Re-export from `crates/caliban-agent-core/src/lib.rs`:

```rust
pub use agent::{Agent, AgentConfig, ModelSwapError};
```

- [ ] **Step 4: Update the three hot read sites in the turn loop**

In `crates/caliban-agent-core/src/stream/mod.rs`:

- Line 255 (`#[instrument(... fields(model = %self.config.model, ...))]`): change to `model = %self.active_model()`. Note that `tracing` displays via `Display`; an `Arc<String>` derefs to `&str` which formats fine.
- Line 320 (`self.provider.capabilities(&self.config.model)`): change to `self.provider.capabilities(self.active_model().as_str())`.
- Line 376 (`model: self.config.model.clone()`): change to `model: self.active_model().as_str().to_string()`.

For each, audit the surrounding ~5 lines to make sure no lifetime issue results from dropping the `Arc<String>` temporary mid-expression — if any does, bind it: `let m = self.active_model(); …use m.as_str()…`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p caliban-agent-core --features mock --test model_swap`
Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-agent-core/src/agent.rs crates/caliban-agent-core/src/stream/mod.rs \
        crates/caliban-agent-core/src/lib.rs crates/caliban-agent-core/tests/model_swap.rs \
        crates/caliban-provider/src/mock.rs
git commit -m "feat(agent-core): Agent::active_model + try_swap_model (same-provider hot swap)"
```

---

## Task 5b: `/model` slash command + picker overlay

**Files:**
- Modify: `caliban/src/tui/slash/model.rs` (replace the display-only stub)
- Modify: `caliban/src/tui/overlay.rs` (`ModelPicker(ModelPickerState)`)
- Modify: `caliban/src/tui/events.rs` (handle picker key events)
- Modify: `caliban/src/tui/render.rs` (render picker)
- Test: `caliban/src/tui/slash/model.rs` (inline)

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::App;

    #[tokio::test]
    async fn model_with_id_swaps_active_model() {
        let mut app = App::for_tests_with_models(&["model-A", "model-B"]);
        assert_eq!(app.agent.active_model().as_str(), "model-A");
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = ModelCommand.execute("model-B", &mut ctx).await.unwrap();
        match outcome {
            SlashOutcome::StatusMessage(s) => assert!(s.contains("model-B")),
            _ => panic!(),
        }
        assert_eq!(app.agent.active_model().as_str(), "model-B");
    }

    #[tokio::test]
    async fn model_with_no_args_opens_picker() {
        let mut app = App::for_tests_with_models(&["model-A", "model-B"]);
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = ModelCommand.execute("", &mut ctx).await.unwrap();
        assert!(matches!(outcome, SlashOutcome::Overlay(crate::tui::Overlay::ModelPicker(_))));
    }

    #[tokio::test]
    async fn model_with_unknown_id_reports_error_and_leaves_active_unchanged() {
        let mut app = App::for_tests_with_models(&["model-A"]);
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = ModelCommand.execute("nope", &mut ctx).await.unwrap();
        match outcome {
            SlashOutcome::StatusMessage(s) => assert!(s.contains("not available")),
            _ => panic!(),
        }
        assert_eq!(app.agent.active_model().as_str(), "model-A");
    }

    #[test]
    fn picker_marks_cross_provider_rows_non_selectable() {
        let rows = vec![
            ModelPickerRow { id: "a".into(), label: "A".into(), provider: "anthropic".into(), selectable: true, caps_summary: "".into(), cost_hint: None },
            ModelPickerRow { id: "b".into(), label: "B".into(), provider: "openai".into(),    selectable: false, caps_summary: "".into(), cost_hint: None },
        ];
        let state = ModelPickerState::new(rows, /*active_provider=*/ "anthropic");
        let visible_selectable = state.rows.iter().filter(|r| r.selectable).count();
        assert_eq!(visible_selectable, 1);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p caliban model::tests`
Expected: FAIL.

- [ ] **Step 3: Rewrite `model.rs`**

Replace the existing `ModelCommand::execute` and add the picker state. The other commands in this file (`EffortCommand`, `StatusCommand`, `LoginCommand`, `LogoutCommand`, `SetupTokenCommand`) are untouched here — `EffortCommand` was already rewritten in Task 3.

```rust
//! `/model [id|--picker]` — show or switch the active model.

use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::Overlay;

pub(crate) struct ModelCommand;

#[derive(Debug, Clone)]
pub struct ModelPickerRow {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub selectable: bool,
    pub caps_summary: String,
    pub cost_hint: Option<String>,
}

#[derive(Debug)]
pub struct ModelPickerState {
    pub rows: Vec<ModelPickerRow>,
    pub active_provider: String,
    pub filter: String,
    pub selection: usize,
}

impl ModelPickerState {
    pub fn new(rows: Vec<ModelPickerRow>, active_provider: &str) -> Self {
        Self { rows, active_provider: active_provider.to_string(), filter: String::new(), selection: 0 }
    }
    pub fn visible(&self) -> Vec<&ModelPickerRow> {
        if self.filter.is_empty() { return self.rows.iter().collect(); }
        let f = self.filter.to_ascii_lowercase();
        self.rows.iter().filter(|r| {
            r.id.to_ascii_lowercase().contains(&f)
                || r.label.to_ascii_lowercase().contains(&f)
                || r.provider.to_ascii_lowercase().contains(&f)
                || r.caps_summary.to_ascii_lowercase().contains(&f)
        }).collect()
    }
}

pub(crate) fn build_picker_rows(app: &crate::tui::app::App) -> Vec<ModelPickerRow> {
    let active_provider = app.agent.provider().name().to_string();
    let mut rows: Vec<ModelPickerRow> = if let Some(router) = app.router.as_ref() {
        router.routes().iter().map(|r| {
            let provider = r.provider.clone();
            let selectable = provider == active_provider;
            ModelPickerRow {
                id: r.model.clone(),
                label: r.label.clone().unwrap_or_else(|| r.model.clone()),
                provider,
                selectable,
                caps_summary: format!("{} ctx", r.max_input_tokens.unwrap_or(0)),
                cost_hint: None,
            }
        }).collect()
    } else {
        // Fallback: ask the active provider for its known models.
        app.agent.provider().known_models().iter().map(|m| ModelPickerRow {
            id: m.id.clone(),
            label: m.id.clone(),
            provider: active_provider.clone(),
            selectable: true,
            caps_summary: format!("{} ctx", m.max_input_tokens),
            cost_hint: None,
        }).collect()
    };
    rows.sort_by(|a, b| b.selectable.cmp(&a.selectable).then_with(|| a.id.cmp(&b.id)));
    rows
}

#[async_trait]
impl SlashCommand for ModelCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/model",
            description: "show or switch the active model",
            args_hint: "[id|--picker]",
            hidden: false,
        }
    }
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let trimmed = args.trim();
        if trimmed.is_empty() || trimmed == "--picker" {
            let rows = build_picker_rows(ctx.app);
            let provider = ctx.app.agent.provider().name().to_string();
            return Ok(SlashOutcome::Overlay(Overlay::ModelPicker(ModelPickerState::new(rows, &provider))));
        }
        match ctx.app.agent.try_swap_model(trimmed) {
            Ok(()) => {
                let caps = ctx.app.agent.provider().capabilities(trimmed);
                ctx.app.context_window.set_capacity(caps.max_input_tokens);
                Ok(SlashOutcome::StatusMessage(format!("model \u{2192} {trimmed}")))
            }
            Err(e) => Ok(SlashOutcome::StatusMessage(format!("/model: {e}"))),
        }
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(ModelCommand));
    // (sibling commands' registrations remain in the same file, untouched here)
}
```

(If `Provider::name()` and `Provider::known_models()` don't exist on the trait, add them as one-liners returning `&'static str` and `&[ModelInfo]` respectively — `ModelInfo` is already in `caliban-provider/src/capabilities.rs`.)

Add `Overlay::ModelPicker(ModelPickerState)` to `caliban/src/tui/overlay.rs`. Implement key handling in `events.rs` (Up/Down skip non-selectable by default; Shift modifier includes them; Enter calls `ctx.app.agent.try_swap_model(&row.id)` and prints the same status message; Esc closes).

For cross-provider attempts via Enter on a non-selectable row, surface the `ModelSwapError::CrossProvider` message. To produce that error, the slash code needs to know which provider each row uses. Easiest: in `events.rs` Enter handler, when `row.selectable == false`, format the message inline rather than calling `try_swap_model`:

```rust
let msg = format!(
    "/model: model `{}` requires provider `{}`, but active provider is `{}`; restart with --provider {}",
    row.id, row.provider, state.active_provider, row.provider
);
app.transcript.push(TranscriptLine::Info(msg));
```

Render in `render.rs`: a two-column table (id | provider | caps_summary), with non-selectable rows in `Style::default().add_modifier(Modifier::DIM)` and a `[needs restart]` suffix.

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban model::tests`
Expected: PASS.

- [ ] **Step 5: Manual smoke test**

```
cargo run --bin caliban
```

In the TUI:
1. Type `/model` — picker opens listing all configured routes.
2. Type a filter — list narrows.
3. Select a same-provider model — status message `"model → <id>"`, statusline updates immediately, next turn uses the new model.
4. Type `/model <bogus-id>` — status message `/model: model `<bogus-id>` is not available on the active provider`.

- [ ] **Step 6: Commit**

```bash
git add caliban/src/tui/slash/model.rs caliban/src/tui/overlay.rs caliban/src/tui/events.rs caliban/src/tui/render.rs \
        crates/caliban-provider/src/provider.rs crates/caliban-provider/src/lib.rs
git commit -m "feat(tui): /model runtime swap + picker overlay (same-provider in v1)"
```

---

## Task 6: `/cost` overlay

**Files:**
- Create: `caliban/src/tui/slash/cost.rs`
- Modify: `caliban/src/tui/overlay.rs` (add `Cost` variant)
- Modify: `caliban/src/tui/render.rs` (render the overlay)
- Modify: `caliban/src/tui/slash/mod.rs` (register)

- [ ] **Step 1: Write the failing test**

In `caliban/src/tui/slash/cost.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::App;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[tokio::test]
    async fn cost_command_opens_overlay() {
        let mut app = App::for_tests();
        app.cost_accumulator.record_usage("anthropic/claude-sonnet-4-6", /*usage=*/ Default::default(), Decimal::from_str("0.0123").unwrap());
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = CostCommand.execute("", &mut ctx).await.unwrap();
        assert!(matches!(outcome, SlashOutcome::Overlay(crate::tui::Overlay::Cost(_))));
    }
}
```

- [ ] **Step 2: Run test** → FAIL.

- [ ] **Step 3: Implement `CostCommand`**

```rust
//! `/cost` — open the per-model + total-USD breakdown overlay.

use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::Overlay;

pub(crate) struct CostCommand;

#[async_trait]
impl SlashCommand for CostCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/cost",
            description: "show cumulative cost and per-model breakdown",
            args_hint: "",
            hidden: false,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let summary = ctx.app.cost_accumulator.summary();
        Ok(SlashOutcome::Overlay(Overlay::Cost(summary)))
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(CostCommand));
}
```

Add `Cost(CostSummary)` variant to `Overlay` in `caliban/src/tui/overlay.rs` (or wherever the enum lives). Add a `render_cost_overlay(&CostSummary)` function in `render.rs` that emits a fixed-width table matching the spec's layout.

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban cost::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/slash/cost.rs caliban/src/tui/slash/mod.rs caliban/src/tui/overlay.rs caliban/src/tui/render.rs
git commit -m "feat(tui): /cost overlay with per-model breakdown"
```

---

## Task 7: `/doctor` real checks (and headless `caliban doctor`)

**Files:**
- Modify: `caliban/src/tui/slash/doctor.rs`
- Create: `caliban/src/diagnostics.rs` (shared between TUI and headless)
- Modify: `caliban/src/main.rs` (new subcommand)

- [ ] **Step 1: Write the failing test**

In `caliban/src/diagnostics.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn diagnostics_run_without_panicking() {
        let r = Diagnostics::run(&DiagOpts { deep: false }).await;
        // Every check should produce a result with a name and a status.
        for c in &r.checks {
            assert!(!c.name.is_empty());
        }
    }
}
```

- [ ] **Step 2: Run test** → FAIL.

- [ ] **Step 3: Implement diagnostics module**

```rust
//! Shared health-check runner for `/doctor` (TUI) and `caliban doctor` (headless).

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus { Pass, Warn, Fail }

#[derive(Debug, Clone)]
pub struct DiagCheck {
    pub name: &'static str,
    pub status: CheckStatus,
    pub hint: String,
}

#[derive(Debug, Default)]
pub struct DiagOpts { pub deep: bool }

#[derive(Debug, Default)]
pub struct Diagnostics { pub checks: Vec<DiagCheck> }

impl Diagnostics {
    pub async fn run(opts: &DiagOpts) -> Self {
        let mut out = Self::default();
        out.checks.push(check_settings().await);
        out.checks.push(check_mcp().await);
        out.checks.push(check_sandbox());
        out.checks.push(check_checkpoint_store());
        out.checks.push(check_session_store());
        out.checks.push(check_claudemd());
        if opts.deep {
            for c in check_providers().await { out.checks.push(c); }
        }
        out
    }
    pub fn exit_code(&self) -> i32 {
        if self.checks.iter().any(|c| c.status == CheckStatus::Fail) { 1 } else { 0 }
    }
}

async fn check_settings() -> DiagCheck { /* … */ }
async fn check_mcp() -> DiagCheck { /* … */ }
fn check_sandbox() -> DiagCheck { /* … */ }
fn check_checkpoint_store() -> DiagCheck { /* … */ }
fn check_session_store() -> DiagCheck { /* … */ }
fn check_claudemd() -> DiagCheck { /* … */ }
async fn check_providers() -> Vec<DiagCheck> { /* one cheap ping per configured provider */ }
```

Each `check_*` follows the same shape: probe → classify pass/warn/fail → format hint.

Update `caliban/src/tui/slash/doctor.rs`:

```rust
async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
    let deep = args.contains("--deep");
    let diag = crate::diagnostics::Diagnostics::run(&crate::diagnostics::DiagOpts { deep }).await;
    Ok(SlashOutcome::Overlay(Overlay::Doctor(diag)))
}
```

Add the headless entry point in `caliban/src/main.rs`:

```rust
// in match args {
//   …existing arms…
Some("doctor") => {
    let deep = std::env::args().any(|a| a == "--deep");
    let diag = crate::diagnostics::Diagnostics::run(&crate::diagnostics::DiagOpts { deep }).await;
    print_diagnostics_text(&diag);
    std::process::exit(diag.exit_code());
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban diagnostics::tests`
Expected: PASS.

- [ ] **Step 5: Manual smoke test**

```
cargo run --bin caliban -- doctor
```

Expected: a table with at least 6 rows, exit 0 if all pass/warn.

- [ ] **Step 6: Commit**

```bash
git add caliban/src/diagnostics.rs caliban/src/tui/slash/doctor.rs caliban/src/main.rs caliban/src/tui/overlay.rs caliban/src/tui/render.rs
git commit -m "feat(tui+cli): real /doctor checks; caliban doctor subcommand"
```

---

## Task 8: `/resume` picker

**Files:**
- Modify: `caliban/src/tui/slash/resume.rs`
- Modify: `caliban/src/tui/overlay.rs` (`Resume(ResumePickerState)`)
- Modify: `caliban/src/tui/events.rs` (handle picker key events)
- Modify: `caliban/src/tui/app.rs` (add `swap_session` helper)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::App;

    #[tokio::test]
    async fn resume_command_opens_picker_with_recent_sessions() {
        let mut app = App::for_tests_with_sessions(/*n=*/ 5);
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = ResumeCommand.execute("", &mut ctx).await.unwrap();
        match outcome {
            SlashOutcome::Overlay(crate::tui::Overlay::Resume(state)) => {
                assert_eq!(state.sessions.len(), 5);
            }
            _ => panic!("expected Resume overlay"),
        }
    }

    #[tokio::test]
    async fn picker_filter_narrows_list() {
        let mut state = ResumePickerState::new(vec![
            mk_session("foo bar"),
            mk_session("baz quux"),
            mk_session("foo baz"),
        ]);
        state.set_filter("foo");
        assert_eq!(state.visible().len(), 2);
    }
}
```

- [ ] **Step 2: Run tests** → FAIL.

- [ ] **Step 3: Implement**

```rust
//! `/resume [query]` — pick a prior session and swap into it without restart.

use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::Overlay;

pub(crate) struct ResumeCommand;

pub struct ResumePickerState {
    pub sessions: Vec<caliban_sessions::SessionSummary>,
    pub filter: String,
    pub selection: usize,
}

impl ResumePickerState {
    pub fn new(sessions: Vec<caliban_sessions::SessionSummary>) -> Self {
        Self { sessions, filter: String::new(), selection: 0 }
    }
    pub fn set_filter(&mut self, f: &str) {
        self.filter = f.to_string();
        self.selection = 0;
    }
    pub fn visible(&self) -> Vec<&caliban_sessions::SessionSummary> {
        if self.filter.is_empty() { self.sessions.iter().collect() }
        else { self.sessions.iter().filter(|s| s.matches_fuzzy(&self.filter)).collect() }
    }
}

#[async_trait]
impl SlashCommand for ResumeCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/resume",
            description: "resume a prior session via picker",
            args_hint: "[query]",
            hidden: false,
        }
    }
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let summaries = ctx.app.sessions_store.list_recent(50).await?;
        let mut state = ResumePickerState::new(summaries);
        if !args.trim().is_empty() { state.set_filter(args.trim()); }
        Ok(SlashOutcome::Overlay(Overlay::Resume(state)))
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(ResumeCommand));
}
```

In `caliban/src/tui/app.rs`, add:

```rust
impl App {
    pub async fn swap_session(&mut self, summary: caliban_sessions::SessionSummary) -> anyhow::Result<()> {
        // 1. Cancel in-flight run.
        if let Some(cancel) = self.run_cancel.take() { cancel.cancel(); }
        // 2. Load.
        let loaded = self.sessions_store.load(&summary.id).await?;
        // 3. Swap message/transcript/context.
        self.messages = loaded.messages.clone();
        self.transcript.replace(&loaded.messages);
        self.context_window.record_history(&loaded.messages);
        self.session = Some(loaded);
        self.last_turn_ttft_ms = None;
        Ok(())
    }
}
```

In `caliban/src/tui/events.rs`, add handlers for the picker's key events (Up/Down to move `selection`, Enter to call `swap_session`, Esc to close).

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban resume::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/slash/resume.rs caliban/src/tui/overlay.rs caliban/src/tui/events.rs caliban/src/tui/app.rs caliban/src/tui/render.rs
git commit -m "feat(tui): /resume picker overlay with in-place session swap"
```

---

## Task 9: `/context` visualization

**Files:**
- Modify: `caliban/src/tui/slash/context.rs`
- Modify: `caliban/src/tui/overlay.rs`
- Modify: `caliban/src/tui/render.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::Message;

    #[test]
    fn compute_breakdown_groups_by_message_kind() {
        let msgs = vec![
            Message::system_text(&"x".repeat(4000)),
            Message::user_text(&"y".repeat(8000)),
            Message::assistant_text(&"z".repeat(12_000)),
        ];
        let b = ContextBreakdown::compute(&msgs);
        assert!(b.system_tokens > 0);
        assert!(b.user_tokens > 0);
        assert!(b.assistant_tokens > 0);
        assert_eq!(b.tool_use_tokens, 0);
        assert_eq!(b.tool_result_tokens, 0);
    }

    #[test]
    fn top_n_returns_largest() {
        let msgs = vec![
            Message::system_text("small"),
            Message::user_text(&"x".repeat(10_000)),
        ];
        let top = ContextBreakdown::compute(&msgs).top_n(2);
        assert_eq!(top.len(), 2);
        assert!(top[0].chars >= top[1].chars);
    }
}
```

- [ ] **Step 2: Run tests** → FAIL.

- [ ] **Step 3: Implement**

```rust
//! `/context` — visualize the active context window's composition.

use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::Overlay;

pub(crate) struct ContextCommand;

#[derive(Debug, Default)]
pub struct ContextBreakdown {
    pub system_tokens: u32,
    pub user_tokens: u32,
    pub assistant_tokens: u32,
    pub tool_use_tokens: u32,
    pub tool_result_tokens: u32,
    pub blocks: Vec<BlockEntry>,
}

#[derive(Debug, Clone)]
pub struct BlockEntry {
    pub kind: &'static str,
    pub label: String,
    pub chars: usize,
}

impl ContextBreakdown {
    pub fn compute(messages: &[caliban_provider::Message]) -> Self { /* walk + accumulate per-kind + per-block */ Default::default() }
    pub fn top_n(&self, n: usize) -> Vec<&BlockEntry> {
        let mut by_size: Vec<&BlockEntry> = self.blocks.iter().collect();
        by_size.sort_by_key(|b| std::cmp::Reverse(b.chars));
        by_size.into_iter().take(n).collect()
    }
}

#[async_trait]
impl SlashCommand for ContextCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/context",
            description: "visualize the active context window's composition",
            args_hint: "",
            hidden: false,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let b = ContextBreakdown::compute(&ctx.app.messages);
        Ok(SlashOutcome::Overlay(Overlay::Context(b)))
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(ContextCommand));
}
```

Render: stacked bar (segments via colored block-char prints) + top-N list. The headless `caliban context --print` variant can wait for a follow-up — gate it behind a TODO comment if it would balloon scope.

- [ ] **Step 4: Run tests** → PASS.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/slash/context.rs caliban/src/tui/overlay.rs caliban/src/tui/render.rs
git commit -m "feat(tui): /context visualization (stacked-bar + top-N largest blocks)"
```

---

## Task 10: `/export`

**Files:**
- Create: `caliban/src/tui/slash/export.rs`
- Add: `caliban/Cargo.toml` clipboard crate (`arboard = "3"`)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::App;
    use tempfile::tempdir;

    #[tokio::test]
    async fn export_writes_markdown_file() {
        let mut app = App::for_tests();
        app.messages.push(caliban_provider::Message::user_text("hi"));
        app.messages.push(caliban_provider::Message::assistant_text("hello"));
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.md");
        let mut ctx = app.slash_ctx_for_tests();
        ExportCommand.execute(&path.to_string_lossy(), &mut ctx).await.unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("hi"));
        assert!(body.contains("hello"));
        assert!(body.starts_with("# caliban session"));
    }
}
```

- [ ] **Step 2: Run test** → FAIL.

- [ ] **Step 3: Implement**

```rust
//! `/export [path]` — write the session transcript to a file or clipboard.

use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};

pub(crate) struct ExportCommand;

#[async_trait]
impl SlashCommand for ExportCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/export",
            description: "export the session transcript to markdown (or clipboard with `-`)",
            args_hint: "[path|-] [--format json]",
            hidden: false,
        }
    }
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let format = if args.contains("--format json") { Format::Json } else { Format::Markdown };
        let body = render_session(ctx.app, format);
        let raw_path = args.split_whitespace().find(|t| !t.starts_with("--")).map(str::to_string);
        match raw_path.as_deref() {
            Some("-") => {
                arboard::Clipboard::new()?.set_text(body)?;
                Ok(SlashOutcome::StatusMessage("session copied to clipboard".into()))
            }
            Some(p) => {
                std::fs::write(p, body)?;
                Ok(SlashOutcome::StatusMessage(format!("exported to {p}")))
            }
            None => {
                let default = format!("caliban-session-{}.md", Utc::now().format("%Y-%m-%d"));
                std::fs::write(&default, body)?;
                Ok(SlashOutcome::StatusMessage(format!("exported to {default}")))
            }
        }
    }
}

enum Format { Markdown, Json }

fn render_session(app: &crate::tui::app::App, format: Format) -> String { /* write header + per-message sections */ }

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(ExportCommand));
}
```

- [ ] **Step 4: Run tests** → PASS.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/slash/export.rs caliban/src/tui/slash/mod.rs caliban/Cargo.toml
git commit -m "feat(tui): /export session to markdown/json/clipboard"
```

---

## Task 11: Permission modal 4-button extension

**Files:**
- Modify: `caliban/src/tui/ask.rs`
- Modify: `crates/caliban-agent-core/src/permissions.rs` (add `RuntimeRule`)
- Test: `caliban/src/tui/ask.rs` (inline)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod modal_tests {
    use super::*;

    #[test]
    fn bash_pattern_derives_first_token() {
        let p = derive_pattern("Bash", &serde_json::json!({"command": "gh pr view 42"}));
        assert_eq!(p, "Bash(gh *)");
    }

    #[test]
    fn read_pattern_uses_first_segment() {
        let p = derive_pattern("Read", &serde_json::json!({"file_path": "/home/me/proj/src/foo.rs"}));
        assert!(p.starts_with("Read(/home/me/proj/src/*"));
    }

    #[test]
    fn always_allow_inserts_runtime_rule() {
        let mut store = caliban_agent_core::permissions::RuleStore::new_empty();
        let modal = AskModal::for_test("Bash", &serde_json::json!({"command": "ls -l"}));
        modal.confirm_with(AskChoice::AlwaysAllow, &mut store).unwrap();
        // Subsequent matching invocation auto-allows.
        let outcome = store.evaluate("Bash", &serde_json::json!({"command": "ls -al"}));
        assert!(matches!(outcome, PermissionVerdict::Allow));
    }
}
```

- [ ] **Step 2: Run tests** → FAIL.

- [ ] **Step 3: Implement**

In `crates/caliban-agent-core/src/permissions.rs`, add:

```rust
/// Runtime-only rule added during a session via the "Always allow/reject"
/// modal action. Composes with config rules under existing precedence
/// (runtime > project > user > managed).
#[derive(Debug, Clone)]
pub struct RuntimeRule {
    pub pattern: String,
    pub verdict: PermissionVerdict,   // Allow or Deny
}

impl RuleStore {
    pub fn add_runtime_rule(&mut self, rule: RuntimeRule) {
        self.runtime_rules.push(rule);
    }
}
```

(Adjust the `RuleStore` definition to hold `runtime_rules: Vec<RuntimeRule>` and to consult them first in `evaluate`.)

In `caliban/src/tui/ask.rs`:

```rust
#[derive(Debug, Clone, Copy)]
pub enum AskChoice { AllowOnce, AlwaysAllow, RejectOnce, AlwaysReject }

pub fn derive_pattern(tool: &str, input: &serde_json::Value) -> String {
    match tool {
        "Bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let first = cmd.split_whitespace().next().unwrap_or("*");
            format!("Bash({first} *)")
        }
        "Edit" | "Read" | "Write" => {
            let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("/*");
            let dir = std::path::Path::new(path).parent()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "/".into());
            format!("{tool}({dir}/*)")
        }
        other if other.starts_with("mcp__") => format!("{other}(*)"),
        other => format!("{other}(*)"),
    }
}
```

Update the modal state machine to support 4 buttons. Existing 2-button code likely has an `Accept/Reject` enum; add the 2 new variants and route `AlwaysAllow`/`AlwaysReject` through `RuleStore::add_runtime_rule`.

Render: when the modal opens, show the derived pattern verbatim above the buttons (per the spec).

- [ ] **Step 4: Run tests** → PASS.

- [ ] **Step 5: Manual smoke test**

```
cargo run --bin caliban
```

Trigger any tool that would prompt. Verify all four buttons render and each works.

- [ ] **Step 6: Commit**

```bash
git add caliban/src/tui/ask.rs crates/caliban-agent-core/src/permissions.rs caliban/src/tui/render.rs caliban/src/tui/events.rs
git commit -m "feat(tui+permissions): 4-button Ask modal with runtime rule insertion"
```

---

## Task 12: Custom statusline command

**Files:**
- Create: `crates/caliban-settings/src/statusline.rs`
- Modify: `crates/caliban-settings/src/schema.json`
- Modify: `caliban/src/tui/app.rs` (hold runner, refresh on TurnEnd)
- Modify: `caliban/src/tui/render.rs` (prefix segment)
- Modify: `caliban/src/tui/events.rs` (call `refresh` from the TurnEnd/RunEnd handler)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    #[tokio::test]
    async fn runner_returns_script_stdout() {
        let dir = tempdir().unwrap();
        let script_path = dir.path().join("hello.sh");
        std::fs::write(&script_path, "#!/bin/sh\necho hello world").unwrap();
        std::fs::set_permissions(&script_path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let runner = StatuslineRunner::new(StatuslineConfig {
            command: script_path.to_string_lossy().to_string(),
            timeout_ms: 500, padding: 1,
        });
        let out = runner.refresh(StatuslineContext::default()).await;
        assert_eq!(out.trim(), "hello world");
    }

    #[tokio::test]
    async fn runner_returns_cached_on_timeout() {
        let runner = StatuslineRunner::new(StatuslineConfig {
            command: "/bin/sh -c 'sleep 2; echo too-slow'".into(),
            timeout_ms: 100, padding: 1,
        });
        runner.set_cached_for_test("cached".into()).await;
        let out = runner.refresh(StatuslineContext::default()).await;
        assert_eq!(out.trim(), "cached");
    }
}
```

- [ ] **Step 2: Run tests** → FAIL.

- [ ] **Step 3: Implement `StatuslineRunner`**

```rust
//! Spawns a user-configured statusline script after each turn end and
//! caches its last line for the renderer.

use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::process::Command;
use tokio::io::AsyncWriteExt as _;
use serde::Serialize;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct StatuslineConfig {
    pub command: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u32,
    #[serde(default = "default_padding")]
    pub padding: u8,
}
fn default_timeout_ms() -> u32 { 200 }
fn default_padding() -> u8 { 1 }

#[derive(Debug, Clone, Default, Serialize)]
pub struct StatuslineContext {
    pub model: String,
    pub cost_usd: String,
    pub permission_mode: String,
    pub effort: String,
    pub workspace_root: String,
    pub session_id: String,
    pub turn_count: u32,
}

pub struct StatuslineRunner {
    config: StatuslineConfig,
    cache: Mutex<Option<(Instant, String)>>,
    consecutive_timeouts: Mutex<u8>,
}

const MAX_CONSECUTIVE_TIMEOUTS: u8 = 3;
const MAX_LINE_LEN: usize = 120;

impl StatuslineRunner {
    pub fn new(config: StatuslineConfig) -> Self {
        Self { config, cache: Mutex::new(None), consecutive_timeouts: Mutex::new(0) }
    }
    pub async fn refresh(&self, ctx: StatuslineContext) -> String {
        // If disabled (too many timeouts), short-circuit to cache.
        if *self.consecutive_timeouts.lock().await >= MAX_CONSECUTIVE_TIMEOUTS {
            return self.cached().await;
        }
        let payload = serde_json::to_string(&ctx).unwrap_or_default();
        let mut parts = self.config.command.split_whitespace();
        let prog = match parts.next() { Some(p) => p, None => return self.cached().await };
        let args: Vec<&str> = parts.collect();
        let mut cmd = Command::new(prog);
        cmd.args(&args).kill_on_drop(true).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
        let child = match cmd.spawn() { Ok(c) => c, Err(_) => return self.cached().await };
        let mut child = child;
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(payload.as_bytes()).await;
        }
        let timeout = Duration::from_millis(self.config.timeout_ms as u64);
        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;
        match result {
            Ok(Ok(o)) => {
                *self.consecutive_timeouts.lock().await = 0;
                let line: String = String::from_utf8_lossy(&o.stdout)
                    .lines().next().unwrap_or("").chars().take(MAX_LINE_LEN).collect();
                *self.cache.lock().await = Some((Instant::now(), line.clone()));
                line
            }
            _ => {
                let mut t = self.consecutive_timeouts.lock().await; *t += 1;
                if *t == MAX_CONSECUTIVE_TIMEOUTS {
                    tracing::warn!(target: "caliban::statusline", "statusline script timed out {} times; disabling for session", MAX_CONSECUTIVE_TIMEOUTS);
                }
                self.cached().await
            }
        }
    }
    async fn cached(&self) -> String {
        self.cache.lock().await.as_ref().map(|(_, s)| s.clone()).unwrap_or_default()
    }
    #[cfg(test)]
    pub async fn set_cached_for_test(&self, s: String) {
        *self.cache.lock().await = Some((Instant::now(), s));
    }
}
```

Add `statusLine` and `tui.showCostInStatusline` to the schema, parse in `caliban-settings/src/lib.rs`, propagate to `App` (held as `Option<Arc<StatuslineRunner>>`).

In `caliban/src/tui/events.rs`, on `TurnEvent::RunEnd` / `TurnEnd`:

```rust
if let Some(runner) = app.statusline_runner.clone() {
    let ctx = build_statusline_context(app);
    tokio::spawn(async move { let _ = runner.refresh(ctx).await; });
}
```

In `caliban/src/tui/render.rs`, prepend the cached value (read via a non-blocking try-lock or a snapshot field on `App`) to the existing statusline render.

- [ ] **Step 4: Run tests**

Run: `cargo test -p caliban-settings statusline`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-settings/src/statusline.rs crates/caliban-settings/src/schema.json crates/caliban-settings/src/lib.rs \
        caliban/src/tui/app.rs caliban/src/tui/events.rs caliban/src/tui/render.rs
git commit -m "feat(settings+tui): custom statusline script (claude-code script-compatible)"
```

---

## Task 13: Workspace sanity + parity matrix

- [ ] **Step 1: Run the full workspace test pass**

Run: `cargo test --workspace --all-features`
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Run: `cargo fmt --all --check`
Expected: PASS.

- [ ] **Step 2: Update `docs/parity-gap-matrix.md`**

For each new/improved surface, tick the row to ✅ (or add a note):
- Section A: permission modal — note the 4-button extension.
- Section K: `/cost` add, `/doctor` real checks, `Status line (custom script)` → ✅.
- Section M: `/effort`, `/resume`, `/context`, `/export` no longer stubs.

Bump the "Last refreshed" date to `2026-05-26`.

- [ ] **Step 3: Commit**

```bash
git add docs/parity-gap-matrix.md
git commit -m "docs: tick TUI slash + statusline rows in parity matrix"
```

---

## Self-Review Notes

- **Spec coverage:** Task 1 (`/clear`), Task 2 (Effort enum), Task 3 (`/effort` slash), Task 4 (OpenAI plumb), Task 5 (Anthropic plumb), Task 5a (`Agent::active_model` + `try_swap_model`), Task 5b (`/model` slash + picker), Task 6 (`/cost`), Task 7 (`/doctor`), Task 8 (`/resume`), Task 9 (`/context`), Task 10 (`/export`), Task 11 (permission modal), Task 12 (custom statusline), Task 13 (workspace + matrix). All ten spec sections traced.
- **Trade-off called out:** `Effort` lives in `caliban-provider` (Task 4 Step 3 note) to avoid a cyclic dep between `caliban-provider` and `caliban-agent-core`. Re-exported from `agent-core` for convenience.
- **Why `active_model` lives on `Agent`, not `AgentConfig`:** `AgentConfig` is `Clone` and cloned per turn for the request builder. Threading `ArcSwap` through a clonable struct invites subtle bugs (forks share or diverge depending on the field). Putting it on `Agent` keeps the swap point at the long-lived owner, mirroring how `permission_mode` works.
- **Headless `caliban context --print`** is deferred to a follow-up to keep this PR focused; the TUI side ships first.
- **Persistent runtime rules:** the spec defers persisting runtime rules to disk; this plan respects that — `RuntimeRule` is session-scoped only.
- **Statusline test on macOS only:** the test script uses `/bin/sh` which is reliable on macOS + most Linux. Windows-CI runs will need a `#[cfg(unix)]` gate around the script test (add when the workspace gains Windows CI).
- **Cross-provider `/model` is deferred.** The picker lists cross-provider routes but does not attempt the swap; selecting one prints a CrossProvider message instead. Hot-swap across providers needs a model-router-driven Agent factory (called out in Spec C open questions).
