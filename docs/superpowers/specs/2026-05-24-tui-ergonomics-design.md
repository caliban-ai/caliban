---
title: TUI ergonomics pack
date: 2026-05-24
status: Implemented
author: john.ford2002@gmail.com
adr: docs/adr/0027-tui-ergonomics.md
---

# TUI ergonomics pack — Design

**Date:** 2026-05-24
**Status:** Implemented
**Sub-project of:** caliban Rust agent harness
**ADR:** `docs/adr/0027-tui-ergonomics.md`

## Goal

Land the TUI ergonomics caliban is missing vs Claude Code in one
coherent sub-project. After this spec ships:

- **`@file` mention + autocomplete** — hardens the existing
  `attach.rs` scaffold; the file source becomes a swappable trait
  (`IgnoreWalkerSource` default, `CommandSource` operator-configurable);
  `respectGitignore` setting honored.
- **`!cmd` shell escape** — `!` at column 0 runs a shell command via
  the existing Bash tool path, output renders inline; gated by the
  same permission rule grammar as model-issued Bash calls.
- **`Ctrl+G` external editor** — write buffer to tempfile, exec
  `$VISUAL`/`$EDITOR`/`vi`, read back on exit.
- **Permission Ask modal** — closes the deferred follow-up from PR
  #8. New `caliban-tui-ask` crate implements `AskHandler` as a bridge
  to a ratatui modal with four actions (Allow once, Allow + persist
  to project/user, Deny).
- **`Ctrl+O` transcript viewer** — overlay rendering full `Message`
  history (all `ContentBlock` variants); `[` dumps viewport to
  terminal scrollback, `v` opens in `$VISUAL`.
- **Reverse history search (`Ctrl+R`)** — scoped session / project /
  all-projects; `Ctrl+S` cycles the scope.

This batch closes six rows under **E. TUI ergonomics** and unblocks
ADR 0029's auto-mode UX.

## Non-goals

- **Vim editing mode.** Separate input-state refactor.
- **Background bash (`Ctrl+B`) / `--bg` subagents.** Sub-agent fleet spec.
- **Voice dictation, image input, theming, status-line scripting.** Out of scope.
- **Multi-line input upgrades.** Already works.

## Architecture

```
caliban/src/tui.rs                ┌──────────────────────────────┐
   handle_event ──────────────────▶ InputBar (refactored)        │
                                  │  modes:                       │
                                  │    Idle / SlashMenu / AtMenu  │
                                  │    ShellEscape (NEW)          │  '!' prefix
                                  │    ReverseHistory (NEW)       │  Ctrl+R
                                  │    ExternalEditor (NEW)       │  Ctrl+G (modal)
                                  │    AskModal (NEW)             │  Permission Ask
                                  │    TranscriptViewer (NEW)     │  Ctrl+O (overlay)
                                  └────────────┬──────────────────┘
                                               │
   App.transcript ◀──── shell-escape output ───┤
                                               │
   caliban-agent-core::AskHandler ◀──── TuiAskHandler
                          │                  (mpsc + oneshot)
                          ▼
                PermissionsHook (existing) ── Ask → TuiAskHandler ── modal
```

Two new dimensions:

1. **`InputMode` grows beyond Idle/SlashMenu/AtMenu.** The first two
   new variants (`ShellEscape`, `ReverseHistory`) keep the prompt
   visible; the last three (`ExternalEditor`, `AskModal`,
   `TranscriptViewer`) are *modal* — they short-circuit the main key
   dispatch.
2. **The Ask modal is the first blocking overlay.** Existing overlays
   are passive viewers (`q`/Esc closes, agent keeps running). The Ask
   modal holds a `oneshot::Sender<HookDecision>` and the agent loop
   is parked on the corresponding receiver until the user picks.

## Crate structure (delta)

```
caliban/src/tui/
├── input.rs               # add modes (see below)
├── attach.rs              # extract FileSuggestionSource trait
├── ask_modal.rs           # NEW: AskModal widget + state
├── shell_escape.rs        # NEW: ! runner + render
├── external_editor.rs     # NEW: $VISUAL/$EDITOR roundtrip
├── transcript_viewer.rs   # NEW: Ctrl+O overlay
└── reverse_history.rs     # NEW: Ctrl+R state

crates/caliban-tui-ask/    # NEW thin crate so agent-core stays UI-free
└── src/lib.rs             # TuiAskHandler (bridge type)
```

