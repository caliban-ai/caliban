# The Interactive TUI

Invoking `caliban` with no prompt on a TTY launches the ratatui-based terminal interface. This is the primary mode for open-ended, conversational work.

## Launching

```bash
caliban
```

Caliban detects that stdin is a TTY and enters the TUI. If you prefer to start from a specific session, pass `--resume <name>` or `--continue` (resumes the most recently updated session).

## Basic flow

The screen is divided into three areas:

```text
┌──────────────────────────────────────────────┐
│ assistant: Ready. What would you like to do? │
│                                              │
│ 🔧 Read({"path":"src/main.rs"})              │
│    → Read src/main.rs, lines 1-42 of 42      │
│                                              │
│ assistant: The entry point is…               │
├──────────────────────────────────────────────┤
│ > █                                          │
├──────────────────────────────────────────────┤
│ ~/dev/my-project · anthropic claude-sonnet-4-6 · session: work │
└──────────────────────────────────────────────┘
```

| Area | Purpose |
|---|---|
| Transcript pane (top) | Conversation history, tool calls, and tool results |
| Input bar (middle) | Type your message here |
| Status line (bottom) | Working directory, active provider/model, session name |

Type your message and press **Enter** to send. For multi-line composition, use **Shift+Enter** on terminals that support the kitty keyboard protocol (kitty, iTerm2, Ghostty, WezTerm, foot) or **Alt+Enter** as a portable fallback.

Press **Ctrl-C** during a turn to cancel it. Press **Ctrl-C** or **Ctrl-D** at an empty prompt to exit.

## Tool calls and the permission modal

When the model wants to invoke a tool (read a file, run a shell command, etc.) caliban checks its permission rules before executing. Depending on the matching rule, the call is:

- **allowed automatically** — executes silently; a status line appears in the transcript.
- **denied automatically** — the model is told the call was refused.
- **asked** — a modal dialog appears:

```text
  ┌─ Permission required ────────────────────────────────┐
  │  Bash: git commit -am "fix typo"                     │
  │                                                      │
  │  [y] Allow once   [Y] Always allow                   │
  │  [n] Deny once    [N] Always deny                    │
  └──────────────────────────────────────────────────────┘
```

Pressing **y** or **n** handles the call once. Pressing **Y** or **N** opens a sub-prompt that lets you write a permanent allow or deny rule to a config scope, so you are not asked again for the same pattern.

## Cycling permission modes

**Shift+Tab** cycles the session-wide permission mode through the available values. The current mode is shown as a chip in the status line (the `default` mode hides the chip). Modes in order:

| Mode | What happens to Ask-class calls |
|---|---|
| `default` | The modal appears |
| `acceptEdits` | Write/Edit/MultiEdit/NotebookEdit are auto-allowed; Bash still asks |
| `plan` | All tool execution is paused; the model can only plan |
| `auto` | An auto-classifier decides; uncertain calls fall back to Ask |
| `dontAsk` | All Ask-class calls are allowed without prompting |

`bypassPermissions` (rules ignored entirely) is only reachable when the session was started with `--allow-dangerously-skip-permissions`.

For a full explanation of each mode, see [Permission Modes](../permissions/modes.md).

## The slash menu

Typing `/` at the input bar opens a fuzzy-search menu of slash commands:

```text
> /
  /clear      Clear the transcript
  /compact    Summarise and compress context
  /model      Switch the active model
  /rewind     Restore a checkpoint
  …
```

Continue typing to filter the list; press **Enter** to run the selected command. See [Slash Commands](../interactive/slash-commands.md) for the full index.

```admonish tip title="File attachments"
Type `@` followed by a path prefix to open a live file picker. Selecting a file inlines its contents into the outgoing message — the model sees the file without a separate Read tool round-trip.
```

For a deeper look at transcript navigation, keyboard shortcuts, and the `@`-attachment picker, see [The TUI in Depth](../interactive/tui-in-depth.md).
