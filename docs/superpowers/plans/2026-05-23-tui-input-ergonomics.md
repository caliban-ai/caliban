# TUI Input Ergonomics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add multi-line composition, slash-command autocomplete, and live @-path completion (with file auto-attach + refuse-with-hint oversize policy) to the caliban TUI.

**Architecture:** Lift the input area's flat fields (`input: String`, `cursor`, `history`, `history_index`) into a single `Input` struct with an `InputMode` enum that drives both rendering and key dispatch. Add three new submodules under `caliban/src/tui/`: `input`, `completer`, `attach`, plus a `toast` primitive that future slices reuse. Path completion uses on-demand `ignore::WalkBuilder::new(dir).max_depth(Some(1))` — no workspace-wide index.

**Tech Stack:** Rust 2024, ratatui 0.29, crossterm (with kitty keyboard protocol), tokio, `ignore` (already present), `nucleo-matcher` (new), `tempfile` (existing dev-dep).

**Spec:** `docs/superpowers/specs/2026-05-23-tui-input-ergonomics-design.md`

---

## File Structure

After implementation, `caliban/src/tui.rs` (currently the only file) becomes a parent module with sibling files. We keep `tui.rs` as the entry point (Rust 2018+ idiom) and add submodule files under `caliban/src/tui/`:

| File | Responsibility | Approx LOC |
|---|---|---|
| `caliban/src/tui.rs` | App struct, render loop, key dispatch glue. Existing file, slimmer after extractions. | ~1400 (was ~1620) |
| `caliban/src/tui/input.rs` | `Input`, `InputMode`, `MenuState`, `Candidate`, key-action helpers. | ~400 |
| `caliban/src/tui/completer.rs` | Fuzzy matcher wrapper for slash and @-path candidates. | ~120 |
| `caliban/src/tui/attach.rs` | `resolve_attachments`, `Attachment`, `AttachError`, wire-format builder. | ~250 |
| `caliban/src/tui/toast.rs` | `Toast`, `ToastLevel`, render helper. | ~80 |
| `caliban/src/main.rs` | Two new CLI flags + env fallbacks. | +20 |
| `Cargo.toml` (workspace) | Add `nucleo-matcher = "0.3"`. | +1 |
| `caliban/Cargo.toml` | Wire workspace dep. | +1 |

---

## Task 1: Workspace dep + module scaffolding

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `caliban/Cargo.toml`
- Create: `caliban/src/tui/input.rs` (stub)
- Create: `caliban/src/tui/completer.rs` (stub)
- Create: `caliban/src/tui/attach.rs` (stub)
- Create: `caliban/src/tui/toast.rs` (stub)
- Modify: `caliban/src/tui.rs` (add `mod` declarations)

- [ ] **Step 1: Add `nucleo-matcher` to the workspace**

Edit `Cargo.toml` workspace dependencies (alphabetical-ish, drop near other matchers):

```toml
[workspace.dependencies]
# ... existing entries ...
nucleo-matcher = "0.3"
```

- [ ] **Step 2: Wire dep into `caliban/Cargo.toml`**

Add to `[dependencies]`:

```toml
nucleo-matcher = { workspace = true }
```

- [ ] **Step 3: Create four empty submodule stubs**

Each file starts with a module-level doc comment so `missing_docs` lint passes:

`caliban/src/tui/input.rs`:
```rust
//! Input state machine for the TUI prompt area.
```

`caliban/src/tui/completer.rs`:
```rust
//! Fuzzy match candidates for slash and @-path menus.
```

`caliban/src/tui/attach.rs`:
```rust
//! Resolve `@path` tokens to file attachments at submit time.
```

`caliban/src/tui/toast.rs`:
```rust
//! Ephemeral one-row notification rendered above the input area.
```

- [ ] **Step 4: Declare submodules in `caliban/src/tui.rs`**

At the top, just below the existing `//! Ratatui-based interactive TUI.` and `#![allow(...)]`:

```rust
mod attach;
mod completer;
mod input;
mod toast;
```

- [ ] **Step 5: Verify the workspace still builds**

Run: `cargo build --workspace`
Expected: clean build, no new warnings.

- [ ] **Step 6: Verify tests still pass**

Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml caliban/Cargo.toml caliban/src/tui.rs caliban/src/tui/
git -c commit.gpgsign=false commit -m "tui: scaffold input/completer/attach/toast submodules + nucleo-matcher dep"
```

---

## Task 2: Extract `Input` struct (refactor, no behaviour change)

Pull the four scattered input fields on `App` (`input`, `cursor`, `history`, `history_index`) into a single struct with the same observable behavior.

**Files:**
- Modify: `caliban/src/tui/input.rs`
- Modify: `caliban/src/tui.rs` (App + handlers)

- [ ] **Step 1: Write failing tests for `Input`**

Replace the stub in `caliban/src/tui/input.rs`:

```rust
//! Input state machine for the TUI prompt area.

/// Single-line input buffer with cursor and ↑/↓ history navigation.
#[derive(Debug, Default)]
pub(crate) struct Input {
    pub(crate) buffer: String,
    pub(crate) cursor: usize,
    pub(crate) history: Vec<String>,
    pub(crate) history_cursor: Option<usize>,
}

impl Input {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn from_history(history: Vec<String>) -> Self {
        Self {
            history,
            ..Self::default()
        }
    }