`caliban-tui-ask` exists so `caliban-agent-core` does not depend on
ratatui. The binary depends on both and wires `Arc<TuiAskHandler>` into
the existing `PermissionsHook::new`.

## Input bar refactor

```rust
#[derive(Debug, Default)]
pub(crate) enum InputMode {
    #[default]
    Idle,
    SlashMenu(MenuState),
    AtMenu(MenuState),

    // non-modal substates of the input line:
    ShellEscape { started_with_bang: bool },
    ReverseHistory(ReverseHistoryState),

    // modal substates (input is locked while active):
    ExternalEditor(EditorLock),
    AskModal(AskModalState),
    TranscriptViewer(TranscriptViewerState),
}
```

Triggers:

| Trigger key | Buffer state required             | Resulting mode                          |
| ----------- | --------------------------------- | --------------------------------------- |
| `/`         | cursor == 1 *and* buffer == `/`   | `SlashMenu`                             |
| `@`         | preceded by start or whitespace   | `AtMenu`                                |
| `!`         | cursor == 1 *and* buffer == `!`   | `ShellEscape`                           |
| `Ctrl+R`    | any                               | `ReverseHistory`                        |
| `Ctrl+G`    | any                               | `ExternalEditor`                        |
| `Ctrl+O`    | any                               | `TranscriptViewer`                      |
| (agent)     | Ask verdict from `PermissionsHook`| `AskModal` (forced)                     |

All input-area key handling moves under a `handle_input_key(mode,
key)` function so we don't sprinkle mode branches across
`handle_event`.

## `@file` source — swappable trait

```rust
pub trait FileSuggestionSource: Send + Sync {
    async fn suggest(&self, cwd: &Path, prefix: &str, max: usize) -> Vec<Candidate>;
}

pub struct IgnoreWalkerSource { pub respect_gitignore: bool, pub show_hidden: bool }
pub struct CommandSource { pub program: String, pub args: Vec<String>, pub timeout: Duration }
```

Default is `IgnoreWalkerSource { respect_gitignore: true, show_hidden:
false }`. The `ignore` crate is already a workspace dep — no new
dependency. `CommandSource` spawns an operator-configured program and
parses newline-separated paths from stdout; on timeout (default 200ms)
or non-zero exit, falls back to the walker and toasts once per session.

Submit-time resolution (`@path` → attached file content with size cap)
is unchanged.

## `!` shell escape

When the buffer is exactly `!`, the input line enters `ShellEscape`.
The leading `!` is displayed but not treated as part of the prompt;
hint strip shows `[run shell command — Enter to execute, Esc to
cancel]`. On Enter:

- Synthesize a `Bash` tool call with the remaining buffer as
  `command`. Routes through the same dispatch path as a model-issued
  `Bash` — `PermissionsHook` gates via existing rules (`Bash:git *`,
  `Bash:rm *`, etc.).
- Output streams into the transcript as a new `ShellEscapeBlock`
  variant — distinct from `ToolCall` so the display can skip the
  model-facing framing:

  ```
  ! git status
    On branch main
    nothing to commit, working tree clean
    (exit 0)
  ```

- The synthesized call is **not** added to conversation history — a
  user-side action.
- Cancellation (Ctrl+C / Esc) leaves a `[cancelled]` marker.
- An `Ask` verdict pops the Ask modal. A `Deny` shows the denial
  reason inline.

Plan mode still gates: synthesized Bash calls route through the same
`before_tool` hook, so plan-mode denies as expected.

## External editor (`Ctrl+G`)

```
Ctrl+G ──► TerminalGuard::suspend  (leave alt-screen, raw mode off)
       ──► write buffer → tempfile (suffix .md)
       ──► spawn editor argv = [editor, tempfile]; inherit stdio; wait()
       ──► read tempfile → buffer; cursor → end
       ──► TerminalGuard::resume   (re-enter alt-screen)
       ──► Drop unlinks tempfile
```

Editor resolution: `$VISUAL` if set and non-empty; else `$EDITOR`;
else `vi` (POSIX) or `notepad` (Windows). The value is whitespace-split
verbatim — `EDITOR='code --wait'` becomes `["code", "--wait",
tempfile]`. No shell parsing. Non-zero exit logs a toast and discards
the temp content.

While the editor runs, the agent loop continues — events arrive but
rendering is paused until resume.

## Permission Ask modal

Lives in `caliban/src/tui/ask_modal.rs`. Uses the existing overlay
infrastructure (`centered_rect`, `Clear`, blocking key dispatch).

