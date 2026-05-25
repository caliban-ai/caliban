# ADR 0027 · TUI ergonomics pack

- **Status:** accepted
- **Date:** 2026-05-24
- **Spec:** `docs/superpowers/specs/2026-05-24-tui-ergonomics-design.md`

## Context

caliban's TUI ships the basics (slash menu, `@`-attach, mouse-wheel
scroll, plan-mode chip, spinner) but six 🔴/🟡 rows under **E. TUI
ergonomics** in `docs/parity-gap-matrix.md` block day-to-day parity
with Claude Code: no shell escape, no external editor handoff, no
permission Ask modal (deferred from PR #8), no transcript viewer, no
reverse history search, and the `@file` suggestion path is hard-coded
with no operator override.

Each is small in isolation; together they push on the same input-bar
state machine and overlay rendering infrastructure. Shipping in one
batch lets us refactor `InputMode` once instead of three times.

The Ask modal in particular has knock-on effects: it's the only piece
of UI that *blocks* the agent loop on user input, so it sets the
contract for both auto-mode (ADR 0029) and MCP elicitation (ADR 0023).
Landing it here gives both specs a stable target.

## Decision

### One input bar, many modes

Promote `InputMode` from `{Idle, SlashMenu, AtMenu}` to a richer enum
that adds `ShellEscape`, `ReverseHistory`, `ExternalEditor`,
`AskModal`, and `TranscriptViewer`. The first two keep the prompt
visible; the last three are modal and short-circuit the main key
dispatch. All input-area key handling moves under a single
`handle_input_key` function.

### `!cmd` is a synthesized `Bash` invocation

A leading `!` at column 0 routes the rest of the line into the
existing `Bash` tool via the existing permission hook. That gives us
the rule grammar (`Bash:git *`, `Bash:rm *`, …) for free and keeps the
audit trail consistent. The synthesized call is **not** added to the
conversation history — it's a user action. Plan mode still gates.

### External editor is a tempfile roundtrip

`Ctrl+G` writes the input buffer to a tempfile, leaves the alternate
screen, execs `$VISUAL`/`$EDITOR`/`vi` with the path as argv, reads
the result back on exit, re-enters the alt-screen. The editor value
is whitespace-split verbatim (no shell parsing); `EDITOR='code
--wait'` works.

### The Ask modal lives in a new `caliban-tui-ask` crate

Adding a thin `caliban-tui-ask` crate keeps `caliban-agent-core`
UI-agnostic. It implements the existing `AskHandler` trait with an
mpsc/oneshot bridge to a ratatui modal supporting four actions —
Allow once, Allow + persist project, Allow + persist user, Deny —
with in-process re-load of the appended rule.

### Transcript viewer renders `Message` directly

`Ctrl+O` walks `App.messages` and renders every `ContentBlock`
variant (text, thinking, tool_use, tool_result, image, redacted) — the
model-eye view, distinct from the streaming-friendly `TranscriptLine`
view. `[` dumps viewport to scrollback via leave/re-enter alt-screen;
`v` opens the full transcript in `$VISUAL`.

### Reverse history search is scope-cycled

`Ctrl+R` opens at session scope; `Ctrl+S` cycles through project and
all-projects scopes. Wider scopes lazily memoize from `SessionStore`
in `spawn_blocking` with a 2s budget.

### File suggestion source becomes a trait

`FileSuggestionSource` with two impls: `IgnoreWalkerSource` (default,
gitignore-aware) and `CommandSource` (spawns an operator-configured
program). Walker stays on the existing `ignore` crate — no new deps.

## Consequences

- **Positive.** Six 🔴/🟡 rows move to ✅ in one initiative. The Ask
  modal unblocks ADR 0029 (auto-mode) and reuses the same overlay
  primitives that ADR 0023 needs for MCP elicitation. Operators get
  the keyboard surface expected of any modern agent CLI.
- **Negative.** `InputMode` becomes a fatter enum; `handle_event`
  needs careful refactoring to keep existing tests green. One new
  crate (`caliban-tui-ask`). Persisting Ask-modal decisions adds a
  write path into `permissions.toml` we previously only read from —
  parse-error and race-with-manual-edit cases need defensive handling.
- **Revisit if:** vim mode lands and the `InputMode` enum needs
  reshape into `(BarMode, EditorMode)`. The transcript viewer is a
  natural anchor for `/recap` and `/btw` later.
- **Out of scope, enabled by this work:** background bash (Ctrl+B),
  vim mode, image input, voice dictation.

## References

- Spec: `docs/superpowers/specs/2026-05-24-tui-ergonomics-design.md`
- Permissions trait: `crates/caliban-agent-core/src/permissions.rs`
- Overlay primitives: `caliban/src/tui.rs::centered_rect`
- Attach scaffold: `caliban/src/tui/attach.rs`
- Companion ADRs: 0028 (Checkpointing — consumes Esc-Esc), 0029
  (Auto-mode — consumes the Ask modal), 0023 (MCP v2 — reuses overlay
  primitives).