    pub(crate) fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub(crate) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.buffer[..self.cursor]
            .chars()
            .next_back()
            .map_or(0, char::len_utf8);
        self.cursor -= prev;
        self.buffer.drain(self.cursor..self.cursor + prev);
    }

    pub(crate) fn delete(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        let next = self.buffer[self.cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        self.buffer.drain(self.cursor..self.cursor + next);
    }

    pub(crate) fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.buffer[..self.cursor]
            .chars()
            .next_back()
            .map_or(0, char::len_utf8);
        self.cursor -= prev;
    }

    pub(crate) fn move_right(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        let next = self.buffer[self.cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        self.cursor += next;
    }

    pub(crate) fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn move_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    pub(crate) fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.history_cursor = None;
    }

    pub(crate) fn submit(&mut self) -> String {
        let line = std::mem::take(&mut self.buffer);
        self.cursor = 0;
        self.history.push(line.clone());
        self.history_cursor = None;
        line
    }

    pub(crate) fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let new_idx = match self.history_cursor {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_cursor = Some(new_idx);
        self.buffer = self.history[new_idx].clone();
        self.cursor = self.buffer.len();
    }

    pub(crate) fn history_next(&mut self) {
        let Some(idx) = self.history_cursor else {
            return;
        };
        if idx + 1 >= self.history.len() {
            self.history_cursor = None;
            self.buffer.clear();
            self.cursor = 0;
        } else {
            self.history_cursor = Some(idx + 1);
            self.buffer = self.history[idx + 1].clone();
            self.cursor = self.buffer.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_cursor_advance() {
        let mut i = Input::new();
        i.insert_char('a');
        i.insert_char('b');
        assert_eq!(i.buffer, "ab");
        assert_eq!(i.cursor, 2);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut i = Input::new();
        i.backspace();
        assert_eq!(i.buffer, "");
        assert_eq!(i.cursor, 0);
    }

    #[test]
    fn backspace_handles_multibyte() {
        let mut i = Input::new();
        i.insert_char('é');
        i.backspace();
        assert_eq!(i.buffer, "");
        assert_eq!(i.cursor, 0);
    }

    #[test]
    fn submit_pushes_history_and_clears() {
        let mut i = Input::new();
        i.insert_char('x');
        let out = i.submit();
        assert_eq!(out, "x");
        assert_eq!(i.buffer, "");
        assert_eq!(i.history, vec!["x".to_string()]);
    }

    #[test]
    fn history_prev_next_round_trip() {
        let mut i = Input::from_history(vec!["one".into(), "two".into()]);
        i.history_prev();
        assert_eq!(i.buffer, "two");
        i.history_prev();
        assert_eq!(i.buffer, "one");
        i.history_next();
        assert_eq!(i.buffer, "two");
        i.history_next();
        assert_eq!(i.buffer, "");
        assert_eq!(i.history_cursor, None);
    }
}
```

- [ ] **Step 2: Run the new tests**

Run: `cargo test -p caliban tui::input`
Expected: 5 tests pass.

- [ ] **Step 3: Migrate `App` to use `Input`**

In `caliban/src/tui.rs`:

(a) Replace these `App` fields:

```rust
    pub(crate) input: String,
    pub(crate) cursor: usize,
    pub(crate) history: Vec<String>,
    pub(crate) history_index: Option<usize>,
```

with:

```rust
    pub(crate) input: input::Input,
```

(b) In `App::new`, replace the four initializers with:

```rust
            input: input::Input::from_history(history),
```

(remove the now-unused local `history` binding once it's been moved into `Input::from_history`).

(c) Replace each direct field access. The relevant call sites and their rewrites:

| Old | New |
|---|---|
| `app.input` (read) | `app.input.buffer` (read) or `&app.input.buffer` |
| `app.input.clear()` | `app.input.clear()` (Input has its own `clear`) |
| `app.cursor` | `app.input.cursor` |
| `app.history` | `app.input.history` |
| `app.history_index` | `app.input.history_cursor` |
| `app.insert_char(c)` | `app.input.insert_char(c)` |
| `app.backspace()` | `app.input.backspace()` |
| `app.delete()` | `app.input.delete()` |
| `app.move_left()` | `app.input.move_left()` |
| `app.move_right()` | `app.input.move_right()` |
| `app.move_home()` | `app.input.move_home()` |
| `app.move_end()` | `app.input.move_end()` |
| `app.history_prev()` | `app.input.history_prev()` |
| `app.history_next()` | `app.input.history_next()` |

(d) Delete the old `impl App` methods that are now in `Input`: `insert_char`, `backspace`, `delete`, `move_left`, `move_right`, `move_home`, `move_end`, `history_prev`, `history_next`.

(e) Inline the submit-on-Enter path: replace the existing

```rust
let line = app.input.clone();
app.input.clear();
app.cursor = 0;
app.history.push(line);
app.history_index = None;
```

with:

```rust
let line = app.input.submit();
```

and use `line` everywhere the old `prompt` was referenced for the rest of that handler. (If `line` and `prompt` are both used as separate names, unify them — they're the same value.)

- [ ] **Step 4: Build**

Run: `cargo build --workspace`
Expected: clean build. If a field access was missed, the compiler will report it precisely — fix and re-run.

- [ ] **Step 5: Verify existing tests still pass**

Run: `cargo test --workspace`
Expected: all green (the existing TUI tests must keep passing — this is a behavior-preserving refactor).

- [ ] **Step 6: Commit**

```bash
git add caliban/src/tui.rs caliban/src/tui/input.rs
git -c commit.gpgsign=false commit -m "tui: extract Input state into tui::input (refactor, no behavior change)"
```

---

## Task 3: Multi-line composition (Shift+Enter, Alt+Enter, kitty proto)

Newlines now insertable; render layer already wraps lines per char (verified by the existing `wrap_lines_to_width` helper and the input-row computation in `render`). The render path already counts `app.input.chars()` which includes `\n` — confirm with a manual run.

**Files:**
- Modify: `caliban/src/tui/input.rs` (add `insert_newline`)
- Modify: `caliban/src/tui.rs` (Shift+Enter / Alt+Enter dispatch, kitty proto, input-row count)

- [ ] **Step 1: Test `insert_newline`**

In `caliban/src/tui/input.rs` test module, append:

```rust
    #[test]
    fn insert_newline_inserts_lf_at_cursor() {
        let mut i = Input::new();
        i.insert_char('a');
        i.insert_char('b');
        i.move_left();
        i.insert_newline();
        assert_eq!(i.buffer, "a\nb");
        assert_eq!(i.cursor, 2);
    }
```

- [ ] **Step 2: Run and verify it fails**

Run: `cargo test -p caliban tui::input::tests::insert_newline_inserts_lf_at_cursor`
Expected: FAIL — `insert_newline` not defined.

- [ ] **Step 3: Implement `insert_newline`**

Add to `impl Input`:

```rust
    pub(crate) fn insert_newline(&mut self) {
        self.buffer.insert(self.cursor, '\n');
        self.cursor += 1;
    }
```

- [ ] **Step 4: Re-run; expect PASS**

Run: `cargo test -p caliban tui::input::tests::insert_newline_inserts_lf_at_cursor`
Expected: PASS.

- [ ] **Step 5: Wire Shift+Enter and Alt+Enter into key dispatch**

In `caliban/src/tui.rs`, find the `KeyCode::Enter` arm inside `handle_key`. Today it unconditionally submits. Change to:

```rust
KeyCode::Enter => {
    if key.modifiers.contains(KeyModifiers::SHIFT)
        || key.modifiers.contains(KeyModifiers::ALT)
    {
        app.input.insert_newline();
        return;
    }
    // ... existing submit body unchanged ...
}
```

- [ ] **Step 6: Enable kitty keyboard protocol on terminal entry**

Edit `TerminalGuard::enter` in `caliban/src/tui.rs`:

Add the import:

```rust
use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags, PopKeyboardEnhancementFlags};
```

Update the body:

```rust
pub(crate) fn enter() -> Result<Self> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
        ),
    )?;
    let backend = CrosstermBackend::new(out);
    let terminal = Terminal::new(backend)?;
    Ok(Self { terminal })
}
```

And pop the flags on Drop, BEFORE leaving the alternate screen:

```rust
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            stdout(),
            PopKeyboardEnhancementFlags,
            DisableMouseCapture,
            LeaveAlternateScreen,
        );
        let _ = disable_raw_mode();
    }
}
```

Note: `Push/Pop` are no-ops on terminals that don't support kitty proto, so this is safe to enable unconditionally.

- [ ] **Step 7: Ensure multi-line input still draws cleanly**

The existing `render` function computes `input_rows` as `(PROMPT_CHARS + app.input.buffer.chars().count()).div_ceil(avail_width).max(1)`. With newlines, this undercounts.

Find the existing `total_input_chars` computation in `render` and replace with this newline-aware version:

```rust
// chars-per-line including the leading prompt on the FIRST visible row
let mut total_rows: usize = 0;
let prompt_chars = PROMPT_CHARS;
for (i, segment) in app.input.buffer.split('\n').enumerate() {
    let chars = segment.chars().count() + if i == 0 { prompt_chars } else { 0 };
    if avail_width == 0 {
        total_rows += 1;
    } else {
        total_rows += chars.div_ceil(avail_width).max(1);
    }
}
let input_rows: u16 =
    u16::try_from(total_rows.clamp(1, INPUT_MAX_ROWS as usize)).unwrap_or(INPUT_MAX_ROWS);
