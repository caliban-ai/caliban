# ADR 0012 · TUI via ratatui (replacing the rustyline REPL)

- **Status:** accepted
- **Date:** 2026-05-23

## Context

caliban's first interactive mode was a rustyline-based REPL: a line
editor with history and slash commands. It worked, but felt like a
shell rather than a proper agent UI. The user asked for a Claude
Code-like experience: dedicated input area, persistent status bar
showing context (cwd, model, session), scrolling conversation
transcript above.

## Decision

Replace the rustyline REPL with a `ratatui` + `crossterm`-based TUI.
Three-region vertical layout:

1. **Output region** — flex-grow; renders the conversation transcript
   via `Paragraph` with wrap. Auto-scrolls to the bottom; PageUp/Down
   for history.
2. **Status bar** — fixed 1 line; shows `cwd · provider model · session
   (turns) · running…`.
3. **Input area** — fixed 2 lines (border + line); plain text input
   with cursor + line editing + arrow-key history.

The event loop multiplexes terminal events (crossterm `EventStream`)
and agent stream events via `tokio::select!`. `std::future::pending()`
keeps the agent arm dormant when no turn is running.

Raw mode + alternate screen entered via a `TerminalGuard` RAII type
that restores terminal state on Drop (including panic-recovery).

## Consequences

- **Positive:** Looks and feels like a modern agent CLI. Status bar
  gives immediate context (which session, which model, which dir).
  Streaming output renders in real-time above the prompt without
  interfering with input. ratatui handles terminal resize automatically.
- **Negative:** Significantly more code (~400 lines vs. rustyline's
  ~250). ratatui + crossterm add non-trivial deps. Markdown rendering,
  mouse support, and customizable themes are deferred. Non-TTY
  invocation without a prompt is now an error (use `--prompt` or pipe
  via `-`).
- **Revisit if:** users want mouse interaction, syntax-highlighted code
  blocks in responses, or split-pane layouts (e.g., a side panel
  showing recent tool calls). Each would be a focused follow-on.
