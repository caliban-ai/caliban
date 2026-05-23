# TUI Input Ergonomics — Design

**Date:** 2026-05-23
**Status:** Approved
**Slice:** 1 of 4 in the TUI/UX polish sub-projects (input → assistant rendering → tool call rendering → permission prompts)

## Goal

Replace caliban's minimal single-line input with three capabilities that together turn the TUI into a daily-driver-grade composer:

1. Multi-line composition.
2. Slash-command autocomplete menu.
3. `@`-path completion with file auto-attach.

Each capability is independently useful but they share the same input area and overlay primitives, so they ship together.

## Non-goals

- Mouse-click cursor placement inside the input buffer (mouse capture is currently used only for scrolling the transcript).
- `@`-attachment chip rendering (paperclip with byte count is the v1; richer chips ship with tool-call rendering in slice 3).
- Permission prompts for any tool (slice 4).
- Markdown / syntax highlighting in assistant output (slice 2).
- Per-attachment token counting (only byte limits in v1).

## Scope summary

| Capability | Trigger | Submit | Cancel |
|---|---|---|---|
| Newline | Shift+Enter or Alt+Enter | n/a | n/a |
| Slash menu | `/` at column 0 | Enter inserts selection; Esc closes | Backspace past `/` |
| @-path menu | `@` anywhere | Enter inserts path; Esc closes | Backspace past `@` |
| Submit | Enter (in `Idle` mode) | resolves @-attachments then sends | Ctrl-C |

Behaviour decisions, agreed during brainstorming:

- **Multi-line:** Shift+Enter is the primary newline; Alt+Enter is the portable fallback for terminals that collapse Shift+Enter to plain Enter (Terminal.app, tmux on some setups).
- **Slash menu position:** floats *below* the input area, matching Claude Code.
- **@-path lookup:** live, on-demand, shell-style — never a workspace-wide pre-walked index.
- **Oversize-file policy:** refuse-with-hint. Aborts the send, leaves the buffer intact, shows a toast. User either drops the `@` or raises the limit.

## Architecture

### Input state machine

Today the input is a `String` + `cursor: usize` in `App`. Lift to:

```rust
pub(crate) struct Input {
    buffer: String,             // raw text including '/' and '@' triggers
    cursor: usize,              // byte offset
    mode: InputMode,
    history: Vec<String>,
    history_cursor: Option<usize>,
}

enum InputMode {
    Idle,
    SlashMenu(MenuState),
    AtMenu(MenuState),
}

struct MenuState {
    candidates: Vec<Candidate>,
    selected: usize,
    trigger_start: usize,       // byte offset of '/' or '@'
}

struct Candidate {
    display: String,            // what the user sees in the menu
    insert: String,             // what gets spliced into the buffer
    score: i32,                 // from nucleo-matcher
}
```

Mode transitions are pure functions of `(buffer, cursor)`. The TUI render reads `Input` and draws the appropriate overlay.

### Key dispatch

`handle_key` consults `Input::mode` first, then falls through to global app dispatch (overlays, Ctrl-C, etc).

| Key | Idle | SlashMenu | AtMenu |
|---|---|---|---|
| `Enter` | submit | insert selected, close menu | insert selected (trailing space), close |
| `Shift+Enter` | newline | newline + close menu | newline + close menu |
| `Alt+Enter` | newline | newline + close menu | newline + close menu |
| `Tab` | – | cycle selection forward | cycle selection forward |
| `Shift+Tab` | – | cycle selection backward | cycle selection backward |
| `↑` / `↓` | history nav | move selection | move selection |
| `Esc` | close overlay | close menu, keep typed text | close menu, keep typed text |
| `/` at col 0 | open SlashMenu | typed into prefix | typed into prefix |
| `@` | open AtMenu | typed into slash prefix | open new AtMenu |
| printable | insert | insert, refilter | insert, refilter |
| `Backspace` past trigger | delete | close menu, then delete | close menu, then delete |

### Kitty keyboard protocol

On `TerminalGuard::enter`, push `KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | REPORT_ALL_KEYS_AS_ESCAPE_CODES`. Terminals that don't support it ignore the request silently; Alt+Enter is the documented portable fallback either way. Pop the flags on `Drop`, before `LeaveAlternateScreen`.

### @-path resolution rules

Active `@`-token = substring from `trigger_start` to the next whitespace or end-of-buffer. Resolution splits it into `(dir_part, name_part)` on the last `/`:

