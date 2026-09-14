# Slash Commands

Slash commands are operator-level shortcuts you type directly in the TUI input bar. They are not model-tool calls and are not gated by the permission rule grammar — they run as your direct action.

## How the slash system works

Type `/` in the input bar to open the suggestion menu. A fuzzy typeahead list shows the visible (non-hidden) registered commands, ranked by match quality.

```admonish note title="How the typeahead ranks matches"
The slash-menu typeahead does case-insensitive **fuzzy subsequence** matching (marked ✅ in the [parity matrix](../appendix/parity.md) since 0.4.0): typing `cfg` matches `/config`. Matches at the start or on a word boundary, and contiguous runs, rank ahead of scattered ones, with alphabetical name as the tiebreak.
```

Continue typing to narrow the list. Use `Tab` / `↓` and `Shift+Tab` / `↑` to move through it. `Enter` completes the highlighted command into the input bar, and a second `Enter` runs it. A command opens an overlay, writes output to the transcript, or shows a status message.

Commands marked `immediate` can run while a model turn is in flight, because they don't need the model: `/usage`, `/context`, `/status`, and so on. Other commands wait until the turn finishes.

### Plugin-supplied commands

The registry is designed to accept plugin-supplied commands. Registering a name twice replaces the earlier command and logs a warning. The plugin loader does not register commands yet, so today only the built-ins appear. See [Plugins](../extending/plugins.md).

## Common commands

The table below lists the most frequently used built-in commands. The full list — including commands added by plugins — is enumerated at runtime by `/help` inside the TUI.

| Command | Args | What it does |
|---------|------|-------------|
| `/help` | — | Open the help overlay listing all visible commands |
| `/clear` | — | Clear transcript and conversation history; keep todos and system prompt |
| `/quit` | — | Exit caliban (`/exit` is an alias) |
| `/resume` | `[query]` | List persisted sessions (optional name substring filter) |
| `/init` | `[--force]` | Write `CLAUDE.draft.md` from `AGENTS.md` / `.cursorrules` / `git status` (never overwrites `CLAUDE.md`) |
| `/model` | `[id]` | Show or switch the active model (same-provider swap in v1) |
| `/effort` | `<level>` | Set reasoning effort: `low`, `medium`, `high`, `max`, or `auto` |
| `/usage` | — | Show token usage and cumulative cost for this session |
| `/cost` | — | Show cumulative USD spend with per-model breakdown |
| `/context` | — | Show context-window utilization + top-N largest blocks |
| `/compact` | — | Trigger the configured compactor to summarize history |
| `/config` | — | Open the configuration overlay (merged settings + scope chain) |
| `/mcp` | — | Open the MCP server status overlay |
| `/hooks` | — | List configured hooks per event |
| `/plugins` | — | List installed plugins with enable/disable status |
| `/permissions` | — | Open the permissions overlay; cycle mode with `Tab`, delete rule with `d` |
| `/rewind` | — | Open the checkpoint overlay to restore code/conversation or fork a new session (also: `Esc Esc` on empty input); see [Checkpoints & Rewind](../memory/checkpoints.md) |
| `/recap` | — | Summarize the conversation without mutating history |
| `/btw` | `<question>` | One-shot ephemeral side query to a fast model; result inlined |
| `/export` | `[path] [--format json]` | Export session transcript to markdown (or JSON) |
| `/doctor` | `[--deep]` | Run health checks: settings, MCP, skills, hooks, provider auth |
| `/status` | — | Show the active provider (auth/subscription status is pending) |
| `/statusline` | — | Inspect the active custom status-line configuration |
| `/loop` | `[--n=N] [--interval=S]` | Report the repeat plan (bounded by `--max-turns`); execution is not implemented yet |

```admonish note title="Full reference"
The complete slash command index, including hidden aliases, is in [Slash Command Index](../reference/slash-index.md). `/help` inside the TUI always lists what your build actually registers.
```

## Adding your own slash commands

Custom slash commands are defined as skills or plugins. See [Custom Slash Commands](../extending/slash-commands.md) for the authoring guide.
