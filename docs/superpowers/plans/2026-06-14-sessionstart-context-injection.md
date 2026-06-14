# SessionStart context-injection hook surface (#106) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a SessionStart hook return `additionalContext` text that is spliced into the system prompt before turn 1, with the #56 built-in nudge as an independent fallback.

**Architecture:** Change the `Hooks::session_start` trait method to return a typed `SessionStartOutcome { additional_context: Vec<String> }`. `CompositeHooks` concatenates children's context in firing order. `fire_session_start` returns the collected `Vec<String>` to `main.rs`, which threads it into `resolve_system_prompt`; a new `append_session_context_block` helper splices it into the system prompt alongside the existing skills block. A reusable `additionalContext` JSON parser ships for #121 to call once config-hook execution is wired.

**Tech Stack:** Rust, `async-trait`, `serde_json`, `tokio`. Workspace: `caliban-ai/caliban`.

**Scope boundary:** External config (`[[hooks.SessionStart]]`) handlers are not executed at runtime today (tracked in #121). This plan delivers the injection *surface* + parser; end-to-end config-hook injection lands with #121.

**Spec:** `docs/superpowers/specs/2026-06-14-sessionstart-context-injection-design.md`

**Verification gate (run before any push, per CLAUDE.md):**
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```

---

## File Structure

- **Modify** `crates/caliban-agent-core/src/hooks.rs` — add `SessionStartOutcome`; change `session_start` trait return type; update `CompositeHooks::session_start` to concatenate; export the new type.
- **Modify** `crates/caliban-agent-core/src/lib.rs` — re-export `SessionStartOutcome`.
- **Modify** (mechanical signature update, return `default()`):
  - `crates/caliban-agent-core/src/decision_log.rs:215`
  - `crates/caliban-agent-core/src/mode_filter.rs:168`
  - `crates/caliban-agent-core/src/permissions.rs:527`
  - `caliban/src/headless/hooks_sink.rs:106`
  - `crates/caliban-agent-core/tests/hooks_events.rs:36,700`
- **Modify** `crates/caliban-agent-core/src/hooks_router.rs` — add `parse_session_start_context` + unit tests.
- **Modify** `caliban/src/system_prompt.rs` — add `append_session_context_block` + unit tests.
- **Modify** `caliban/src/startup.rs` — `fire_session_start` returns `Vec<String>`; `resolve_system_prompt` gains a `session_context: &[String]` param and splices the block; `run_headless` internal re-fire stays event-only.
- **Modify** `caliban/src/main.rs` — capture `fire_session_start` result, pass into `resolve_system_prompt`.
- **Add** integration test `caliban/tests/session_start_context.rs` (or extend existing) — trait-impl hook injects context into `message[0]`; #56 nudge present with no hook.

---

## Task 1: Introduce `SessionStartOutcome` and change the trait (whole workspace compiles)

A Rust trait-signature change does not compile until every impl is updated, so this task lands the type + trait + all mechanical impl updates together. The behavioral piece (CompositeHooks concatenation) is covered by its own test in Step 6.

**Files:**
- Modify: `crates/caliban-agent-core/src/hooks.rs`
- Modify: `crates/caliban-agent-core/src/lib.rs:47-50`
- Modify: `crates/caliban-agent-core/src/decision_log.rs:215`
- Modify: `crates/caliban-agent-core/src/mode_filter.rs:168`
- Modify: `crates/caliban-agent-core/src/permissions.rs:527`
- Modify: `caliban/src/headless/hooks_sink.rs:106`
- Modify: `crates/caliban-agent-core/tests/hooks_events.rs:36,700`

- [ ] **Step 1: Add the `SessionStartOutcome` type**

In `crates/caliban-agent-core/src/hooks.rs`, near the other outcome types (after `TurnDecision`, ~line 46), add:

```rust
/// Outcome of [`Hooks::session_start`]. Carries context blocks a SessionStart
/// hook wants spliced into the system prompt before the first turn. Empty by
/// default (the common case: a hook with no context to contribute).
#[derive(Debug, Clone, Default)]
pub struct SessionStartOutcome {
    /// Context blocks contributed by SessionStart hooks, in firing order.
    /// Each entry is appended to the system prompt's session-context block.
    pub additional_context: Vec<String>,
}
```

- [ ] **Step 2: Change the trait default method**

In `crates/caliban-agent-core/src/hooks.rs:408-412`, replace:

```rust
    async fn session_start(&self, _ctx: &SessionCtx<'_>) -> Result<()> {
        Ok(())
    }
```

with:

```rust
    /// Fired once when a session begins (after settings load, before the
    /// first user prompt). Return [`SessionStartOutcome`] to contribute
    /// context spliced into the system prompt before turn 1.
    async fn session_start(&self, _ctx: &SessionCtx<'_>) -> Result<SessionStartOutcome> {
        Ok(SessionStartOutcome::default())
    }
```

- [ ] **Step 3: Update `CompositeHooks::session_start` to concatenate**

In `crates/caliban-agent-core/src/hooks.rs:724-732`, replace:

```rust
    async fn session_start(&self, ctx: &SessionCtx<'_>) -> Result<()> {
        if self.all_noop {
            return Ok(());
        }
        for h in &self.layers {
            h.session_start(ctx).await?;
        }
        Ok(())
    }
```

with:

```rust
    async fn session_start(&self, ctx: &SessionCtx<'_>) -> Result<SessionStartOutcome> {
        if self.all_noop {
            return Ok(SessionStartOutcome::default());
        }
        let mut merged = SessionStartOutcome::default();
        for h in &self.layers {
            let outcome = h.session_start(ctx).await?;
            merged.additional_context.extend(outcome.additional_context);
        }
        Ok(merged)
    }
```

- [ ] **Step 4: Re-export the type**

In `crates/caliban-agent-core/src/lib.rs`, add `SessionStartOutcome` to the `hooks::{...}` re-export list (line ~47, alongside `SessionCtx`, `SessionOutcome`):

```rust
    TaskCtx, TaskOutcome, ToolCtx, TurnCtx, TurnDecision, SessionStartOutcome, build_envelope, envelope_with_cwd,
```

(Keep the existing items; just add `SessionStartOutcome`. Match the exact existing line's other names — do not drop any.)

- [ ] **Step 5: Update all non-contributing impls (mechanical)**

Each of these does its existing side-effect work, then returns an empty outcome. For each, change the return type from `Result<()>` / `HookResult<()>` to `Result<SessionStartOutcome>` / `HookResult<SessionStartOutcome>` and replace the trailing `Ok(())` with `Ok(SessionStartOutcome::default())`. Import the type where needed (`use crate::hooks::SessionStartOutcome;` or `caliban_agent_core::SessionStartOutcome`).

- `crates/caliban-agent-core/src/decision_log.rs:215` — keep the body; change signature + final `Ok(())` → `Ok(crate::hooks::SessionStartOutcome::default())`.
- `crates/caliban-agent-core/src/mode_filter.rs:168` — same.
- `crates/caliban-agent-core/src/permissions.rs:527` — same.
- `caliban/src/headless/hooks_sink.rs:106` — returns `HookResult<()>`; change to `HookResult<SessionStartOutcome>`, final `Ok(())` → `Ok(SessionStartOutcome::default())`. Add `use caliban_agent_core::SessionStartOutcome;` if not already imported.
- `crates/caliban-agent-core/tests/hooks_events.rs:36` and `:700` — test doubles; same mechanical change.

- [ ] **Step 6: Add a CompositeHooks concatenation test**

In `crates/caliban-agent-core/src/hooks.rs` `#[cfg(test)]` module, add a test with two fake hooks each returning one context string, asserting `CompositeHooks` returns both in order:

```rust
#[tokio::test]
async fn composite_session_start_concatenates_context_in_order() {
    struct CtxHook(&'static str);
    #[async_trait::async_trait]
    impl Hooks for CtxHook {
        async fn session_start(&self, _ctx: &SessionCtx<'_>) -> Result<SessionStartOutcome> {
            Ok(SessionStartOutcome { additional_context: vec![self.0.to_string()] })
        }
    }
    let composite = CompositeHooks::new(vec![
        std::sync::Arc::new(CtxHook("first")) as std::sync::Arc<dyn Hooks>,
        std::sync::Arc::new(CtxHook("second")) as std::sync::Arc<dyn Hooks>,
    ]);
    let cwd = std::path::Path::new(".");
    let ctx = SessionCtx { session_id: "t", cwd, provider: "test", model: "m" };
    let out = composite.session_start(&ctx).await.unwrap();
    assert_eq!(out.additional_context, vec!["first".to_string(), "second".to_string()]);
}
```

(Adjust `CompositeHooks::new` arg type / `Arc<dyn Hooks>` coercion to match the existing constructor signature in this file. If other tests in this module already build `CompositeHooks`, mirror their exact construction style.)

- [ ] **Step 7: Build + test to verify the workspace compiles and the concat test passes**

Run:
```bash
cargo build --workspace --all-targets
cargo test --workspace --lib -p caliban-agent-core composite_session_start
```
Expected: build succeeds; the concat test passes.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(hooks): session_start returns SessionStartOutcome with additional_context (#106)"
```

---

## Task 2: `additionalContext` parser in the router

**Files:**
- Modify: `crates/caliban-agent-core/src/hooks_router.rs`
- Test: same file `#[cfg(test)]` module

- [ ] **Step 1: Write failing tests**

In the `#[cfg(test)]` module of `crates/caliban-agent-core/src/hooks_router.rs`, add:

```rust
#[test]
fn session_start_context_flat_shape() {
    let blob = r#"{ "additionalContext": "hello from hook" }"#;
    assert_eq!(parse_session_start_context(blob), Some("hello from hook".to_string()));
}

#[test]
fn session_start_context_nested_shape() {
    let blob = r#"{ "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": "nested ctx" } }"#;
    assert_eq!(parse_session_start_context(blob), Some("nested ctx".to_string()));
}

#[test]
fn session_start_context_absent_or_nonjson() {
    assert_eq!(parse_session_start_context(""), None);
    assert_eq!(parse_session_start_context("not json"), None);
    assert_eq!(parse_session_start_context(r#"{ "other": 1 }"#), None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
cargo test -p caliban-agent-core session_start_context
```
Expected: FAIL — `parse_session_start_context` not found.

- [ ] **Step 3: Implement the parser**

In `crates/caliban-agent-core/src/hooks_router.rs`, near `parse_decision_blob` (~line 50), add. Note the existing `HookSpecificOutput` struct (line 34) does not include `additionalContext`; add an independent deserialization struct so we don't disturb the decision path:

```rust
/// Stdout JSON shapes that can carry SessionStart `additionalContext`.
#[derive(Debug, Deserialize, Default)]
struct SessionStartBlob {
    #[serde(rename = "additionalContext")]
    additional_context: Option<String>,
    #[serde(rename = "hookSpecificOutput", default)]
    hook_specific_output: Option<SessionStartNested>,
}

#[derive(Debug, Deserialize, Default)]
struct SessionStartNested {
    #[serde(rename = "additionalContext")]
    additional_context: Option<String>,
}

/// Extract SessionStart `additionalContext` from a handler's stdout JSON.
/// Accepts the flat (`{"additionalContext": ...}`) and nested
/// (`{"hookSpecificOutput": {"additionalContext": ...}}`) shapes. Returns
/// `None` for empty / non-JSON / absent input.
///
/// Reusable surface for #121 (config-hook execution) — not yet invoked here,
/// since config handlers are not wired into the runtime Hooks chain.
pub(crate) fn parse_session_start_context(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let blob = serde_json::from_str::<SessionStartBlob>(trimmed).ok()?;
    blob.additional_context
        .or_else(|| blob.hook_specific_output.and_then(|n| n.additional_context))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
cargo test -p caliban-agent-core session_start_context
```
Expected: PASS (3 tests).

- [ ] **Step 5: Silence dead-code if needed**

`parse_session_start_context` is `pub(crate)` but not yet called in production (it's for #121). If clippy/`-D warnings` flags it as unused, annotate with `#[allow(dead_code)]` and a comment referencing #121. Verify:
```bash
cargo clippy -p caliban-agent-core --all-targets -- -D warnings
```
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(hooks): parse SessionStart additionalContext from handler stdout (#106, for #121)"
```

---

## Task 3: `append_session_context_block` in system_prompt

**Files:**
- Modify: `caliban/src/system_prompt.rs`
- Test: same file `#[cfg(test)]` module

- [ ] **Step 1: Write failing tests**

In `caliban/src/system_prompt.rs` `#[cfg(test)]` module (~line 159), add:

```rust
#[test]
fn session_context_block_empty_is_noop() {
    let base = "You are caliban.\n";
    assert_eq!(append_session_context_block(base, &[]), base);
}

#[test]
fn session_context_block_appends_wrapped() {
    let base = "You are caliban.";
    let out = append_session_context_block(base, &["alpha".to_string(), "beta".to_string()]);
    assert!(out.starts_with("You are caliban."));
    assert!(out.contains("<session-context>"));
    assert!(out.contains("alpha"));
    assert!(out.contains("beta"));
    assert!(out.contains("</session-context>"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
cargo test -p caliban session_context_block
```
Expected: FAIL — `append_session_context_block` not found.

- [ ] **Step 3: Implement the helper**

In `caliban/src/system_prompt.rs`, after `append_skills_block` (~line 157), add (mirror the skills/todo block's trailing-newline handling):

```rust
/// Append a `<session-context>` block carrying SessionStart hook-supplied
/// context. Returns the prompt unchanged when `blocks` is empty (byte-identical
/// to today's prompt — no delimiter emitted). Blocks are joined with a blank
/// line in firing order.
#[must_use]
pub(crate) fn append_session_context_block(prompt: &str, blocks: &[String]) -> String {
    if blocks.is_empty() {
        return prompt.to_string();
    }
    let joined = blocks.join("\n\n");
    let mut out = String::with_capacity(prompt.len() + joined.len() + 64);
    out.push_str(prompt);
    if !prompt.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\n<session-context>\n");
    out.push_str(&joined);
    if !joined.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("</session-context>\n");
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
cargo test -p caliban session_context_block
```
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(system-prompt): append_session_context_block helper (#106)"
```

---

## Task 4: Thread hook context into the system prompt

**Files:**
- Modify: `caliban/src/startup.rs` (`fire_session_start` ~1622, `resolve_system_prompt` ~1938, `run_headless` ~984)
- Modify: `caliban/src/main.rs` (~408, ~419-420)

- [ ] **Step 1: Make `fire_session_start` return the collected context**

In `caliban/src/startup.rs:1622-1634`, change the signature and body:

```rust
pub(crate) async fn fire_session_start(args: &Args, agent: &Arc<Agent>, model: &str) -> Vec<String> {
    let cwd_now = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let session_id = args.session.clone().unwrap_or_else(|| "ephemeral".into());
    let session_ctx = caliban_agent_core::SessionCtx {
        session_id: &session_id,
        cwd: &cwd_now,
        provider: provider_name(resolved_provider(args)),
        model,
    };
    match agent.hooks().session_start(&session_ctx).await {
        Ok(outcome) => outcome.additional_context,
        Err(e) => {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_HOOKS, error = %e, "session_start hook error (non-fatal)");
            Vec::new()
        }
    }
}
```

- [ ] **Step 2: Add `session_context` param to `resolve_system_prompt` and splice it**

In `caliban/src/startup.rs:1938`, add a parameter:

```rust
pub(crate) async fn resolve_system_prompt(
    args: &Args,
    agent: &Arc<Agent>,
    cwd_for_prompt: &std::path::Path,
    settings_snapshot: &caliban_settings::Settings,
    session_context: &[String],
) -> Result<Option<String>> {
```

Then wrap the two non-`None` return paths so the session-context block is appended *after* the skills block. Replace the custom-prompt return (lines ~1972-1977):

```rust
    if !default_prompt_in_effect {
        let with_skills = system_prompt::append_skills_block(&body, &skill_names);
        return Ok(Some(system_prompt::append_session_context_block(
            &with_skills,
            session_context,
        )));
    }
```

and the final return (lines ~2029-2032):

```rust
    let with_skills = system_prompt::append_skills_block(&final_prompt, &skill_names);
    Ok(Some(system_prompt::append_session_context_block(
        &with_skills,
        session_context,
    )))
```

(The early `let Some(body) = system_prompt else { return Ok(None); };` path stays unchanged — no system prompt means nothing to splice into.)

- [ ] **Step 3: Update `main.rs` to capture and pass the context**

In `caliban/src/main.rs:408`, capture the result:

```rust
    // Fire SessionStart hook (best-effort); collect any hook-supplied context.
    let session_context = startup::fire_session_start(&args, &agent, &model).await;
```

Then at the `resolve_system_prompt` call (line ~419-420), pass it:

```rust
    let system_prompt =
        startup::resolve_system_prompt(&args, &agent, &cwd_for_prompt, &settings_snapshot, &session_context).await?;
```

- [ ] **Step 4: Prevent double-injection in the headless re-fire**

In `caliban/src/startup.rs:984`, the `run_headless` internal `session_start` fire is for event emission only. Its result is already discarded via `if let Err(e) = ...`. The return type is now `Result<SessionStartOutcome>`; the `if let Err(e)` pattern still compiles (the `Ok(_)` outcome is dropped). Add a clarifying comment so the discard is intentional:

```rust
        // Event-emission only: context injection already happened at the
        // main.rs SessionStart fire (threaded into the system prompt). We
        // discard the outcome here to avoid double-injecting.
        if let Err(e) = agent.hooks().session_start(&session_ctx).await {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_HOOKS, error = %e, "session_start hook error (non-fatal)");
        }
```

- [ ] **Step 5: Build to verify everything compiles**

Run:
```bash
cargo build --workspace --all-targets
```
Expected: success. Fix any other `resolve_system_prompt` / `fire_session_start` call sites the compiler flags (grep first: `rg -n "resolve_system_prompt|fire_session_start" caliban/src` — expected only main.rs + the defs).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(startup): splice SessionStart hook context into the system prompt (#106)"
```

---

## Task 5: Integration test — context reaches the model; #56 nudge preserved

**Files:**
- Create: `caliban/tests/session_start_context.rs` (or extend an existing integration test if one already exercises `resolve_system_prompt`; check `caliban/tests/` first)

- [ ] **Step 1: Check for an existing harness**

Run:
```bash
ls caliban/tests/ && rg -l "resolve_system_prompt|session_start" caliban/tests/ 2>/dev/null
```
If an existing test already builds an `Agent` with custom hooks + calls `resolve_system_prompt`, extend it. Otherwise create the new file below.

- [ ] **Step 2: Write the test**

Because `resolve_system_prompt` takes `session_context: &[String]` directly, the inject-and-splice path is testable without spawning a real hook: pass a non-empty slice and assert the block appears; pass empty and assert the #56 skills block is still present (when skills are loaded) and no `<session-context>` delimiter appears.

If `resolve_system_prompt` is `pub(crate)` (not reachable from `tests/`), prefer a unit test in `caliban/src/startup.rs`'s `#[cfg(test)]` module instead. Add there:

```rust
// In caliban/src/startup.rs #[cfg(test)] module.
#[tokio::test]
async fn session_context_is_spliced_into_prompt() {
    // Build a minimal agent + default args so the default prompt is in effect.
    // (Mirror the construction used by existing resolve_system_prompt tests in
    // this module; if none exist, build via the same Args::parse_from(["caliban"])
    // + build_agent path other startup tests use.)
    let args = test_args();                 // helper: default Args
    let agent = test_agent(&args).await;    // helper: minimal agent
    let settings = caliban_settings::Settings::default();
    let cwd = std::env::current_dir().unwrap();

    let with_ctx = resolve_system_prompt(&args, &agent, &cwd, &settings, &["INJECTED-MARKER".to_string()])
        .await
        .unwrap()
        .expect("default prompt in effect");
    assert!(with_ctx.contains("<session-context>"));
    assert!(with_ctx.contains("INJECTED-MARKER"));

    let without_ctx = resolve_system_prompt(&args, &agent, &cwd, &settings, &[])
        .await
        .unwrap()
        .expect("default prompt in effect");
    assert!(!without_ctx.contains("<session-context>"));
}
```

If `test_args` / `test_agent` helpers do not already exist in the module, reuse the exact construction pattern from the nearest existing `#[tokio::test]` in `startup.rs` (search: `rg -n "async fn .*\(\) \{" caliban/src/startup.rs` within `#[cfg(test)]`). Do not invent a new agent-construction path — copy a working one.

- [ ] **Step 3: Run the test to verify it fails (if written before Task 4) or passes (after)**

Run:
```bash
cargo test -p caliban session_context_is_spliced_into_prompt -- --nocapture
```
Expected: PASS (Task 4 already implemented the splice).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test(startup): SessionStart context splices into prompt; absent when empty (#106)"
```

---

## Task 6: Full verification gate + parity matrix + PR

- [ ] **Step 1: Run the complete CI-mirroring gate**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```
All four must pass. If `fmt --check` fails, run `cargo fmt --all` and re-commit.

- [ ] **Step 2: Tick the parity matrix if applicable**

Check `docs/parity-gap-matrix.md` for a hooks/SessionStart row covering this surface; if present, advance its status (🔴→🟡 or note "surface shipped; config-hook execution tracked in #121"). Commit any change.

- [ ] **Step 3: Push the branch and open the PR**

```bash
git push -u origin worktree-issue-106-sessionstart-context-injection
gh pr create --repo caliban-ai/caliban \
  --title "feat(tools): SessionStart context-injection hook surface (#106)" \
  --body "Closes #106. Adds the SessionStart context-injection surface: session_start returns SessionStartOutcome { additional_context }, CompositeHooks concatenates, and hook-supplied context is spliced into the system prompt before turn 1 (alongside the #56 skills nudge, which remains an independent fallback). Ships a reusable additionalContext parser for #121. Wiring config-defined [[hooks.*]] handlers into the runtime is tracked separately in #121.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

---

## Self-Review

**Spec coverage:**
- §1 Hook return shape → Task 1 (type + trait + composite concat).
- §2 External hooks parser (scope-bounded) → Task 2.
- §3 Placement (system-prompt block) → Task 3 + Task 4.
- §4 Threading → Task 4.
- §5 Gating & fallback → inherited via CompositeHooks/existing gating; #56 nudge preserved, asserted in Task 5.
- Testing → Tasks 1, 2, 3, 5.

**Placeholder scan:** No TBDs. Test-helper reuse (Task 5) explicitly instructs copying an existing construction pattern rather than inventing one — acceptable because the exact agent-construction boilerplate depends on existing test scaffolding the worker must read.

**Type consistency:** `SessionStartOutcome { additional_context: Vec<String> }`, `append_session_context_block(&str, &[String]) -> String`, `parse_session_start_context(&str) -> Option<String>`, `fire_session_start(...) -> Vec<String>`, `resolve_system_prompt(..., session_context: &[String])` — names used consistently across Tasks 1–5.