| Token | dir_part | name_part |
|---|---|---|
| `@foo` | `.` (workspace root) | `foo` |
| `@src/ma` | `src/` | `ma` |
| `@/etc/host` | `/etc/` | `host` |
| `@~/.config/f` | `~/` (expanded) | `f` |
| `@../sibling/x` | `../sibling/` (cwd-relative) | `x` |
| `@` (empty) | `.` | `""` |

Workspace root resolution: caliban already has a `WorkspaceRoot` type — reuse it. `~/` expansion: `dirs::home_dir()` already in the workspace.

For each keystroke that mutates the active token, run:

```rust
ignore::WalkBuilder::new(&dir_part)
    .max_depth(Some(1))
    .hidden(name_part_starts_with_dot)
    .git_ignore(true)
    .git_exclude(true)
    .build()
```

Yielded entries (minus the directory itself) feed into `nucleo-matcher` ranked against `name_part`. Cap output at 500 candidates per readdir — if the directory exceeds that, append a synthetic `... <N more>` candidate that is unselectable.

### Submit pipeline

```rust
fn resolve_attachments(
    buffer: &str,
    workspace_root: &Path,
    cwd: &Path,
    per_file_max: u64,
    total_budget: u64,
) -> Result<ResolvedMessage, AttachError>;

struct ResolvedMessage {
    visible_text: String,           // user's literal input, @-paths intact
    attachments: Vec<Attachment>,
}

struct Attachment {
    path: PathBuf,                  // absolute, canonicalized
    display_path: String,           // workspace-relative if under root, else absolute
    bytes: u64,
    content: String,                // UTF-8 validated
}

enum AttachError {
    Oversize  { path: PathBuf, bytes: u64, limit: u64 },
    BudgetExceeded { running_total: u64, limit: u64 },
    NotUtf8  { path: PathBuf },
    Io       { path: PathBuf, source: io::Error },
}
```

Token matching: `@<non-whitespace>` AND resolves to an existing regular file. If the path doesn't exist, the `@`-prefix is left as literal text (user might genuinely be typing `@username`).

If `resolve_attachments` returns `Ok`, the TUI builds an outgoing user message with this body (provider-agnostic — formatted into the message text, not multi-part content blocks, so it works uniformly across Anthropic / OpenAI / Gemini / Ollama):

```
<visible_text>

--- attached: src/main.rs (1247 bytes) ---
<file content>

--- attached: README.md (3401 bytes) ---
<file content>
```

Transcript rendering: each attached file shows as a single elided line under the user message: `📎 src/main.rs (1.2 KB)`. File contents do not appear in the visible transcript; they only go to the model. Total bytes counter in the status bar: out of scope for v1.

If `resolve_attachments` returns `Err`, the TUI raises a toast (see below) and aborts the send. The buffer is preserved so the user can edit.

### Toast primitive

New UI element rendered as a one-row strip *just above* the input area:

```rust
pub(crate) struct Toast {
    level: ToastLevel,
    text: String,
    shown_at: Instant,
}

enum ToastLevel { Error, Warn, Info }
```

Stored in `App` as `Option<Toast>`. Auto-dismiss after 5 s OR on the next key event. Replacement, not stacking — newer toast wins.

Styling: red bg / white fg for `Error`, yellow bg / black fg for `Warn`, dim white for `Info`. All on a single row; long messages truncate with `…`.

Reused later by tool-call permission errors and any other transient feedback.

### Configuration

CLI flags on `caliban`:

| Flag | Default | Env var |
|---|---|---|
| `--max-attach-bytes <N>` | `262144` (256 KB) | `CALIBAN_MAX_ATTACH_BYTES` |
| `--attach-budget-bytes <N>` | `1048576` (1 MB) | `CALIBAN_ATTACH_BUDGET_BYTES` |

Per-file and aggregate caps independent. Aggregate exceeds → `BudgetExceeded` error referencing the running total.

## Data flow on submit

```
[user presses Enter in Idle mode]
        |
        v
resolve_attachments(buffer, workspace_root, cwd, per_file_max, total_budget)
        |
   +----+----+
   |         |
  Err       Ok(ResolvedMessage)
   |         |
   v         v
 Toast    build outgoing message:
 abort      visible_text + "\n--- attached: path (N bytes) ---\n<content>" * N
 send       |
            v
          Session::user_text(outgoing)
            |
            v
          existing Session::run() pipeline
            |
            v
          transcript appends:
            user line(s)
            📎 path1 (size)
            📎 path2 (size)
            assistant streaming...
```

## Components / files

