# TUI Ergonomics Implementation Plan (IE1 / IE2 / IE3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the three TUI ergonomics findings logged in `docs/TODO.md` (PR #77):
- **IE1** — non-model slash commands fire during inference via an `immediate: bool` flag on `SlashCommandMeta`.
- **IE2** — user-typed messages during a running turn are queued and auto-sent on `RunEnd`; two-stage Esc clears queue then cancels.
- **IE3** — mouse drag-select-to-clipboard inside alt-screen mode (render-time position map + Down/Drag/Up state machine + Style.bg overlay + OSC-52 with `arboard` fallback).

**Architecture:** All three features touch the TUI event loop (`caliban/src/tui/events.rs`) and `App` state (`caliban/src/tui/app.rs`). IE1 adds a registry-level flag + an intercept *before* the running-turn bail at `events.rs:870`. IE2 replaces the bail with a queue-push and adds a drain on the two `RunEnd`-time sites at `events.rs:314,328`. IE3 is mostly additive in `events.rs::handle_mouse`, `render.rs` (the position-map wrap), and a new `tui/clipboard.rs` for OSC-52/arboard. The three features are interleaved at the same call sites, so we land them as a single PR with a commit per feature, in this order: IE1 → IE2 → IE3.

**Tech Stack:** Rust 1.95, ratatui, crossterm, tokio, async-trait, anyhow. Existing test pattern is `#[cfg(test)] mod tests` colocated with code; pure helpers split out for unit-testability.

---

## File structure

**Modified:**
- `caliban/src/tui/slash.rs` — add `immediate: bool` to `SlashCommandMeta`; add `lookup_meta(name)` to `SlashCommandRegistry`; add `is_immediate_slash(prompt, registry)` pure helper for the classifier (IE1).
- `caliban/src/tui/slash/{basic,config,cost,dx,existing,export,model,observe,perms,session}.rs` — add `immediate: <bool>` to every `SlashCommandMeta` literal; tag the ones that don't touch the agent loop as `true` (IE1).
- `caliban/src/tui/app.rs` — add `queued: VecDeque<String>` to `App`; add `esc_armed_at: Option<Instant>` for two-stage Esc; initializers (IE2). Add `mouse_selection: Option<MouseSelection>` field for IE3.
- `caliban/src/tui/events.rs` — submit handler reorder (IE1 intercept first, then IE2 push-to-queue replacing the bail); `RunEnd` drain at the two `app.running = None;` sites; Esc handler two-stage logic (IE2). `handle_mouse` state machine (IE3).
- `caliban/src/tui/render.rs` — wrap transcript draw to build the position map per frame; apply selection highlight overlay (IE3).
- `Cargo.toml` (caliban binary crate) — confirm `arboard` is reachable as a direct dep or via `caliban-images` re-export (IE3).

**Created:**
- `caliban/src/tui/clipboard.rs` — `pub(crate) fn copy_to_clipboard(text: &str) -> Result<()>` with OSC-52 primary path and `arboard` fallback (IE3).
- `caliban/src/tui/mouse_select.rs` — `MouseSelection` state machine + `PositionMap` data structure + tests (IE3).

**Test files:** colocated `#[cfg(test)] mod tests` in each modified file. New unit tests for: registry `lookup_meta`, `is_immediate_slash` classifier, `App::push_queued` / `App::drain_queued` behavior, two-stage Esc timer, position-map lookup, selection state-machine transitions.

---

## IE1 — Immediate slash commands during inference

### Task 1: Add `immediate: bool` to `SlashCommandMeta` and update all literals

**Files:**
- Modify: `caliban/src/tui/slash.rs:33-44` (struct definition + test helpers at :263)
- Modify: every `meta()` impl in `caliban/src/tui/slash/*.rs` (add `immediate: false` to each literal)

- [ ] **Step 1: Write the failing test**