```

(Replace the old `let total_input_chars = ...; let input_rows = ...;` block; keep `INPUT_MAX_ROWS` as-is.)

Also update the manual character-wrap loop that produces the input area lines (search for `let mut s = String::with_capacity(PROMPT_CHARS + app.input.len())`) so that it splits on `\n` first and prepends the prompt only on the first segment.

The minimal patch in that block:

```rust
let input_chunk_width = chunks[2].width as usize;
if input_chunk_width > 0 {
    let avail = input_chunk_width.saturating_sub(0); // no border on input area
    let mut visual_rows: Vec<String> = Vec::new();
    for (i, segment) in app.input.buffer.split('\n').enumerate() {
        let mut s = String::new();
        if i == 0 {
            s.push_str("> ");
        }
        s.push_str(segment);
        // char-wrap each logical line independently
        let mut chars = s.chars().peekable();
        let mut row = String::new();
        let mut row_width: usize = 0;
        while let Some(c) = chars.next() {
            row.push(c);
            row_width += 1;
            if row_width >= avail {
                visual_rows.push(std::mem::take(&mut row));
                row_width = 0;
            }
        }
        visual_rows.push(row);
    }
    let lines: Vec<Line> = visual_rows.into_iter().map(Line::raw).collect();
    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, chunks[2]);
}
```

(The existing implementation does roughly the same character-wrap; this version wraps per logical line. Match the surrounding code's style if it differs.)

- [ ] **Step 8: Build**

Run: `cargo build --workspace`
Expected: clean.

- [ ] **Step 9: Run all tests**

Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 10: Manual smoke (DO THIS — don't skip)**

In an interactive terminal (kitty / iTerm2 / Ghostty preferred for Shift+Enter; Terminal.app or tmux for Alt+Enter):

```bash
ANTHROPIC_API_KEY=$YOUR_KEY cargo run -- 
```

Type `hello`, press Shift+Enter, type `world`, press Enter. The user line in the transcript should show two lines (`hello` / `world`).

Repeat with Alt+Enter.

- [ ] **Step 11: Commit**

```bash
git add caliban/src/tui.rs caliban/src/tui/input.rs
git -c commit.gpgsign=false commit -m "feat(tui): multi-line input via Shift+Enter (Alt+Enter fallback)"
```

---

## Task 4: Completer module + slash menu

Add `nucleo-matcher` wrapper and the SlashMenu mode. The menu appears below the input area (between input and status bar) and floats over the transcript.

**Files:**
- Modify: `caliban/src/tui/completer.rs`
- Modify: `caliban/src/tui/input.rs` (add `InputMode`, `MenuState`, `Candidate`)
- Modify: `caliban/src/tui.rs` (render menu, dispatch keys)

- [ ] **Step 1: Implement completer with tests**

Replace the stub at `caliban/src/tui/completer.rs`:

```rust
//! Fuzzy match candidates for slash and @-path menus.

use nucleo_matcher::{Config, Matcher, Utf32String, pattern::{CaseMatching, Normalization, Pattern}};

/// One candidate the user can pick.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    /// What the user sees in the menu (e.g. `"/help"`, `"src/main.rs"`).
    pub(crate) display: String,
    /// What replaces the trigger token when the user picks this candidate.
    pub(crate) insert: String,
    /// Higher = better match.
    pub(crate) score: u32,
}

/// Rank `items` against `query` using nucleo. Empty query => return all in
/// original order with score 0.
pub(crate) fn rank(items: &[(&str, &str)], query: &str, limit: usize) -> Vec<Candidate> {
    if items.is_empty() {
        return Vec::new();
    }
    if query.is_empty() {
        return items
            .iter()
            .take(limit)
            .map(|(d, i)| Candidate {
                display: (*d).to_string(),
                insert: (*i).to_string(),
                score: 0,
            })
            .collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut scored: Vec<Candidate> = items
        .iter()
        .filter_map(|(d, i)| {
            let haystack = Utf32String::from(*d);
            pattern.score(haystack.slice(..), &mut matcher).map(|s| Candidate {
                display: (*d).to_string(),
                insert: (*i).to_string(),
                score: s,
            })
        })
        .collect();
    scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.display.cmp(&b.display)));
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_all_in_order() {
        let items = &[("/help", "/help"), ("/quit", "/quit")];
        let out = rank(items, "", 10);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].display, "/help");
    }

    #[test]
    fn ranks_prefix_matches_highest() {
        let items = &[
            ("/help", "/help"),
            ("/clear", "/clear"),
            ("/config", "/config"),
        ];
        let out = rank(items, "he", 10);
        assert_eq!(out[0].display, "/help");
    }

    #[test]
    fn nonmatch_excluded() {
        let items = &[("/help", "/help")];
        let out = rank(items, "zzz", 10);
        assert!(out.is_empty());
    }

    #[test]
    fn limit_respected() {
        let items = &[("a", "a"), ("ab", "ab"), ("abc", "abc")];
        let out = rank(items, "a", 2);
        assert_eq!(out.len(), 2);
    }
}
```

- [ ] **Step 2: Run completer tests**

Run: `cargo test -p caliban tui::completer`
Expected: 4 pass.

- [ ] **Step 3: Add `InputMode` and `MenuState` to `tui::input`**

Append to `caliban/src/tui/input.rs`:

```rust
use crate::tui::completer::Candidate;

/// Active mode of the input — drives both render and key dispatch.
#[derive(Debug, Default)]
pub(crate) enum InputMode {
    #[default]
    Idle,
    SlashMenu(MenuState),
    AtMenu(MenuState),
}

#[derive(Debug)]
pub(crate) struct MenuState {
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) selected: usize,
    /// Byte offset of the trigger character (`/` or `@`) in `Input::buffer`.
    pub(crate) trigger_start: usize,
}

impl MenuState {
    pub(crate) fn new(trigger_start: usize, candidates: Vec<Candidate>) -> Self {
        Self {
            candidates,
            selected: 0,
            trigger_start,
        }
    }

    pub(crate) fn cycle_next(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.candidates.len();
    }

    pub(crate) fn cycle_prev(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.candidates.len() - 1
        } else {
            self.selected - 1
        };
    }
}
```

And add the field to `Input`:

```rust
#[derive(Debug, Default)]
pub(crate) struct Input {
    pub(crate) buffer: String,
    pub(crate) cursor: usize,
    pub(crate) history: Vec<String>,
    pub(crate) history_cursor: Option<usize>,
    pub(crate) mode: InputMode,
}
```

- [ ] **Step 4: Add slash-menu transition helpers**

Append to `impl Input`:

```rust
    /// Open the slash menu if the buffer is exactly "/".
    pub(crate) fn maybe_open_slash_menu(&mut self, all_commands: &[(&str, &str)]) {
        if matches!(self.mode, InputMode::Idle)
            && self.buffer == "/"
            && self.cursor == 1
        {
            let cands = crate::tui::completer::rank(all_commands, "", 32);
            self.mode = InputMode::SlashMenu(MenuState::new(0, cands));
        }
    }

    /// Refilter the slash menu against the current prefix after the leading `/`.
    pub(crate) fn refilter_slash_menu(&mut self, all_commands: &[(&str, &str)]) {
        if let InputMode::SlashMenu(ref mut menu) = self.mode {
            if self.buffer.starts_with('/') && self.cursor >= 1 {
                let prefix = &self.buffer[1..self.cursor];
                let cands = crate::tui::completer::rank(all_commands, prefix, 32);
                menu.candidates = cands;
                menu.selected = 0;
                return;
            }
            self.mode = InputMode::Idle;
        }
    }

    /// Replace the trigger-anchored prefix with the selected candidate's
    /// `insert` text. Sets mode back to Idle.
    pub(crate) fn accept_menu_selection(&mut self) {
        let (start, end, insert) = match &self.mode {
            InputMode::SlashMenu(m) | InputMode::AtMenu(m) => {
                let cand = match m.candidates.get(m.selected) {
                    Some(c) => c,
                    None => {
                        self.mode = InputMode::Idle;
                        return;
                    }
                };
                let start = m.trigger_start;
                // Find the end of the active token: until next whitespace or EOL.
                let after_trigger = &self.buffer[start..];
                let end_offset = after_trigger
                    .find(|c: char| c.is_whitespace())
                    .unwrap_or(after_trigger.len());
                (start, start + end_offset, cand.insert.clone())
            }
            InputMode::Idle => return,
        };
        self.buffer.replace_range(start..end, &insert);
        self.cursor = start + insert.len();
        self.mode = InputMode::Idle;
    }

    pub(crate) fn close_menu(&mut self) {
        self.mode = InputMode::Idle;
    }
}
```

- [ ] **Step 5: Tests for slash transitions**

Add to the `#[cfg(test)] mod tests` in `tui::input`:

