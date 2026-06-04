# The TUI in Depth

Caliban's interactive mode is a full-screen terminal UI built on [ratatui](https://ratatui.rs) + crossterm. This chapter covers everything that goes beyond the basics introduced in [The Interactive TUI](../getting-started/tui.md).

## Layout

The screen is divided into three regions from top to bottom:

```text
┌─────────────────────────────────────────────────────┐
│                                                     │
│   Transcript / output region  (flex-grow)           │
│                                                     │
├─────────────────────────────────────────────────────┤
│   Input area (2 rows)                               │
├─────────────────────────────────────────────────────┤
│   Status bar (1 row)                                │
└─────────────────────────────────────────────────────┘
```

The input area sits between the transcript and the status bar, placing the prompt visually close to the context information below it.

## Status line

The status bar shows `cwd · provider model · session (turns) · running…` during a live turn. When caliban is idle the spinner disappears and the elapsed-turn time is shown instead.

A custom prefix segment can be prepended by configuring a shell script in settings (see [Settings Reference](../configuration/reference.md) for the `statusLine` key). The script runs off-thread after each turn completes; its output is cached so it never blocks rendering. Use `/statusline` to inspect the active configuration.

## Keybindings

| Key | Action |
|-----|--------|
| `Enter` | Submit prompt |
| `\` + `Enter` | Insert a literal newline (multi-line input) |
| `PageUp` / `PageDown` | Scroll transcript |
| `Ctrl+R` | Reverse history search (session scope) |
| `Ctrl+S` | Cycle history scope → project → all projects |
| `Ctrl+G` | Open prompt in `$VISUAL` / `$EDITOR` / `vi` |
| `Ctrl+O` | Open transcript viewer overlay |
| `Ctrl+B` | Launch or follow a background bash process |
| `Shift+Tab` | Cycle permission mode chip |
| `Esc` | Close overlay / cancel input |
| `Esc Esc` | Open checkpoint rewind overlay (on empty input) |

## Overlays

Overlays are modal popups rendered centered (approximately 80% × 80%) over the main view. Press `Esc` or `q` to close any overlay. The active input bar is suppressed while an overlay is open.

Available overlays and how to reach them:

| Overlay | How to open |
|---------|------------|
| Help | `/help` |
| Configuration | `/config` |
| MCP server status | `/mcp` |
| Skills | `/skills` |
| Permissions editor | `/permissions` |
| Transcript viewer | `Ctrl+O` |
| Checkpoint rewind | `/rewind` or `Esc Esc` (on empty input) |
| System prompt | `/system` |

## Editor modes

Caliban's input bar uses **emacs-style** key bindings by default (`Ctrl+A` / `Ctrl+E` for line start/end, `Ctrl+K` to kill to end-of-line, etc.).

```admonish warning title="Vim mode is not yet available"
Vim editing mode is listed as a gap in the [parity matrix](../appendix/parity.md) (status: 🔴 planned). The `InputMode` enum is designed to accommodate a vim layer, but it has not shipped. Emacs bindings are the only editor mode in the current release.
```

## External editor handoff

`Ctrl+G` writes your current input buffer to a temp file, suspends the TUI (leaving the alternate screen), execs `$VISUAL` / `$EDITOR` / `vi` with the file as the argument, then reads the result back and re-enters the TUI. Multi-word editor values like `EDITOR='code --wait'` work because the value is split on whitespace without shell parsing.

## Transcript viewer

`Ctrl+O` opens the transcript viewer overlay. It renders every `ContentBlock` in the conversation history — text, tool calls, tool results, thinking blocks, and images — as the model sees them.

| Key | Action |
|-----|--------|
| `[` | Dump the current viewport to scrollback (leave + re-enter alt-screen) |
| `v` | Open the full transcript in `$VISUAL` |
| `q` / `Esc` | Close the viewer |
| `?` | Show key reference |

## Following background bash (Ctrl+B)

Background bash lets caliban run a shell command in the background while you continue interacting with the agent. Press `Ctrl+B` inside the TUI to open or follow the background bash output panel. The agent can launch background bash tasks via `Bash{background:true}`; the TUI surfaces their output through the same panel.

## Reverse history search

`Ctrl+R` opens inline reverse search over the current session's prompt history, showing matches as you type. `Ctrl+S` cycles the scope outward:

```
Ctrl+R  →  session scope
Ctrl+S  →  project scope  →  all-projects scope
```

Wider scopes are loaded lazily in a background task (budget: 2 s). History is persisted per project.

```admonish tip title="Configuring the TUI"
All TUI-relevant settings — the status line script, output style, and context-window thresholds — live in the settings hierarchy. See [Settings Reference](../configuration/reference.md) and [Output Styles](../extending/output-styles.md) for details.
```