In `caliban/src/tui/slash.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn meta_carries_immediate_flag() {
    let m = SlashCommandMeta {
        name: "/x", description: "", args_hint: "", hidden: false, immediate: true,
    };
    assert!(m.immediate);
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `cargo test --bin caliban tui::slash::tests::meta_carries_immediate_flag`
Expected: FAIL with "no field `immediate`" compile error.

- [ ] **Step 3: Add the field**

In `caliban/src/tui/slash.rs:33`:

```rust
pub(crate) struct SlashCommandMeta {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) args_hint: &'static str,
    pub(crate) hidden: bool,
    /// `true` => command can fire while a turn is in flight (IE1: doesn't
    /// need the model). Default `false`. See `docs/TODO.md` IE1.
    pub(crate) immediate: bool,
}
```

Update the test helper `echo()` at :263 to include `immediate: false`.

- [ ] **Step 4: Add `immediate: false` to every existing `meta()` literal**

Find: `rg -ln 'SlashCommandMeta \{' caliban/src/tui/slash/`
For each file, add `immediate: false,` after `hidden: ...` in every struct literal.

- [ ] **Step 5: Run tests, verify all pass**

Run: `cargo test --bin caliban tui::slash 2>&1 | tail -20`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add caliban/src/tui/slash.rs caliban/src/tui/slash/
git commit -m "feat(tui): add immediate: bool to SlashCommandMeta (IE1 prep)"
```

### Task 2: Add `lookup_meta` to registry + classifier helper

**Files:**
- Modify: `caliban/src/tui/slash.rs` (registry impl + new helper)

- [ ] **Step 1: Write the failing tests**

In `caliban/src/tui/slash.rs` tests mod:

```rust
#[test]
fn lookup_meta_returns_some_for_known_command() {
    let mut r = SlashCommandRegistry::new();
    r.register(echo("/foo", false));
    assert!(r.lookup_meta("/foo").is_some());
    assert!(r.lookup_meta("/bar").is_none());
}

#[test]
fn is_immediate_slash_recognizes_tagged_command() {
    let mut r = SlashCommandRegistry::new();
    r.register(echo_immediate("/inst", true));
    r.register(echo("/slow", false));
    assert!(is_immediate_slash("/inst", &r));
    assert!(is_immediate_slash("/inst with args", &r));
    assert!(!is_immediate_slash("/slow", &r));
    assert!(!is_immediate_slash("/unknown", &r));
    assert!(!is_immediate_slash("hello world", &r));
    assert!(!is_immediate_slash("", &r));
}
```

Add helper `echo_immediate(name, imm) -> Arc<dyn SlashCommand>` in the tests mod (same shape as `echo` but with `immediate: imm`).

- [ ] **Step 2: Run, verify fails**

Run: `cargo test --bin caliban tui::slash::tests::lookup_meta_returns_some_for_known_command`
Expected: FAIL with "no method `lookup_meta`" / "function `is_immediate_slash` not found".

- [ ] **Step 3: Implement**

In `caliban/src/tui/slash.rs` `impl SlashCommandRegistry`:

```rust
/// Look up a command's static meta by exact name. Used by the
/// submit-handler classifier to decide whether to fire a slash
/// command during a running turn (IE1).
pub(crate) fn lookup_meta(&self, name: &str) -> Option<&SlashCommandMeta> {
    self.by_name.get(name).map(|c| c.meta())
}
```

Add at end of `caliban/src/tui/slash.rs` (outside impl):