```rust
    #[test]
    fn slash_opens_menu_at_col_zero() {
        let mut i = Input::new();
        i.insert_char('/');
        i.maybe_open_slash_menu(&[("/help", "/help"), ("/quit", "/quit")]);
        assert!(matches!(i.mode, InputMode::SlashMenu(_)));
    }

    #[test]
    fn typing_refilters_slash_menu() {
        let mut i = Input::new();
        i.insert_char('/');
        i.maybe_open_slash_menu(&[("/help", "/help"), ("/quit", "/quit")]);
        i.insert_char('h');
        i.refilter_slash_menu(&[("/help", "/help"), ("/quit", "/quit")]);
        match &i.mode {
            InputMode::SlashMenu(m) => assert_eq!(m.candidates[0].display, "/help"),
            _ => panic!("expected slash menu"),
        }
    }

    #[test]
    fn accept_selection_replaces_token() {
        let mut i = Input::new();
        i.insert_char('/');
        i.maybe_open_slash_menu(&[("/help", "/help")]);
        i.insert_char('h');
        i.refilter_slash_menu(&[("/help", "/help")]);
        i.accept_menu_selection();
        assert_eq!(i.buffer, "/help");
        assert!(matches!(i.mode, InputMode::Idle));
    }
```

- [ ] **Step 6: Run those tests**

Run: `cargo test -p caliban tui::input`
Expected: previous 6 + 3 new = 9 pass.

- [ ] **Step 7: Wire key dispatch**

In `caliban/src/tui.rs`, define the slash command list once (e.g., at module top):

```rust
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help", "/help"),
    ("/clear", "/clear"),
    ("/config", "/config"),
    ("/mcp", "/mcp"),
    ("/skills", "/skills"),
    ("/system", "/system"),
    ("/sessions", "/sessions"),
    ("/save", "/save"),
    ("/usage", "/usage"),
    ("/exit", "/exit"),
    ("/quit", "/quit"),
];
```

In `handle_key`, before the existing `KeyCode::Char(c)` arm runs the input mutators, dispatch on `app.input.mode`:

(a) For `KeyCode::Char('/')`: insert as normal, then `app.input.maybe_open_slash_menu(SLASH_COMMANDS)`.

(b) For any other `KeyCode::Char(c)` while in `SlashMenu`: insert, then `app.input.refilter_slash_menu(SLASH_COMMANDS)`.

(c) For `KeyCode::Enter` (no Shift/Alt) while in `SlashMenu`: `app.input.accept_menu_selection()` and **return** — don't fall through to submit. (User explicitly accepted; if they want to submit they press Enter again.)

(d) For `KeyCode::Tab` while in `SlashMenu`: `cycle_next()`. `Shift+Tab` (or `BackTab`): `cycle_prev()`.

(e) For `KeyCode::Esc` while in `SlashMenu`: `app.input.close_menu()` and don't propagate to the existing Esc-closes-overlay path.

(f) For `KeyCode::Backspace` while in `SlashMenu`: `backspace()`, then if `buffer` no longer starts with `/`, `close_menu()`. Otherwise `refilter_slash_menu(SLASH_COMMANDS)`.

(g) For `KeyCode::Up` / `KeyCode::Down` while in `SlashMenu`: move selection (`cycle_prev` / `cycle_next`) instead of doing history nav.

Each of these is a small `if let InputMode::SlashMenu(_) = app.input.mode` guard around the existing match arms.

- [ ] **Step 8: Render the slash menu**

In `render`, after `frame.render_widget(paragraph, chunks[2])` for the input area, add:

```rust
if let InputMode::SlashMenu(ref menu) = app.input.mode {
    render_input_menu(frame, chunks[2], menu);
}
```

Define `render_input_menu` in `caliban/src/tui.rs`:

```rust
fn render_input_menu(
    frame: &mut ratatui::Frame<'_>,
    input_area: ratatui::layout::Rect,
    menu: &input::MenuState,
) {
    use ratatui::layout::Rect;
    use ratatui::widgets::List;
    use ratatui::widgets::ListItem;

    if menu.candidates.is_empty() {
        return;
    }
    let max_rows = 8u16;
    let rows = u16::try_from(menu.candidates.len())
        .unwrap_or(max_rows)
        .min(max_rows);
    let width = input_area.width;
    // Float menu directly above the input area so it doesn't overlap typing.
    let menu_area = Rect {
        x: input_area.x,
        y: input_area.y.saturating_sub(rows),
        width,
        height: rows,
    };
    let items: Vec<ListItem> = menu
        .candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let style = if i == menu.selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(c.display.clone(), style)))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(ratatui::widgets::Clear, menu_area);
    frame.render_widget(list, menu_area);
}
```

(Note: the spec says menu floats below the input. Rendering "below" in a TUI with a status bar at the bottom is tricky — there's no room. Float ABOVE the input area but visually it reads as "near the input" the way Claude Code's menu does. If the user wants strictly-below, that's a follow-up tweak: it'd require allocating menu space in the layout instead of overlaying with `Clear`.)

- [ ] **Step 9: Build + test**

Run: `cargo build --workspace && cargo test --workspace`
Expected: clean build, all tests green.

- [ ] **Step 10: Manual smoke**

Launch the TUI. Type `/`. Menu should appear with all 11 commands. Type `h` — menu narrows to `/help`. Press Enter — `/help` is in the buffer, menu closed. Press Enter again — help overlay opens (existing behavior).

Press `/` again. Type `xx` — menu shows no candidates (empty list). Press Esc — menu closes, buffer keeps `/xx`. Backspace twice — buffer is `/`, menu reopens (because typing brought us back to the trigger). Backspace once more — menu closes.

- [ ] **Step 11: Commit**

```bash
git add caliban/src/tui.rs caliban/src/tui/input.rs caliban/src/tui/completer.rs
git -c commit.gpgsign=false commit -m "feat(tui): slash-command autocomplete menu via nucleo-matcher"
```

---

## Task 5: @-path token parser

Pure logic for splitting an `@<token>` into `(dir_part, name_part)` and resolving the dir against (workspace_root, cwd, `~`).

**Files:**
- Modify: `caliban/src/tui/attach.rs`

- [ ] **Step 1: Write the failing tests**

Replace the stub at `caliban/src/tui/attach.rs`:

```rust
//! Resolve `@path` tokens to file attachments at submit time.

use std::path::{Path, PathBuf};

/// Split an `@<token>` (passed WITHOUT the leading `@`) into the directory
/// to enumerate and the name fragment to match against.
///
/// Resolution rules:
/// - `""`           => dir = workspace_root, name = ""
/// - `"foo"`        => dir = workspace_root, name = "foo"
/// - `"src/ma"`     => dir = workspace_root.join("src/"), name = "ma"
/// - `"/etc/h"`     => dir = "/etc/",     name = "h"
/// - `"~/.config/f"`=> dir = home.join(".config/"), name = "f"
/// - `"../sib/x"`   => dir = cwd.join("../sib/").canonicalize_lex(), name = "x"
pub(crate) fn split_at_token(
    token: &str,
    workspace_root: &Path,
    cwd: &Path,
    home: Option<&Path>,
) -> (PathBuf, String) {
    // 1. Split into dir_part_str + name_part on the last '/'.
    let (dir_str, name) = match token.rfind('/') {
        Some(i) => (&token[..=i], token[i + 1..].to_string()),
        None => ("", token.to_string()),
    };

    // 2. Resolve dir_str.
    let dir: PathBuf = if dir_str.is_empty() {
        workspace_root.to_path_buf()
    } else if let Some(rest) = dir_str.strip_prefix("~/") {
        match home {
            Some(h) => h.join(rest),
            None => workspace_root.join(dir_str),
        }
    } else if dir_str == "~/" || dir_str == "~" {
        home.map(Path::to_path_buf).unwrap_or_else(|| workspace_root.to_path_buf())
    } else if Path::new(dir_str).is_absolute() {
        PathBuf::from(dir_str)
    } else if dir_str.starts_with("./") || dir_str.starts_with("../") {
        cwd.join(dir_str)
    } else {
        workspace_root.join(dir_str)
    };

    (dir, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_token_is_workspace_root() {
        let (d, n) = split_at_token("", Path::new("/ws"), Path::new("/ws/sub"), Some(Path::new("/home")));
        assert_eq!(d, PathBuf::from("/ws"));
        assert_eq!(n, "");
    }

    #[test]
    fn bare_name_resolves_against_workspace() {
        let (d, n) = split_at_token("foo", Path::new("/ws"), Path::new("/ws"), None);
        assert_eq!(d, PathBuf::from("/ws"));
        assert_eq!(n, "foo");
    }

    #[test]
    fn nested_relative_under_workspace() {
        let (d, n) = split_at_token("src/ma", Path::new("/ws"), Path::new("/ws"), None);
        assert_eq!(d, PathBuf::from("/ws/src/"));
        assert_eq!(n, "ma");
    }

    #[test]
    fn absolute_path_passes_through() {
        let (d, n) = split_at_token("/etc/h", Path::new("/ws"), Path::new("/ws"), None);
        assert_eq!(d, PathBuf::from("/etc/"));
        assert_eq!(n, "h");
    }

    #[test]
    fn tilde_expands_to_home() {
        let (d, n) = split_at_token(
            "~/.config/f",
            Path::new("/ws"),
            Path::new("/ws"),
            Some(Path::new("/home/john")),
        );
        assert_eq!(d, PathBuf::from("/home/john/.config/"));
        assert_eq!(n, "f");
    }

    #[test]
    fn dotdot_resolves_against_cwd() {
        let (d, n) = split_at_token("../sib/x", Path::new("/ws"), Path::new("/ws/inner"), None);
        assert_eq!(d, PathBuf::from("/ws/inner/../sib/"));
        assert_eq!(n, "x");
    }

    #[test]
    fn trailing_slash_means_empty_name() {
        let (d, n) = split_at_token("src/", Path::new("/ws"), Path::new("/ws"), None);
        assert_eq!(d, PathBuf::from("/ws/src/"));
        assert_eq!(n, "");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p caliban tui::attach`
