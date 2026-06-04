# Slash Commands

Slash commands are operator-level shortcuts you type directly in the TUI input bar. They are not model-tool calls and are not gated by the permission rule grammar — they run as your direct action.

## How the slash system works

Type `/` in the input bar to open the suggestion menu. A fuzzy typeahead list appears showing all registered commands grouped by category.

```admonish warning title="Typeahead is partially implemented"
The slash-menu fuzzy typeahead is marked 🟡 (partial) in the [parity matrix](../appendix/parity.md). Basic prefix matching works; full fuzzy ranking and category grouping are in progress.
```

Continue typing to narrow the list, then press `Enter` (or `Tab`) to select a command. Some commands run immediately (`immediate: true`) and return to the input bar; others open an overlay or emit output to the transcript.

Hooks fire on every slash submission: `UserPromptSubmit` carries `is_slash: true`, `command`, and `args`, so hooks can audit or veto any slash command.

### Plugin-supplied commands

Plugins can register additional slash commands through the same `SlashCommandRegistry`. Built-in commands take priority; a plugin command with a conflicting name is dropped with a warning at registration time. See [Plugins](../extending/plugins.md) for details.

## Common commands

The table below lists the most frequently used built-in commands. The full list — including commands added by plugins — is enumerated at runtime by `/help` inside the TUI.

| Command | Args | What it does |
|---------|------|-------------|
| `/help` | — | Open the help overlay listing all visible commands |
| `/clear` | — | Clear transcript and conversation history; keep todos and system prompt |
| `/quit` | — | Exit caliban (`/exit` is an alias) |
| `/resume` | `[query]` | List persisted sessions (optional name substring filter) |
| `/init` | `[--force]` | Generate `CLAUDE.draft.md` from `AGENTS.md` / `.cursorrules` / `git status` |
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
| `/rewind` | — | Open the checkpoint picker (also: `Esc Esc` on empty input) |
| `/recap` | — | Summarize the conversation without mutating history |
| `/btw` | `<question>` | One-shot ephemeral side query to a fast model; result inlined |
| `/export` | `[path] [--format json]` | Export session transcript to markdown (or JSON) |
| `/doctor` | `[--deep]` | Run health checks: settings, MCP, skills, hooks, provider auth |
| `/status` | — | Show provider and auth status |
| `/statusline` | — | Inspect the active custom status-line configuration |
| `/loop` | `[--n=N] [--interval=S]` | Plan repeated turns (execution bounded by `--max-turns`) |

```admonish note title="Full reference"
The complete, up-to-date slash command index — including plugin-supplied commands and hidden aliases — lives in [Slash Command Index](../reference/slash-index.md). The index is generated from the live registry so it always reflects what is actually registered in your build.
```

## Adding your own slash commands

Custom slash commands are defined as skills or plugins. See [Custom Slash Commands](../extending/slash-commands.md) for the authoring guide.