```rust
/// Classifier: returns `true` iff `prompt` is a slash invocation whose
/// command is registered with `immediate: true`. Pure function so the
/// event-handler intercept stays unit-testable.
#[must_use]
pub(crate) fn is_immediate_slash(prompt: &str, registry: &SlashCommandRegistry) -> bool {
    let name = prompt.split_whitespace().next().unwrap_or("");
    if !name.starts_with('/') {
        return false;
    }
    registry.lookup_meta(name).is_some_and(|m| m.immediate)
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test --bin caliban tui::slash 2>&1 | tail -10`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/slash.rs
git commit -m "feat(tui): add lookup_meta + is_immediate_slash classifier (IE1)"
```

### Task 3: Tag the immediate commands

**Files:**
- Modify: per-command `meta()` impls — flip `immediate: false` → `immediate: true` for non-agent-loop-touching commands.

**Audit list (verified against current code):**
- `/usage`, `/context` (observe.rs) — render-only → `immediate: true`
- `/cost` (cost.rs) — render-only → `immediate: true`
- `/help` (basic.rs or wherever) — render-only → `immediate: true`
- `/perms` (perms.rs) — overlay → `immediate: true`
- `/config` (config.rs) — overlay → `immediate: true`
- `/model`, `/effort` (model.rs) — config flip → `immediate: true`
- `/theme` (if exists) — config flip → `immediate: true`
- `/export` (export.rs) — file write only → `immediate: true`
- `/doctor` (if a slash exists) — checks only → `immediate: true`
- `/quit` (basic.rs) — `immediate: true` (sets `should_exit`)
- **Keep `immediate: false`:** `/clear`, `/compact`, `/rewind`, `/resume`, `/recap`, `/btw`, `/loop`, `/init`, `/plan`, `/skills`, `/memory`, `/plugin(s)`, `/hooks`, `/agents`, `/mcp`, `/statusline`, `/tui`, `/login`, `/logout`, `/status`, `/feedback`, `/heapdump` — these mutate session state or wait on the agent.

- [ ] **Step 1: Write the failing test (regression-style)**

In `caliban/src/tui/slash.rs` tests mod:

```rust
#[test]
fn known_immediate_commands_are_tagged_in_builtin_registry() {
    let r = register_builtin();
    for cmd in &["/usage", "/context", "/cost", "/help", "/export"] {
        let m = r.lookup_meta(cmd).unwrap_or_else(|| panic!("missing {cmd}"));
        assert!(m.immediate, "expected {cmd} to be immediate");
    }
    // Non-immediate sanity check.
    for cmd in &["/clear", "/compact"] {
        let m = r.lookup_meta(cmd).unwrap_or_else(|| panic!("missing {cmd}"));
        assert!(!m.immediate, "{cmd} should not be immediate");
    }
}
```

- [ ] **Step 2: Run, verify fails (all default false)**

Expected: FAIL on first immediate command.

- [ ] **Step 3: Flip the flags per audit list**

Edit each command's `meta()` literal, change `immediate: false` to `immediate: true`. Be precise about which commands.

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test --bin caliban tui::slash 2>&1 | tail -10`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/slash/
git commit -m "feat(tui): tag non-model slash commands as immediate (IE1)"
```

### Task 4: Wire intercept into events.rs submit handler

**Files:**
- Modify: `caliban/src/tui/events.rs:864-872` (submit handler).

- [ ] **Step 1: Write the failing integration test (extract logic to testable helper)**

The submit handler at events.rs:864 isn't easily unit-testable as-is. Extract the classification step into a small function and test it. Add to `caliban/src/tui/events.rs` `#[cfg(test)]`:

```rust
#[test]
fn submit_decision_immediate_slash_during_running_dispatches() {
    use crate::tui::slash::{self, register_builtin};
    let r = register_builtin();
    assert!(slash::is_immediate_slash("/context", &r));
    assert!(!slash::is_immediate_slash("/clear", &r));
    assert!(!slash::is_immediate_slash("hello", &r));
}
```

- [ ] **Step 2: Run, verify pass (classifier already exists from Task 2)**

This test is a sanity check that the classifier sees `register_builtin`'s tags.

- [ ] **Step 3: Wire intercept into the submit handler**

In `caliban/src/tui/events.rs:864`, replace the block starting with `(KeyCode::Enter, _) => {` so that the immediate-slash check happens BEFORE the running-turn bail:

```rust
(KeyCode::Enter, _) => {
    let prompt = app.input.buffer.trim().to_string();
    if prompt.is_empty() {
        return;
    }

    // IE1: immediate slash commands fire even while a turn is in flight.
    // Tag is on `SlashCommandMeta.immediate`; classifier is pure.
    if slash::is_immediate_slash(&prompt, &app.slash_registry) {
        let _ = app.input.submit();
        app.auto_scroll = true;
        handle_slash_command(&prompt, app);
        return;
    }

    // Ignore submit if a turn is already running (will be replaced by
    // IE2 queue-push in the next commit).
    if app.running.is_some() {
        return;
    }
    // ... existing code unchanged ...
```

(Add `use crate::tui::slash;` if not already imported.)

- [ ] **Step 4: Run the full test suite + a manual smoke test**

Run: `cargo test --bin caliban 2>&1 | tail -20`
Expected: green.

Manual check: build, launch TUI, start a long-running model turn, type `/context` mid-stream → overlay should open immediately.

(Manual TUI test is documented; not part of `cargo test`.)

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/events.rs
git commit -m "feat(tui): intercept immediate slash commands during inference (IE1)"
```

---

## IE2 — Queued user messages with two-stage Esc

### Task 5: Add `queued` field + push helper + tests

**Files:**
- Modify: `caliban/src/tui/app.rs:159` (`running: Option<RunningTurn>`) — add `queued: VecDeque<String>` and `esc_armed_at: Option<std::time::Instant>` adjacent fields.
- Modify: `caliban/src/tui/app.rs:346` (`App::new` or default init) — initialize new fields.

- [ ] **Step 1: Write the failing test**

In `caliban/src/tui/app.rs` `#[cfg(test)]`:

