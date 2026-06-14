# ADR 0013 · TUI overlays + layout v2 (input bracketed by horizontal rules)

- **Status:** accepted
- **Date:** 2026-05-23

## Context

The first TUI iteration shipped a working three-region layout (output |
status | input) and slash commands that wrote to the transcript. As the
slash-command list grew (help, config, mcp, skills) the transcript
became a cluttered place to render reference information.

## Decision

1. **Layout v2** reorders the regions so the input area sits between
   the output region and the status bar, bracketed by single-row
   horizontal rules. This puts the active input visually closer to the
   bottom (where the user's hands rest) and matches the Claude Code
   layout the user requested.

2. **Overlays** are modal popups rendered centered (80% × 80%) over
   the main view via ratatui's `Clear` + bordered `Block` + `Paragraph`
   widgets. `ViewState::Overlay(Overlay)` on `App` tracks which
   overlay is active; `Esc` or `q` resets to `ViewState::Main`. Main-
   view key handling is suppressed while an overlay is open (the
   overlay is read-only in v1).

3. **Four sub-menus:** `/help` (slash command + key reference),
   `/config` (active configuration from `app.args`/`app.session`),
   `/mcp` (stub pointing at future caliban-mcp-client), `/skills`
   (stub pointing at future caliban-skills).

## Consequences

- **Positive:** Reference views don't pollute the transcript. The
  layout is closer to Claude Code's. The /config view is genuinely
  useful for verifying caliban's state at a glance. The /mcp and
  /skills stubs document the future direction in the UI itself.
- **Negative:** Two more enum variants per addition; overlay content
  is static for now and must be hand-edited when slash commands
  evolve. Editing config from the UI is deferred.
- **Revisit if:** A keyboard-driven command palette (Ctrl+P-style) is
  desired; if the slash-command list grows beyond ~12 entries and
  needs categorization; if /config gains edit capability (toggling
  bools, changing model mid-session) requiring stateful focus tracking.