```rust
// crates/caliban-tui-ask/src/lib.rs
pub struct TuiAskHandler {
    tx: tokio::sync::mpsc::UnboundedSender<AskRequest>,
}

pub struct AskRequest {
    pub tool_name: String,
    pub input_summary: String,
    pub matched_rule: Option<Rule>,
    pub respond: tokio::sync::oneshot::Sender<AskResponse>,
}

pub enum AskResponse {
    AllowOnce,
    AllowAndAddRule { tool_pattern: String, persist: PersistScope },
    Deny,
}

pub enum PersistScope { Project, User }

#[async_trait]
impl AskHandler for TuiAskHandler { /* sends AskRequest, awaits oneshot */ }
```

Modal rendering:

```
┌─ Permission needed ─────────────────────────────────┐
│ Tool:    Bash                                       │
│ Command: rm -rf /tmp/scratch                        │
│ Rule:    Bash (Ask) — built-in default              │
│                                                     │
│ [y] Allow once                                      │
│ [a] Allow + remember as Bash:rm * in this project   │
│ [u] Allow + remember in user permissions.toml       │
│ [n] Deny    [Esc] Deny                              │
└─────────────────────────────────────────────────────┘
```

Persist actions append to the chosen file (project
`<workspace>/.caliban/permissions.toml` or user
`$XDG_CONFIG_HOME/caliban/permissions.toml`) and re-load the rule set
in-process. Before append, check for literal-equal rule and skip if
present.

Safety:

- Modal is blocking — Enter on the input line is a no-op. Ctrl+C
  resolves to `Deny`.
- 10-minute hard timeout on the oneshot — past that, resolve `Deny`
  with reason "ask modal timed out" (matches existing Bash deadline).
- Auto-scroll disabled while the modal is open.

## Transcript viewer (`Ctrl+O`)

Renders `App.messages` directly (not `TranscriptLine`). Each `Message`
walks its `ContentBlock`s:

- `Text` — verbatim
- `Thinking` — dim italic, `▸` prefix
- `ToolUse` — `🔧 {name}({pretty_json})`
- `ToolResult` — indented `→`; `is_error` flips color
- `Image` — `[image: media_type=<mime> bytes=<n>]`
- `RedactedThinking` — `[redacted]`

Keys:

| Key       | Action                                                      |
| --------- | ----------------------------------------------------------- |
| j/k, ↑↓   | Scroll one row                                              |
| g / G     | Top / bottom                                                |
| Ctrl+E    | Toggle show-all (hide thinking + tool inputs by default)    |
| `[`       | Dump viewport to scrollback (leave alt-screen, print, re-enter) |
| `v`       | Write full transcript to tempfile + exec `$VISUAL`/`$EDITOR`|
| q, Esc    | Close                                                       |

## Reverse history search (`Ctrl+R`)

```rust
pub struct ReverseHistoryState {
    pub query: String,
    pub scope: HistoryScope,    // Session | Project | AllProjects
    pub cursor: usize,
}
```

- `Ctrl+R` opens at `Session` scope with the current input history.
- `Ctrl+S` cycles `Session → Project → AllProjects → Session`. Project
  scope reads all session files under the current workspace from the
  existing `SessionStore`; all-projects walks `SessionStore::default_root()`.
- Typing refilters; ↑/↓ cycle matches; Enter accepts; Esc reverts.

Wider scopes lazily memoize on first open per TUI session. The
all-projects load runs in `tokio::task::spawn_blocking` with a 2s
budget; on expiry, fall back to Session scope and toast a warning.

## Public API sketches

```rust
// caliban/src/tui/shell_escape.rs
pub(crate) async fn run_shell_escape(
    command: String,
    permissions: Arc<PermissionsHook>,
    cwd: PathBuf,
    cancel: CancellationToken,
) -> Result<ShellEscapeOutcome, ShellEscapeError>;

// caliban/src/tui/external_editor.rs
pub(crate) fn edit_externally(
    guard: &mut TerminalGuard,
    initial: &str,
) -> Result<Option<String>, ExternalEditorError>;

// caliban/src/tui/transcript_viewer.rs
pub(crate) fn dump_to_scrollback(
    guard: &mut TerminalGuard,
    messages: &[Message],
) -> Result<(), TranscriptError>;
```

## Tests (enumerated)

1. **InputMode dispatch** — `!` at col 0 enters ShellEscape; `!`
   mid-buffer does not.
