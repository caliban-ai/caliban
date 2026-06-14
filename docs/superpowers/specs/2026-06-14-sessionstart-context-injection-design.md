# SessionStart context-injection hook surface (#106)

**Status:** Design approved 2026-06-14
**Issue:** caliban-ai/caliban#106
**Spun out of:** #56 (built-in skill-guidance nudge — direction #3)

## Problem

caliban has a `session_start()` hook event (`crates/caliban-agent-core/src/hooks.rs`),
but hooks cannot inject text into the system prompt or conversation at session start.
Claude Code's superpowers pack relies exactly on this — a SessionStart injection that
adds `additionalContext` mandating the model check for an applicable skill before
acting. #56 shipped a built-in, caliban-managed nudge (`tools.skill_guidance` + a
`## Skills` system-prompt block); this is the *general, extensible* version: let any
SessionStart hook contribute context that reaches the model on turn 1, so skill packs
and plugins can ship their own activation/guidance preambles.

## Goal

A configured SessionStart hook (trait impl or external config handler) can return
`additionalContext` text that is spliced into the system prompt before the first turn,
honoring existing hook gating, with the #56 built-in nudge remaining as an independent,
additive fallback.

## Key constraint discovered

In `caliban/src/main.rs`, `startup::fire_session_start` (line ~408) runs **before**
`startup::resolve_system_prompt` (line ~420), which builds the system prompt and inserts
it as `message[0]`. This ordering means hook-returned context can be threaded directly
into `resolve_system_prompt` and spliced into the system prompt — no reordering needed.

A second, redundant `session_start` fire exists inside `startup::run_headless`
(`caliban/src/startup.rs:~984`) purely so `--include-hook-events` emits the frame. Context
must be captured **once** (at the `main.rs:408` firing) and threaded forward; the
headless re-fire stays event-emission-only so context is not double-injected.

## Design

### 1. Hook return shape

Change the trait method:

```rust
// crates/caliban-agent-core/src/hooks.rs
async fn session_start(&self, _ctx: &SessionCtx<'_>) -> Result<SessionStartOutcome> {
    Ok(SessionStartOutcome::default())
}
```

New outcome type (mirrors the `before_tool` / `user_prompt_submit` pattern of returning
an outcome rather than `()`):

```rust
#[derive(Debug, Clone, Default)]
pub struct SessionStartOutcome {
    /// Context blocks contributed by SessionStart hooks, in firing order.
    pub additional_context: Vec<String>,
}
```

- `NoopHooks::session_start` → `Ok(SessionStartOutcome::default())`.
- `CompositeHooks::session_start` → fire each child in order, concatenating every
  child's `additional_context` into one `Vec<String>` (preserving order).
- Rejected alternative: reuse `HookDecision`. Its allow/deny/rewrite semantics do not
  fit additive context, and `session_start` has no notion of denial.

### 2. External (config) hooks

`hooks_router.rs` handlers (`ShellCommandHook`, `HttpHook`, `PromptHook`, `AgentHook`,
`McpHook`) spawn the handler, send the event envelope as JSON on stdin, and parse stdout.
For SessionStart, extend the parse path to read `additionalContext` from the handler's
stdout JSON and surface it via `SessionStartOutcome::additional_context`.

Accepted JSON shapes (Claude Code-compatible):

```json
{ "additionalContext": "text..." }
```

and the nested form:

```json
{ "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": "text..." } }
```

Non-JSON or absent `additionalContext` → no context contributed (empty), preserving
current best-effort behavior.

### 3. Placement — system-prompt block

Thread the captured `Vec<String>` into `resolve_system_prompt` and append it as a
dedicated block, alongside the existing `append_skills_block` (#56). A new
`system_prompt::append_session_context_block(body, &blocks)` helper wraps the
concatenated blocks in a clearly delimited section (e.g. `<session-context>` …
`</session-context>`) and appends at the tail, so it survives output-style and
memory-tier layering exactly as the skills block does. Empty input → no block, no
delimiter (byte-identical to today's prompt).

This persists across turns as part of the system prompt and matches Claude Code's
SessionStart `additionalContext` semantics. The synthetic-leading-message alternative
was rejected as more invasive (touches session/history construction across the
fresh-session, headless, and resume paths) and only one-shot.

### 4. Threading

- `fire_session_start` returns the `SessionStartOutcome` (or just the
  `Vec<String>`) to its `main.rs` caller instead of discarding it.
- `resolve_system_prompt` gains a `session_context: &[String]` parameter; it appends the
  session-context block after (or alongside) the skills block in every return path
  (custom-prompt path, default-prompt path, and the early `None` short-circuit returns
  `None` unchanged — no system prompt means nothing to splice into).
- `run_headless`'s internal `session_start` fire is annotated/adjusted to not re-capture
  context for injection (event emission only).

### 5. Gating & fallback

- `disable_all_hooks` and managed-hooks-only (`allow_*_hooks_only`) gating is enforced in
  the router/composite before handlers run, so a disabled hook contributes no context for
  free — no new gating code on the injection path.
- The #56 built-in skills nudge is **independent and additive**: it still fires when no
  hook supplies context. Both blocks can coexist.
- No new opt-out setting (YAGNI — existing hook gating covers disablement).

## Testing

Unit:
- `CompositeHooks::session_start` concatenates child `additional_context` in order.
- `NoopHooks::session_start` returns empty.
- Router parses `additionalContext` from both flat and nested stdout JSON; non-JSON →
  empty.
- `append_session_context_block`: empty input is a no-op (byte-identical); non-empty
  wraps and appends; coexists with the skills block.
- Gating: a disabled / managed-filtered hook contributes no context.

Integration:
- A configured SessionStart hook's text reaches the system `message[0]` on turn 1.
- With no hook configured, the #56 skills nudge is still present and unchanged.

## Scope / blast radius

The trait signature change ripples to every `Hooks` impl: `NoopHooks`, `CompositeHooks`,
the `hooks_router.rs` handlers, `PermissionsHook`, and test doubles (e.g. the
`CountingHook` in `tui.rs`). Each non-contributing impl simply returns
`SessionStartOutcome::default()`. No behavior change for callers that ignore the outcome.

## Acceptance criteria (from #106)

- [x] A configured SessionStart hook can inject context that reaches the model on turn 1.
- [x] Injection respects `disable_all_hooks` / managed-hooks gating.
- [x] Existing #56 built-in nudge still works when no hook supplies guidance.
- [x] Unit/integration coverage for the inject-and-splice path.