Expected: 7 pass.

- [ ] **Step 3: Commit**

```bash
git add caliban/src/tui/attach.rs
git -c commit.gpgsign=false commit -m "feat(tui): split_at_token resolves @-path tokens to (dir, name)"
```

---

## Task 6: @-path readdir + menu wiring

Live `ignore::WalkBuilder` for the active token's directory.

**Files:**
- Modify: `caliban/src/tui/attach.rs` (add `read_dir_candidates`)
- Modify: `caliban/src/tui/input.rs` (`AtMenu` open/refilter helpers)
- Modify: `caliban/src/tui.rs` (key dispatch for `@`)

- [ ] **Step 1: Tests for `read_dir_candidates`**

Append to `caliban/src/tui/attach.rs` test module:

```rust
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    fn touch(dir: &Path, rel: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::File::create(&p).unwrap().write_all(b"x").unwrap();
    }

    #[test]
    fn read_dir_lists_files_and_dirs() {
        let td = TempDir::new().unwrap();
        touch(td.path(), "alpha.txt");
        touch(td.path(), "beta.rs");
        fs::create_dir(td.path().join("sub")).unwrap();
        let cands = read_dir_candidates(td.path(), false).unwrap();
        let displays: Vec<&str> = cands.iter().map(|c| c.display.as_str()).collect();
        assert!(displays.contains(&"alpha.txt"));
        assert!(displays.contains(&"beta.rs"));
        assert!(displays.contains(&"sub/"));
    }

    #[test]
    fn read_dir_respects_gitignore() {
        let td = TempDir::new().unwrap();
        // Initialize a fake git repo so .gitignore is honored by the ignore crate.
        fs::create_dir(td.path().join(".git")).unwrap();
        fs::write(td.path().join(".gitignore"), "secret.txt\n").unwrap();
        touch(td.path(), "visible.txt");
        touch(td.path(), "secret.txt");
        let cands = read_dir_candidates(td.path(), false).unwrap();
        let displays: Vec<&str> = cands.iter().map(|c| c.display.as_str()).collect();
        assert!(displays.contains(&"visible.txt"));
        assert!(!displays.contains(&"secret.txt"));
    }

    #[test]
    fn read_dir_hides_dotfiles_unless_requested() {
        let td = TempDir::new().unwrap();
        touch(td.path(), ".env");
        touch(td.path(), "visible.txt");
        let hidden = read_dir_candidates(td.path(), false).unwrap();
        assert!(!hidden.iter().any(|c| c.display == ".env"));
        let shown = read_dir_candidates(td.path(), true).unwrap();
        assert!(shown.iter().any(|c| c.display == ".env"));
    }
```

- [ ] **Step 2: Implement `read_dir_candidates`**

Append to `caliban/src/tui/attach.rs` (above the test module):

```rust
use crate::tui::completer::Candidate;

/// One directory's immediate children, gitignore-aware. Returns names only
/// (no full path). Directories include a trailing `/`.
pub(crate) fn read_dir_candidates(dir: &Path, show_hidden: bool) -> std::io::Result<Vec<Candidate>> {
    use ignore::WalkBuilder;
    let mut out = Vec::new();
    let walker = WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(!show_hidden)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .build();
    for entry in walker.flatten() {
        if entry.path() == dir {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let name = entry.file_name().to_string_lossy();
        let display = if is_dir {
            format!("{name}/")
        } else {
            name.to_string()
        };
        let insert_str = display.clone();
        out.push(Candidate {
            display,
            insert: insert_str,
            score: 0,
        });
        if out.len() >= 500 {
            break;
        }
    }
    out.sort_by(|a, b| a.display.cmp(&b.display));
    Ok(out)
}
```

- [ ] **Step 3: Run those tests**

Run: `cargo test -p caliban tui::attach`
Expected: 7 + 3 = 10 pass.

- [ ] **Step 4: AtMenu transitions in `tui::input`**

Append to `impl Input` in `caliban/src/tui/input.rs`:

```rust
    /// Find the active @-token surrounding the cursor, if any.
    /// Returns `Some((trigger_byte_offset, raw_token_without_at))`.
    pub(crate) fn active_at_token(&self) -> Option<(usize, String)> {
        let before = &self.buffer[..self.cursor];
        let at_pos = before.rfind('@')?;
        // The character immediately before '@' (if any) must be whitespace or start-of-buffer.
        if at_pos > 0 {
            let prev = before[..at_pos].chars().next_back().unwrap_or(' ');
            if !prev.is_whitespace() {
                return None;
            }
        }
        // Token runs from `at_pos + 1` to either next whitespace or end of buffer.
        let after_at = &self.buffer[at_pos + 1..];
        let end_in_after = after_at
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after_at.len());
        let token = &self.buffer[at_pos + 1..at_pos + 1 + end_in_after];
        Some((at_pos, token.to_string()))
    }

    pub(crate) fn open_at_menu(&mut self, trigger_start: usize, candidates: Vec<Candidate>) {
        self.mode = InputMode::AtMenu(MenuState::new(trigger_start, candidates));
    }
```

- [ ] **Step 5: Tests for `active_at_token`**

Append to `tui::input::tests`:

```rust
    #[test]
    fn detects_at_token_at_start_of_buffer() {
        let mut i = Input::new();
        i.insert_char('@');
        i.insert_char('s');
        let (start, tok) = i.active_at_token().unwrap();
        assert_eq!(start, 0);
        assert_eq!(tok, "s");
    }

    #[test]
    fn detects_at_token_after_whitespace() {
        let mut i = Input::new();
        for c in "hello @sr".chars() {
            i.insert_char(c);
        }
        let (start, tok) = i.active_at_token().unwrap();
        assert_eq!(start, 6);
        assert_eq!(tok, "sr");
    }

    #[test]
    fn ignores_at_inside_word() {
        let mut i = Input::new();
        for c in "user@host".chars() {
            i.insert_char(c);
        }
        assert!(i.active_at_token().is_none());
    }
```

- [ ] **Step 6: Run input tests**

Run: `cargo test -p caliban tui::input`
Expected: previous 9 + 3 new = 12 pass.

- [ ] **Step 7: Wire `@` dispatch in `caliban/src/tui.rs`**

Pseudocode for the key dispatch — adapt to fit the existing match in `handle_key`:

After `Input` is mutated for any `Char(c)`, evaluate the @-state:

```rust
fn refresh_at_menu(app: &mut App) {
    use crate::tui::attach::{read_dir_candidates, split_at_token};
    use crate::tui::completer::Candidate;
    let Some((start, token)) = app.input.active_at_token() else {
        if matches!(app.input.mode, InputMode::AtMenu(_)) {
            app.input.close_menu();
        }
        return;
    };
    let cwd = app.cwd.clone();
    let workspace_root = app
        .args
        .workspace
        .clone()
        .unwrap_or_else(|| cwd.clone());
    let home = dirs::home_dir();
    let (dir, name) = split_at_token(&token, &workspace_root, &cwd, home.as_deref());
    let show_hidden = name.starts_with('.');
    let raw = read_dir_candidates(&dir, show_hidden).unwrap_or_default();
    let items: Vec<(&str, &str)> = raw
        .iter()
        .map(|c| (c.display.as_str(), c.insert.as_str()))
        .collect();
    let ranked = crate::tui::completer::rank(&items, &name, 32);

    // The candidate `insert` from `read_dir_candidates` is just the leaf name
    // (e.g. "main.rs" or "src/"). The buffer replacement runs from the `@`
    // to end-of-token, so the insert string must reproduce the FULL new
    // token including '@' and any directory prefix the user already typed.
    let dir_prefix = &token[..token.len() - name.len()];
    let ranked_with_full_insert: Vec<Candidate> = ranked
        .into_iter()
        .map(|mut c| {
            c.insert = format!("@{dir_prefix}{}", c.insert);
            c
        })
        .collect();
    app.input.open_at_menu(start, ranked_with_full_insert);
}
```

Then in `handle_key`:

- After inserting any character, if `app.input.active_at_token().is_some()` call `refresh_at_menu(app)`.
- After `backspace()`/`delete()`, call `refresh_at_menu(app)` for the same reason.
- `Tab` / `Shift+Tab` / `Up` / `Down` / `Esc` / `Enter` (no-shift, no-alt) in `AtMenu` mirror the SlashMenu handling: cycle / close / accept.
- `Enter` accept in AtMenu calls `app.input.accept_menu_selection()` — the candidate's `insert` ("alpha.txt" or "sub/") replaces the token after `@`, leaving the `@` itself in place.

When `AtMenu` is accepted with a directory candidate (display ends with `/`), don't close the menu — `accept_menu_selection` puts e.g. `@src/` in the buffer. Immediately call `refresh_at_menu(app)` so the menu re-opens for the new directory. Implementation:

```rust
KeyCode::Enter if !key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
    if matches!(app.input.mode, InputMode::AtMenu(_)) {
        let was_dir = match &app.input.mode {
            InputMode::AtMenu(m) => m
                .candidates
                .get(m.selected)
                .is_some_and(|c| c.display.ends_with('/')),
            _ => false,
        };
        app.input.accept_menu_selection();
        if was_dir {
            refresh_at_menu(app);
        }
        return;
    }
    // ... slash menu accept (Task 4) and submit (existing) below ...
}
```

- [ ] **Step 8: Render `AtMenu`**

Update `render`:

```rust
match app.input.mode {
    InputMode::SlashMenu(ref menu) | InputMode::AtMenu(ref menu) => {
        render_input_menu(frame, chunks[2], menu);
    }
    InputMode::Idle => {}
}
```

- [ ] **Step 9: Build + test**

Run: `cargo build --workspace && cargo test --workspace`
Expected: clean, all tests green.

- [ ] **Step 10: Manual smoke**

Launch caliban in its own repo directory. Type `look at @s`. Menu should show `src/` (and possibly other matches starting with `s`). Press Tab to highlight `src/`, press Enter. Buffer becomes `look at @src/`, menu re-opens showing children of `src/`. Pick `main.rs` (or similar), buffer becomes `look at @src/main.rs `. Press Esc to close. Type more, submit. (Attachments aren't wired yet — the `@` text just goes to the model as literal text. We'll wire attach in Task 8.)

- [ ] **Step 11: Commit**

```bash
git add caliban/src/tui.rs caliban/src/tui/input.rs caliban/src/tui/attach.rs
git -c commit.gpgsign=false commit -m "feat(tui): live @-path completion menu via ignore::WalkBuilder"
```

---

## Task 7: Toast primitive

**Files:**
- Modify: `caliban/src/tui/toast.rs`
- Modify: `caliban/src/tui.rs` (add `toast: Option<Toast>` to App, render strip, dismiss on key)

- [ ] **Step 1: Implement Toast with tests**

Replace the stub at `caliban/src/tui/toast.rs`:

```rust
//! Ephemeral one-row notification rendered above the input area.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub(crate) enum ToastLevel {
    Error,
    Warn,
    Info,
}

#[derive(Debug)]
pub(crate) struct Toast {
    pub(crate) level: ToastLevel,
    pub(crate) text: String,
    shown_at: Instant,
    ttl: Duration,
}

impl Toast {
    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self::new(ToastLevel::Error, text)
    }

    pub(crate) fn warn(text: impl Into<String>) -> Self {
        Self::new(ToastLevel::Warn, text)
    }

    pub(crate) fn info(text: impl Into<String>) -> Self {
        Self::new(ToastLevel::Info, text)
    }

    fn new(level: ToastLevel, text: impl Into<String>) -> Self {
        Self {
            level,
            text: text.into(),
            shown_at: Instant::now(),
            ttl: Duration::from_secs(5),
        }
    }

    pub(crate) fn is_expired(&self) -> bool {
        self.shown_at.elapsed() >= self.ttl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_toast_not_expired() {
        let t = Toast::error("boom");
        assert!(!t.is_expired());
    }

    #[test]
    fn level_preserved() {
        assert!(matches!(Toast::error("x").level, ToastLevel::Error));
        assert!(matches!(Toast::warn("x").level, ToastLevel::Warn));
        assert!(matches!(Toast::info("x").level, ToastLevel::Info));
    }
}
```

- [ ] **Step 2: Run toast tests**

Run: `cargo test -p caliban tui::toast`
Expected: 2 pass.

- [ ] **Step 3: Add `toast` field to `App`**

In `caliban/src/tui.rs`:

```rust
pub(crate) struct App {
    // ... existing fields ...
    pub(crate) toast: Option<toast::Toast>,
}
```

Initialize in `App::new`: `toast: None,`.

- [ ] **Step 4: Render the toast strip**

In `render`, the layout currently allocates a fixed number of chunks. Update layout to reserve one row for the toast when one is present (height 1 row), drawn between the transcript and the input.

Find the `Layout::default().direction(Direction::Vertical).constraints(...)` block. Today it has something like `Constraint::Min(0), Constraint::Length(spinner_rows), Constraint::Length(input_rows), Constraint::Length(1)` (or similar). Add a conditional toast row between input and status:

```rust
let toast_rows: u16 = if app.toast.as_ref().is_some_and(|t| !t.is_expired()) {
    1
} else {
    0
};
let constraints = [
    Constraint::Min(0),
    Constraint::Length(spinner_rows),
    Constraint::Length(toast_rows),
    Constraint::Length(input_rows),
    Constraint::Length(1), // status
];
```

Update `chunks[*]` indices accordingly. Then:

```rust
if toast_rows == 1 {
    if let Some(t) = &app.toast {
        let (fg, bg) = match t.level {
            toast::ToastLevel::Error => (Color::White, Color::Red),
            toast::ToastLevel::Warn => (Color::Black, Color::Yellow),
            toast::ToastLevel::Info => (Color::Gray, Color::Reset),
        };
        let p = Paragraph::new(Line::from(Span::styled(
            t.text.clone(),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        )));
        frame.render_widget(p, chunks[/* toast index */]);
    }
}
```

(Match the index to where you placed the toast row.)

- [ ] **Step 5: Dismiss on key event**

In `handle_key`, at the very top:

```rust
if app.toast.is_some() {
    app.toast = None;
}
```