2. **Shell escape permission Allow** — Bash allowed; output captured;
   exit 0 displayed.
3. **Shell escape permission Deny** — Bash:rm * denied; `[denied: rule
   …]` shown; no subprocess.
4. **Shell escape cancellation** — Ctrl+C while running cancels via
   the Bash tool's cancel token.
5. **External editor roundtrip** — fixture editor replaces buffer
   contents.
6. **External editor non-zero exit** — toast shows error; buffer
   unchanged; tempfile unlinked.
7. **TuiAskHandler bridge** — sending `AllowOnce` resolves the
   oneshot to `HookDecision::Allow`.
8. **AskModal persist project** — `[a]` appends rule to project TOML
   and re-loads rule set.
9. **AskModal persist user** — `[u]` appends to user TOML.
10. **AskModal Deny propagates** — synthesized denial ToolResult.
11. **AskModal duplicate-rule skip** — `[a]` twice writes one rule.
12. **Transcript viewer renders all ContentBlock variants** — snapshot
    fixture stable.
13. **Transcript viewer Ctrl+E toggle** — thinking blocks hide/show.
14. **Transcript dump-to-scrollback** — mock writer records lines;
    alt-screen leave/re-enter sequence captured.
15. **Reverse history session scope** — typing `git ` filters matches;
    Enter writes to buffer.
16. **Reverse history scope cycle** — `Ctrl+S` cycles
    Session→Project→AllProjects.
17. **`@file` source: respect gitignore default** — walker excludes
    `target/`.
18. **`@file` source: custom command** — fixture script returns two
    paths; both become candidates.
19. **`@file` source: command timeout** — slow command times out at
    200ms; falls back to walker; toast surfaces once.
20. **External-editor lock** — keystrokes do not mutate buffer while
    editor is running.
21. **Ask modal blocks auto-scroll** — opening disables auto-scroll;
    closing restores.

## Risks

- **InputMode refactor regresses slash/at flow.** Mitigation: refactor
  in two passes — first extract `handle_input_key` with current modes
  only, then add new modes — keep tests green between passes.
- **External editor terminal handoff artifacts** on terminals with
  partial keyboard-enhancement support. Mitigation: best-effort
  flag pop+push (same pattern as `TerminalGuard::Drop`); fallback
  toast and disable `Ctrl+G` for the session on `tcsetattr` failure.
- **Ask modal deadlock.** Mitigation: 10-minute hard timeout on the
  oneshot.
- **Persisted-rule duplication.** Mitigation: check before append;
  dedupe also in `load_rules_file`.
- **All-projects history scope slow on large vaults.** Mitigation: 2s
  budget in `spawn_blocking`; fall back to Session on expiry.
- **Shell escape vs plan mode.** Mitigation: synthesized call routes
  through `before_tool`; plan mode denies as expected with
  `[denied: plan mode]`.

## Acceptance criteria

- `cargo build --workspace` clean; clippy clean; fmt clean.
- ≥21 new tests passing.
- `caliban-tui-ask` exports `TuiAskHandler`, `AskRequest`,
  `AskResponse`, `PersistScope`.
- `caliban/src/tui/` adds the five new modules; `tui.rs` wires new
  `InputMode` variants.
- Six rows in `docs/parity-gap-matrix.md` move 🔴 → ✅ under **E. TUI
  ergonomics**: `@file` autocomplete (hardening), `!` shell escape,
  external editor, Ask modal, transcript viewer, reverse history
  search. Vim mode / background bash / voice / image input stay 🔴.
- README "TUI" section documents `Ctrl+R`, `Ctrl+O`, `Ctrl+G`, `!`,
  Ask modal, and new `fileSuggestion` / `respectGitignore` settings.
- ADR 0027 in `accepted` status.

## Cross-spec dependencies

- **ADR 0028 (Checkpointing) consumes Esc-Esc.** This spec leaves that
  combo to checkpointing — Esc-Esc opens the rewind menu only from
  `InputMode::Idle` with an empty buffer.
- **ADR 0029 (Permission modes + auto-mode) consumes `TuiAskHandler`.**
  Auto-mode's `soft_deny` classifier verdict falls through to the
  same modal.
- **ADR 0023 (MCP v2) reuses modal-overlay infrastructure** for the
  elicitation modal. No code shared, but rendering primitives
  (`centered_rect`, `Clear`, blocking dispatch) originate here.