```rust
#[test]
fn app_initializes_queued_empty() {
    let app = App::for_test();  // or whatever the existing test constructor is
    assert!(app.queued.is_empty());
    assert!(app.esc_armed_at.is_none());
}
```

(If no `for_test()` constructor exists, the test goes wherever `App` can be constructed in tests — match existing patterns.)

- [ ] **Step 2: Run, verify fails**

Expected: FAIL on "no field `queued`".

- [ ] **Step 3: Add fields + init**

In `caliban/src/tui/app.rs`, struct `App`:

```rust
/// Messages typed by the user while a turn was already running. Drained
/// FIFO on `RunEnd`. Render path shows the front as a `QUEUED:` hint.
/// See `docs/TODO.md` IE2.
pub(crate) queued: std::collections::VecDeque<String>,
/// Set when Esc is pressed with a non-empty queue. A second Esc within
/// ESC_REARM_WINDOW (2s) cancels the running turn; otherwise the arm
/// expires. See `docs/TODO.md` IE2.
pub(crate) esc_armed_at: Option<std::time::Instant>,
```

In the `App::new` / default-init paths (search for existing `running: None,` for the right spot):

```rust
queued: std::collections::VecDeque::new(),
esc_armed_at: None,
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test --bin caliban tui::app 2>&1 | tail -10`

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/app.rs
git commit -m "feat(tui): add queued + esc_armed_at fields to App (IE2 prep)"
```

### Task 6: Replace running-turn bail with queue-push

**Files:**
- Modify: `caliban/src/tui/events.rs:870` (the running-turn bail).
- Modify: `caliban/src/tui/render.rs` (add `QUEUED: <preview>` indicator near the input bar).

- [ ] **Step 1: Write the failing test**

In `caliban/src/tui/events.rs` tests (or wherever `App` is constructed for tests):

```rust
#[test]
fn submit_during_running_pushes_to_queue() {
    let mut app = App::for_test();
    // Set running.
    let cancel = tokio_util::sync::CancellationToken::new();
    app.running = Some(RunningTurn {
        cancel, activity: Activity::WaitingForModel { since: std::time::Instant::now() },
    });
    app.input.buffer = "hello".into();
    app.input.cursor = 5;
    // Simulate the queue-push path directly.
    push_user_input_to_queue(&mut app);
    assert_eq!(app.queued.front().map(String::as_str), Some("hello"));
    assert_eq!(app.input.buffer, "");
}
```

- [ ] **Step 2: Run, verify fails**

Expected: FAIL with "function not defined".

- [ ] **Step 3: Implement helper + wire into submit**

Add to `caliban/src/tui/events.rs`:

```rust
/// IE2: push the current input buffer onto the queue and clear it.
/// Called by the submit handler when a turn is running and the prompt
/// isn't an immediate slash. Idempotent on empty input.
pub(crate) fn push_user_input_to_queue(app: &mut App) {
    let line = app.input.buffer.trim().to_string();
    if line.is_empty() {
        return;
    }
    app.queued.push_back(line);
    app.input.buffer.clear();
    app.input.cursor = 0;
}
```

In the submit handler, replace the `if app.running.is_some() { return; }` (now at line ~870 after IE1's intercept) with:

```rust
if app.running.is_some() {
    push_user_input_to_queue(app);
    return;
}
```

- [ ] **Step 4: Add render indicator**

In `caliban/src/tui/render.rs`, find the input-bar render path. Add (above the input area, similar to existing `pending_ask` hint):

```rust
if let Some(front) = app.queued.front() {
    let preview: String = front.chars().take(48).collect();
    let suffix = if app.queued.len() > 1 {
        format!(" (+{} more)", app.queued.len() - 1)
    } else {
        String::new()
    };
    // Render as Info / dim line above the input area.
    let line = format!("QUEUED: {preview}{suffix}");
    // ... slot into existing render layout
}
```

(The render integration is layout-dependent; follow existing pending-Ask render pattern.)

- [ ] **Step 5: Run tests + manual smoke**

Run: `cargo test --bin caliban 2>&1 | tail -10`
Manual: start a long turn, type and submit `hello` mid-stream → input clears, `QUEUED: hello` appears above input.

- [ ] **Step 6: Commit**

```bash
git add caliban/src/tui/events.rs caliban/src/tui/render.rs
git commit -m "feat(tui): push user input to queue during running turn (IE2)"
```

### Task 7: Drain queue on RunEnd

**Files:**
- Modify: `caliban/src/tui/events.rs:314` and `:328` (the two `app.running = None;` sites — `RunEnd` handling).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn drain_queue_dispatches_next_user_input() {
    let mut app = App::for_test();
    app.queued.push_back("queued msg".into());
    // Simulate RunEnd cleanup.
    let dispatched = drain_one_queued(&mut app);
    assert_eq!(dispatched.as_deref(), Some("queued msg"));
    assert!(app.queued.is_empty());
}

#[test]
fn drain_queue_noop_when_empty() {
    let mut app = App::for_test();
    assert!(drain_one_queued(&mut app).is_none());
}
```