- **New:** `caliban/src/tui/input.rs` — `Input`, `InputMode`, `MenuState`, key dispatch. Extracted from `tui.rs`. ~400 LOC.
- **New:** `caliban/src/tui/attach.rs` — `resolve_attachments`, `Attachment`, `AttachError`. Pure logic, no TUI dependency. ~200 LOC.
- **New:** `caliban/src/tui/toast.rs` — `Toast`, `ToastLevel`, render helper. ~80 LOC.
- **New:** `caliban/src/tui/completer.rs` — fuzzy-matcher wrapper for both slash and @-path candidates. ~120 LOC.
- **Modified:** `caliban/src/tui.rs` — replace `app.input` String with `Input` struct, wire menus + toast into render loop, update `handle_key` to consult `Input::mode`. Help overlay learns the new keys.
- **Modified:** `caliban/src/main.rs` — two new CLI flags + env-var fallbacks.
- **Modified:** `caliban/Cargo.toml` — add `nucleo-matcher` workspace dep.
- **Modified:** `Cargo.toml` (workspace) — add `nucleo-matcher = "0.3"`.

## Dependencies

- `nucleo-matcher = "0.3"` — fuzzy ranking (used by Helix; small, no heavy deps).
- `ignore` — already present, used for the per-keystroke `WalkBuilder`.
- `crossterm` — already present, source of `KeyboardEnhancementFlags`.

## Testing

### Pure-logic (no TUI)

- `Input` state transitions: table-driven `(state, key) → state'` covering every cell of the dispatch table.
- `resolve_attachments`:
  - Happy path: one `@`-token resolves and attaches.
  - Multiple `@`-tokens: all attach, in order of appearance.
  - Non-existent path: stays as literal text in `visible_text`, no attachment.
  - Oversize: returns `AttachError::Oversize`.
  - Budget exceeded: returns `AttachError::BudgetExceeded` with running total.
  - Non-UTF-8: returns `AttachError::NotUtf8`.
- `@`-token splitter: parameter sweep over `~/`, `..`, absolute, trailing slash, empty token, dotfile prefix.

### Integration

- `tempfile::TempDir` populated with files + a `.gitignore` excluding `secret.txt`. Assert `secret.txt` is not yielded by completion candidates AND that direct `@secret.txt` still errors as `NotFound` (gitignore is for discovery; we treat ignored files as not-found for completion-flow purposes — explicit user paths still resolve, but discovery hides them).
- Submit with one `@`-attachment: assert outgoing wire string contains the framed file content.
- Submit with oversize attachment: assert `AttachError::Oversize`, assert no send, assert toast visible.

### TUI rendering (snapshot via `ratatui::backend::TestBackend`)

- SlashMenu open at `/he` → menu shows `/help`, `/clear`, etc.; second item highlighted via ↓.
- AtMenu open at `@src/` → menu shows immediate children of `src/`, sorted by score.
- `Toast::error("oversize")` rendered above input, red bg.

No new test infrastructure required — `ratatui::backend::TestBackend` is already used in existing tests.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Shift+Enter indistinguishable on legacy terminals | Alt+Enter documented in `/help`, surfaced on startup banner if kitty proto negotiation fails |
| `WalkBuilder` per-keystroke gitignore re-parse is slow | Don't preoptimize. Profile if observed; cache `Gitignore` matcher per workspace if needed |
| Huge directory (e.g. unignored `node_modules/`) | Hard cap of 500 candidates with "...N more" overflow row |
| Edge case: cursor moves out of active token | Close the menu on any cursor-jump event |
| Refuse-with-hint feels punitive for someone hitting 257KB | `--max-attach-bytes` is easy to raise; default is documented |

## Out of scope (deferred slices)

- Slice 2: assistant markdown + code block rendering.
- Slice 3: tool-call frame redesign + collapsibility + typed arg/result formatters.
- Slice 4: interactive permission prompts. The `Toast` primitive built here is the foundation for the permission overlay.

## Success criteria

- `cargo test --workspace` passes including new unit, integration, and snapshot tests.
- Manual smoke test:
  1. Type a multi-line prompt (Shift+Enter mid-sentence). Submit. Verify all lines in user transcript.
  2. Type `/he`, see menu, Enter, see `/help` opened.
  3. Type `@src/`, see directory completion, select `main.rs`, submit. Verify file attached to outgoing message (visible to model in next turn), 📎 line in transcript.
  4. Try `@<big-file-over-256KB>`, verify red toast and unsent buffer.
- `/help` overlay includes the new keybindings (Shift+Enter, Alt+Enter, Tab in menus).