(Place this BEFORE the existing handler body so any keystroke dismisses the toast. Don't `return` — the keystroke should still take effect.)

- [ ] **Step 6: Auto-expire on tick**

In the main event loop (`run`), after each frame draw, drop expired toasts:

```rust
if app.toast.as_ref().is_some_and(|t| t.is_expired()) {
    app.toast = None;
}
```

Place this near the top of the loop body so it runs every tick.

- [ ] **Step 7: Build + test**

Run: `cargo build --workspace && cargo test --workspace`
Expected: clean.

- [ ] **Step 8: Smoke (temporary)**

Add a temporary debug binding (e.g., behind a feature flag, OR just temporarily in your local copy): on `Ctrl+T`, call `app.toast = Some(toast::Toast::error("test toast"));`. Launch the TUI, press Ctrl+T, verify red strip above input. After 5 s, it disappears. Press a key, it disappears immediately. Then REVERT the temporary binding before committing.

- [ ] **Step 9: Commit**

```bash
git add caliban/src/tui.rs caliban/src/tui/toast.rs
git -c commit.gpgsign=false commit -m "feat(tui): ephemeral Toast primitive for inline error/warn/info"
```

---

## Task 8: Attachment resolver + submit pipeline + CLI flags

This is the largest task because three things tie together — resolver function, CLI plumbing, and the actual wiring in the submit handler.

**Files:**
- Modify: `caliban/src/tui/attach.rs` (add `resolve_attachments`, `Attachment`, `AttachError`)
- Modify: `caliban/src/main.rs` (CLI flags + env vars)
- Modify: `caliban/src/tui.rs` (submit pipeline, transcript line for 📎)
- Modify: `caliban/src/tui.rs` (TranscriptLine variant for attachment)

- [ ] **Step 1: Test resolve_attachments**

Append to `caliban/src/tui/attach.rs` test module:

```rust
    #[test]
    fn resolves_single_attachment() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("hello.txt");
        fs::write(&p, "hi there").unwrap();
        let msg = format!("Look at @{}", p.display());
        let r = resolve_attachments(&msg, td.path(), td.path(), 1024, 4096).unwrap();
        assert_eq!(r.attachments.len(), 1);
        assert_eq!(r.attachments[0].bytes, 8);
        assert_eq!(r.attachments[0].content, "hi there");
    }

    #[test]
    fn missing_path_left_as_literal() {
        let td = TempDir::new().unwrap();
        let msg = "hello @nonexistent there";
        let r = resolve_attachments(msg, td.path(), td.path(), 1024, 4096).unwrap();
        assert!(r.attachments.is_empty());
        assert_eq!(r.visible_text, msg);
    }

    #[test]
    fn oversize_returns_error() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("big.txt");
        fs::write(&p, vec![b'x'; 4096]).unwrap();
        let msg = format!("@{}", p.display());
        let err = resolve_attachments(&msg, td.path(), td.path(), 1024, 8192).unwrap_err();
        assert!(matches!(err, AttachError::Oversize { .. }));
    }

    #[test]
    fn budget_exceeded_returns_error() {
        let td = TempDir::new().unwrap();
        let a = td.path().join("a.txt");
        let b = td.path().join("b.txt");
        fs::write(&a, vec![b'x'; 700]).unwrap();
        fs::write(&b, vec![b'x'; 700]).unwrap();
        let msg = format!("@{} @{}", a.display(), b.display());
        let err = resolve_attachments(&msg, td.path(), td.path(), 1024, 1024).unwrap_err();
        assert!(matches!(err, AttachError::BudgetExceeded { .. }));
    }

    #[test]
    fn multiple_attachments_in_order() {
        let td = TempDir::new().unwrap();
        let a = td.path().join("a.txt");
        let b = td.path().join("b.txt");
        fs::write(&a, "aa").unwrap();
        fs::write(&b, "bb").unwrap();
        let msg = format!("@{} and @{}", a.display(), b.display());
        let r = resolve_attachments(&msg, td.path(), td.path(), 1024, 4096).unwrap();
        assert_eq!(r.attachments.len(), 2);
        assert_eq!(r.attachments[0].content, "aa");
        assert_eq!(r.attachments[1].content, "bb");
    }
```

- [ ] **Step 2: Implement `resolve_attachments`**

Append to `caliban/src/tui/attach.rs` (above the tests):

```rust
/// Successfully resolved message ready to send.
#[derive(Debug)]
pub(crate) struct ResolvedMessage {
    pub(crate) visible_text: String,
    pub(crate) attachments: Vec<Attachment>,
}

#[derive(Debug)]
pub(crate) struct Attachment {
    pub(crate) path: PathBuf,
    pub(crate) display_path: String,
    pub(crate) bytes: u64,
    pub(crate) content: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AttachError {
    #[error("@{} is {bytes} bytes; over the per-file limit of {limit}", path.display())]
    Oversize { path: PathBuf, bytes: u64, limit: u64 },
    #[error("attachments total {running_total} bytes; over the budget of {limit}")]
    BudgetExceeded { running_total: u64, limit: u64 },
    #[error("@{} is not valid UTF-8", path.display())]
    NotUtf8 { path: PathBuf },
    #[error("@{}: {source}", path.display())]
    Io { path: PathBuf, source: std::io::Error },
}

/// Resolve every `@<path>` token in `buffer`. Tokens that don't resolve to an
/// existing regular file are left as literal text. If any resolved file
/// violates the per-file or aggregate size caps, returns an error and DOES
/// NOT attach anything.
pub(crate) fn resolve_attachments(
    buffer: &str,
    workspace_root: &Path,
    cwd: &Path,
    per_file_max: u64,
    total_budget: u64,
) -> Result<ResolvedMessage, AttachError> {
    let home = dirs::home_dir();
    let mut attachments = Vec::new();
    let mut running_total: u64 = 0;

    for tok in extract_at_tokens(buffer) {
        let (dir, name) = split_at_token(&tok, workspace_root, cwd, home.as_deref());
        let candidate = if name.is_empty() {
            dir.clone()
        } else {
            dir.join(&name)
        };
        if !candidate.is_file() {
            continue;
        }
        let meta = match std::fs::metadata(&candidate) {
            Ok(m) => m,
            Err(source) => return Err(AttachError::Io { path: candidate, source }),
        };
        let bytes = meta.len();
        if bytes > per_file_max {
            return Err(AttachError::Oversize {
                path: candidate,
                bytes,
                limit: per_file_max,
            });
        }
        running_total = running_total.saturating_add(bytes);
        if running_total > total_budget {
            return Err(AttachError::BudgetExceeded {
                running_total,
                limit: total_budget,
            });
        }
        let bytes_vec = match std::fs::read(&candidate) {
            Ok(b) => b,
            Err(source) => return Err(AttachError::Io { path: candidate, source }),
        };
        let content = match String::from_utf8(bytes_vec) {
            Ok(s) => s,
            Err(_) => return Err(AttachError::NotUtf8 { path: candidate }),
        };
        let display_path = candidate
            .strip_prefix(workspace_root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| candidate.display().to_string());
        attachments.push(Attachment {
            path: candidate,
            display_path,
            bytes,
            content,
        });
    }

    Ok(ResolvedMessage {
        visible_text: buffer.to_string(),
        attachments,
    })
}

/// Pull out every `@<token>` from `buffer` (token = run of non-whitespace
/// after `@`, where the `@` is at start-of-buffer or preceded by whitespace).
fn extract_at_tokens(buffer: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = buffer.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && !(bytes[end] as char).is_whitespace() {
                end += 1;
            }
            if end > start {
                // Use char-safe slicing.
                out.push(buffer[start..end].to_string());
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// Build the outgoing wire string: visible_text followed by framed
/// `--- attached: ... ---` blocks for each attachment.
pub(crate) fn format_outgoing(msg: &ResolvedMessage) -> String {
    if msg.attachments.is_empty() {
        return msg.visible_text.clone();
    }
    let mut out = msg.visible_text.clone();
    for a in &msg.attachments {
        out.push_str("\n\n--- attached: ");
        out.push_str(&a.display_path);
        out.push_str(" (");
        out.push_str(&a.bytes.to_string());
        out.push_str(" bytes) ---\n");
        out.push_str(&a.content);
    }
    out
}
```

Add `thiserror` to `caliban/Cargo.toml` if not already present (it is — caliban-agent-core uses it). Check with `grep thiserror caliban/Cargo.toml`. If missing, add `thiserror = { workspace = true }` to `[dependencies]`.

- [ ] **Step 3: Run all attach tests**

Run: `cargo test -p caliban tui::attach`
Expected: previous 10 + 5 new = 15 pass.

- [ ] **Step 4: Add CLI flags + env vars to `caliban/src/main.rs`**

In the `Args` struct, before the `--debug` flag (around line 124), add:

```rust
    /// Maximum size of a single @-attachment, in bytes.
    #[arg(long, default_value_t = 262_144, env = "CALIBAN_MAX_ATTACH_BYTES")]
    pub(crate) max_attach_bytes: u64,

    /// Aggregate size cap across all @-attachments in one message, in bytes.
    #[arg(long, default_value_t = 1_048_576, env = "CALIBAN_ATTACH_BUDGET_BYTES")]
    pub(crate) attach_budget_bytes: u64,
```

- [ ] **Step 5: Add attachment line to `TranscriptLine`**

In `caliban/src/tui.rs`, find the existing `enum TranscriptLine` (search for `pub(crate) enum TranscriptLine`). Add a variant:

```rust
    /// 📎 marker showing an attached file under a user message.
    Attached { display_path: String, bytes: u64 },
```

Update the render function that converts `TranscriptLine` to `Line`s (in `render_transcript`) — add a match arm:

```rust
        TranscriptLine::Attached { display_path, bytes } => {
            let human = format_bytes(*bytes);
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    format!("📎 {display_path} ({human})"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
```

And add a small helper:

```rust
fn format_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}
```

- [ ] **Step 6: Wire submit pipeline**

In `handle_key`, in the Enter-submit arm (NOT the menu-accept and NOT the Shift/Alt-newline cases), find:

```rust
let line = app.input.submit();
```

Immediately AFTER history push and BEFORE `if prompt.starts_with('/')` slash dispatch, intercept:

```rust
let workspace_root = app
    .args
    .workspace
    .clone()
    .unwrap_or_else(|| app.cwd.clone());

let resolved = match crate::tui::attach::resolve_attachments(
    &line,
    &workspace_root,
    &app.cwd,
    app.args.max_attach_bytes,
    app.args.attach_budget_bytes,
) {
    Ok(r) => r,
    Err(e) => {
        let hint = match &e {
            crate::tui::attach::AttachError::Oversize { .. } => {
                "Drop the @ or raise --max-attach-bytes."
            }
            crate::tui::attach::AttachError::BudgetExceeded { .. } => {
                "Remove attachments or raise --attach-budget-bytes."
            }
            crate::tui::attach::AttachError::NotUtf8 { .. } => {
                "Binary files can't be inlined; ask me to Read it instead."
            }
            crate::tui::attach::AttachError::Io { .. } => {
                "Check the path and try again."
            }
        };
        app.toast = Some(toast::Toast::error(format!("{e} — {hint}")));
        // Restore the buffer so the user can edit.
        app.input.buffer = line;
        app.input.cursor = app.input.buffer.len();
        return;
    }
};

let outgoing_text = crate::tui::attach::format_outgoing(&resolved);
let prompt = resolved.visible_text.clone();
```

Then where the existing code did `caliban_provider::Message::user_text(prompt)`, change to use `outgoing_text` for the wire message but keep `prompt` for the transcript:

```rust
app.transcript.push(TranscriptLine::UserPrompt(prompt.clone()));
for a in &resolved.attachments {
    app.transcript.push(TranscriptLine::Attached {
        display_path: a.display_path.clone(),
        bytes: a.bytes,
    });
}
// ...
messages.push(caliban_provider::Message::user_text(outgoing_text));
```

Slash commands (`prompt.starts_with('/')`) still dispatch with `prompt`, not `outgoing_text`.

- [ ] **Step 7: Build + test**

Run: `cargo build --workspace && cargo test --workspace`
Expected: clean, all green.

- [ ] **Step 8: Integration smoke (manual)**

```bash
echo "hello world" > /tmp/caliban-test.txt
ANTHROPIC_API_KEY=$KEY cargo run -- 
```

Type `What's in @/tmp/caliban-test.txt ?` and submit. Verify:
- 📎 `/tmp/caliban-test.txt (11 B)` (or similar) appears under your user message.
- The assistant's response references "hello world" — confirms the file content reached the model.

Then try oversize:

```bash
head -c 300000 /dev/urandom | base64 > /tmp/caliban-big.txt
```

Type `@/tmp/caliban-big.txt` and submit. Verify a red toast appears at the bottom of the input strip, the buffer is preserved, no request is sent.

- [ ] **Step 9: Commit**

```bash
git add caliban/src/main.rs caliban/src/tui.rs caliban/src/tui/attach.rs caliban/Cargo.toml
git -c commit.gpgsign=false commit -m "feat(tui): @-path auto-attach with refuse-with-hint oversize policy"
```

---

## Task 9: Help overlay update + final smoke

**Files:**
- Modify: `caliban/src/tui.rs` (slash-help overlay rows)
- Modify: `README.md` (mention the new keybindings)

- [ ] **Step 1: Update the slash-help overlay rows**

Search for the help overlay row table (look for "Mouse wheel" — it was added recently). Add entries:

```rust
    ("Shift+Enter", "Insert newline (Alt+Enter on legacy terminals)"),
    ("/", "Open slash-command menu"),
    ("@", "Open path-completion menu (auto-attaches files on submit)"),
    ("Tab / Shift+Tab", "Cycle selection in a menu"),
```

Keep them grouped with the other keybinding rows.

- [ ] **Step 2: Update README**

In `README.md`, the TUI usage section currently lists slash commands but not the new keybindings. Append a paragraph after the existing slash-commands list:

```markdown
The input area supports multi-line composition (Shift+Enter, or Alt+Enter on
terminals that can't distinguish Shift+Enter from plain Enter). Typing `/`
opens a slash-command menu and typing `@` opens a fuzzy file picker that
auto-attaches the referenced file's content to the outgoing message. Files
over `--max-attach-bytes` (default 256 KB) or that exceed the per-message
`--attach-budget-bytes` (default 1 MB) cause an inline error and abort
the send.
```

- [ ] **Step 3: Build + test one more time**

Run: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets`
Expected: clean.

- [ ] **Step 4: Format**

Run: `cargo fmt --all`

- [ ] **Step 5: End-to-end smoke run**

Launch caliban interactively:

```bash
ANTHROPIC_API_KEY=$KEY cargo run --
```

Run through all four scenarios from the spec:

1. Multi-line: type a 3-line prompt with Shift+Enter between lines, submit, verify transcript shows the multi-line user message.
2. Slash menu: type `/`, see menu, type `he`, see `/help` highlighted, press Enter, verify help overlay opens.
3. @-attach: type `@README.md`, autocomplete to `@README.md`, submit, verify 📎 line appears and the assistant can reference README content.
4. Oversize: try `@<large-file>`, verify red toast and no send.

- [ ] **Step 6: Commit**

```bash
git add caliban/src/tui.rs README.md
git -c commit.gpgsign=false commit -m "docs(tui): document multi-line, slash menu, @-attach keybindings"
```

---

## Self-review checklist

After the final task, run through the spec one more time:

- [x] Multi-line composition (Shift+Enter + Alt+Enter fallback) — Task 3
- [x] Kitty keyboard protocol negotiation — Task 3
- [x] Slash autocomplete menu below input — Task 4
- [x] @-path completion (live, shell-style, no global walk) — Task 5 + 6
- [x] Token splitter handling ~/, .., absolute, trailing slash, empty — Task 5
- [x] Per-keystroke readdir with `ignore::WalkBuilder` + gitignore — Task 6
- [x] Auto-attach file content with `--- attached: ... ---` framing — Task 8
- [x] Refuse-with-hint oversize policy with `--max-attach-bytes` + `--attach-budget-bytes` — Task 8
- [x] Toast primitive (error / warn / info, auto-expire 5 s, dismiss on key) — Task 7
- [x] 📎 transcript line for each attached file — Task 8
- [x] `/help` overlay mentions the new keybindings — Task 9