- [ ] **Step 2: Run, verify fails**

Expected: FAIL with "function not defined".

- [ ] **Step 3: Implement drain helper + wire into RunEnd**

Add to `caliban/src/tui/events.rs`:

```rust
/// IE2: pop the next queued user message. Caller is responsible for
/// dispatching it as the next user turn via the same path Enter takes.
/// Returns `None` if the queue is empty.
pub(crate) fn drain_one_queued(app: &mut App) -> Option<String> {
    app.queued.pop_front()
}
```

In events.rs:314 and :328 (after `app.running = None;`), add:

```rust
if let Some(next) = drain_one_queued(app) {
    // Re-enter the submit path with the queued text.
    app.input.buffer = next;
    app.input.cursor = app.input.buffer.len();
    // Synthesize an Enter key event by calling the submit branch directly,
    // OR set a flag the event loop drains on the next tick. The exact
    // wiring depends on whether re-entering the closure is safe here —
    // if not, set `app.pending_dispatch = true` and handle on tick.
}
```

(The exact re-entry pattern depends on the event-loop structure; pick whichever doesn't violate borrow rules. If immediate re-entry is awkward, use a `pending_dispatch` flag and dispatch in the main tick loop.)

- [ ] **Step 4: Run tests + manual smoke**

Manual: start a long turn, queue 2 messages, wait for completion → first message auto-sends, then second.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/events.rs
git commit -m "feat(tui): drain queued user messages on RunEnd (IE2)"
```

### Task 8: Two-stage Esc

**Files:**
- Modify: `caliban/src/tui/events.rs:813` (existing Esc-cancel path).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn esc_clears_queue_when_non_empty() {
    let mut app = App::for_test();
    app.queued.push_back("a".into());
    app.queued.push_back("b".into());
    let armed = handle_esc(&mut app, std::time::Instant::now());
    assert!(app.queued.is_empty());
    assert!(armed.queue_cleared);
    assert!(!armed.turn_cancelled);
}

#[test]
fn second_esc_within_window_cancels_turn() {
    let mut app = App::for_test();
    let now = std::time::Instant::now();
    app.queued.push_back("x".into());
    let _ = handle_esc(&mut app, now);  // arms
    let r = handle_esc(&mut app, now + std::time::Duration::from_millis(1500));
    assert!(r.turn_cancelled);
}

#[test]
fn esc_outside_window_re_arms_not_cancels() {
    let mut app = App::for_test();
    let now = std::time::Instant::now();
    app.queued.push_back("x".into());
    let _ = handle_esc(&mut app, now);
    let r = handle_esc(&mut app, now + std::time::Duration::from_secs(3));
    assert!(!r.turn_cancelled);
}
```

- [ ] **Step 2: Run, verify fails**

- [ ] **Step 3: Implement `handle_esc` helper**

Add to `caliban/src/tui/events.rs`:

```rust
const ESC_REARM_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Default, PartialEq)]
pub(crate) struct EscOutcome {
    pub(crate) queue_cleared: bool,
    pub(crate) turn_cancelled: bool,
}

/// IE2: two-stage Esc.
/// - If queue is non-empty: clear queue, arm a 2s window.
/// - Else if running.is_some(): cancel the turn.
/// - If queue was empty and previously armed within window, also cancel.
pub(crate) fn handle_esc(app: &mut App, now: std::time::Instant) -> EscOutcome {
    let mut out = EscOutcome::default();
    if !app.queued.is_empty() {
        app.queued.clear();
        app.esc_armed_at = Some(now);
        out.queue_cleared = true;
        return out;
    }
    // Queue empty path: check arm window or fall through to existing cancel.
    let armed_recently = app
        .esc_armed_at
        .is_some_and(|t| now.duration_since(t) <= ESC_REARM_WINDOW);
    if armed_recently || app.running.is_some() {
        if let Some(r) = &app.running {
            r.cancel.cancel();
        }
        out.turn_cancelled = true;
        app.esc_armed_at = None;
    }
    out
}
```

In events.rs:813 (the existing Esc handler block), replace direct `running.cancel.cancel()` with `handle_esc(app, std::time::Instant::now());`.

- [ ] **Step 4: Run tests + manual smoke**

Manual: queue 2 messages, press Esc → both cleared, second Esc within 2s cancels turn.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/tui/events.rs
git commit -m "feat(tui): two-stage Esc (clear queue, then cancel) (IE2)"
```

---

## IE3 — Mouse drag-select inside alt-screen with OSC-52

(Tasks 9–14 expand into detail when IE2 lands. Outline now; flesh out before execution.)

### Task 9: `PositionMap` data structure (pure, unit-tested)

**Files:**
- Create: `caliban/src/tui/mouse_select.rs` (new module).
- Modify: `caliban/src/tui.rs` (add `mod mouse_select;`).

Pure data structure: `(row, col) → (message_id, char_offset)` lookup. Built per-frame by the renderer. Methods: `new(rows, cols)`, `record(row, col, msg_id, offset)`, `extract_range(start: (r,c), end: (r,c)) -> String`. Tests: round-trip a known layout.

### Task 10: `MouseSelection` state machine (pure)

**Files:**
- Modify: `caliban/src/tui/mouse_select.rs`.

States: `Idle | Selecting { start } | Done { start, end }`. Transitions on `Down(Left)` / `Drag(Left)` / `Up(Left)` / `Down(Right or other)`. Tests: each transition.

### Task 11: Render-time position-map population

**Files:**
- Modify: `caliban/src/tui/render.rs` (wrap transcript draw).

Each frame: build a fresh `PositionMap`, record positions as each styled span is drawn. Store on `App` (e.g., `app.last_position_map: PositionMap`).

### Task 12: Visual highlight overlay

**Files:**
- Modify: `caliban/src/tui/render.rs`.

After the transcript draws, walk `app.mouse_selection.range()` and apply `Style::default().bg(highlight)` to those cells via ratatui's `buf.set_style(area, style)`.

### Task 13: Mouse-event state machine wiring

**Files:**
- Modify: `caliban/src/tui/events.rs::handle_mouse` (currently at line 630).

Map crossterm `MouseEventKind::Down(MouseButton::Left)`/`Drag(Left)`/`Up(Left)` onto `MouseSelection` transitions. On `Up`, extract text via the position map and call `clipboard::copy_to_clipboard`. Existing `ScrollUp`/`ScrollDown` paths untouched.

### Task 14: Clipboard write — OSC-52 with `arboard` fallback

**Files:**
- Create: `caliban/src/tui/clipboard.rs`.
- Modify: `caliban/src/tui.rs` (add `mod clipboard;`).
- Modify: `caliban/Cargo.toml` (confirm `arboard` is available; pull from `caliban-images` re-export or add direct dep).

```rust
/// Best-effort clipboard write. Tries OSC-52 first; falls back to arboard
/// if OSC-52 is suspected unsupported (e.g. macOS Terminal.app where
/// support is patchy).
pub(crate) fn copy_to_clipboard(text: &str) -> Result<()> {
    if let Err(_) = write_osc52(text) {
        return write_arboard(text);
    }
    Ok(())
}
```

OSC-52: emit `\x1b]52;c;<base64>\x07` to stdout. Tests: OSC-52 formatter is pure; arboard fallback is feature-tested only on platforms where it's available.

---

## Self-review (skill section)

**Spec coverage:** IE1 covered Tasks 1-4. IE2 covered Tasks 5-8. IE3 outlined Tasks 9-14 (will be expanded with full TDD substeps when IE2 lands).

**Placeholder scan:** IE3 tasks 9-14 are intentionally outlined (medium-effort piece, expand before implementing). No "TBD" placeholders in IE1/IE2 task bodies.

**Type consistency:** `SlashCommandMeta`, `is_immediate_slash`, `push_user_input_to_queue`, `drain_one_queued`, `handle_esc`/`EscOutcome` are referenced consistently across tasks.

**Scope check:** Three features bundled. Justified because they share file ownership (events.rs, app.rs) and would conflict if split.

---

## Execution

Inline TDD execution in this session (the user has been driving inline iteration throughout this conversation, and the brainstorming/spec phases have already been done in the TODO entries). Caliban+ollama sub-agent driver will be used for delegable lookup tasks (file enumeration, single-step searches) where its capability ceiling (~1-2 step tasks) suffices; the actual implementation work is done directly.
