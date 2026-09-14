# Slash Command Index

Type `/` in the interactive TUI to open the command picker, or type a command name directly. Commands marked **hidden** are accessible by name but do not appear in `/help`.

For a narrative introduction, see [Slash Commands](../interactive/slash-commands.md) and [Custom Slash Commands](../extending/slash-commands.md).

---

## Session

| Command | Args | Description |
|---------|------|-------------|
| `/clear` | — | Clear the transcript and conversation history. Keeps system prompt, todos, plan-mode, and skills cache. |
| `/init` | `[--force]` | Generate a `CLAUDE.draft.md` from available context sources (`AGENTS.md`, `.cursorrules`, `.windsurfrules`, `README.md`, `git status`). Always writes the draft and never touches `CLAUDE.md`; review and rename it yourself. `--force` suppresses the "CLAUDE.md already exists" notice. |
| `/resume` | `[query]` | List persisted sessions sorted by most-recently-updated, with an optional case-insensitive substring filter. |
| `/recap` | — | Summarize the conversation so far without mutating history. |
| `/export` | `[path] [--format json]` | Export the session transcript to a file. Default format: Markdown. Default filename: `caliban-session-<date>.md` in the CWD. Pass `--format json` for JSON output. |
| `/btw` | `<question>` | One-shot ephemeral question to a fast model (routed as `FastClassifier`); result inlined to transcript without touching the main session. |

---

## Model & Auth

| Command | Args | Description |
|---------|------|-------------|
| `/model` | `[id]` | With no args: list the active provider's known model ids and the currently-selected one. With an id: switch the active model at runtime (same-provider in v1). |
| `/effort` | `<level>` | Set reasoning effort for the next turn. Values: `low`, `medium`, `high`, `max`, `auto`. |
| `/think` | `<on\|off\|auto\|budget>` | Toggle extended thinking independently of `/effort`. `auto` derives the budget from `/effort`; a bare `<budget>` (or `on <budget>`) sets an explicit token budget. No args reports the current setting. |
| `/status` | — | Show the active provider name. Full auth/subscription status is pending the Auth spec. |
| `/login` | — | Run the active provider's auth flow (full browser OAuth implementation pending the Auth spec). |
| `/logout` | — | Clear cached credentials for the active provider (pending the Auth spec). |
| `/setup-token` | — | Generate a long-lived Anthropic OAuth token for CI use (pending the Auth spec). |

---

## Permissions

| Command | Args | Description |
|---------|------|-------------|
| `/permissions` | — | Open the permissions overlay. Shows current mode, bypass-latch state, and runtime rules. Tab cycles mode; `d` deletes the selected rule. |

---

## Observability

| Command | Args | Description |
|---------|------|-------------|
| `/usage` | — | Show cumulative token and cost usage for this session, per model. |
| `/cost` | — | Show cumulative cost and a per-(provider, model) breakdown with cache savings. |
| `/context` | — | Show context window utilization and the top-N largest content blocks (by character count). |
| `/compact` | — | Trigger the configured compactor; reports dropped/summarized message count. |
| `/doctor` | `[--deep]` | Run startup-time health checks (settings, MCP, skills, hooks, auth). `--deep` adds provider auth pings. |

---

## Memory

| Command | Args | Description |
|---------|------|-------------|
| `/memory` | `[list\|show <slug>\|edit <slug>\|delete <slug> --force]` | View or edit memory tiers and auto-memory topic files. No args: show tier summary. `delete` (alias `rm`) only previews the deletion unless `--force` is given. |

---

## Configuration & Extensibility

| Command | Args | Description |
|---------|------|-------------|
| `/config` | — | Open the tabbed settings editor overlay. |
| `/hooks` | — | List configured hooks per event type with handler counts. |
| `/mcp` | — | Open the MCP server status overlay. |
| `/plugins` | — | List installed plugins with enable/disable status. |
| `/agents` | — | List sub-agents. (Full fleet overlay arrives with the sub-agent isolation spec; use `caliban agents list` from a shell for now.) |
| `/skills` | — | List skills loaded from `.caliban/skills/` and other configured roots. |

---

## Plan Mode

| Command | Args | Description |
|---------|------|-------------|
| `/plan` | — | Toggle plan mode. When ON, mutating tools are blocked. Reflected in the active session and statusline. |

---

## Output

| Command | Args | Description |
|---------|------|-------------|
| `/output-style` | — | Show the active output style and the available list. Change the style with `CALIBAN_OUTPUT_STYLE` and a restart (the `output_style` setting is not read yet). |

---

## Diagnostics

| Command | Args | Description |
|---------|------|-------------|
| `/rewind` | — | Open the checkpoint overlay (ADR 0028); also opened by Esc Esc on empty input. Pick a prompt with ↑/↓, then `c` restore code, `v` conversation, `b` both, `s`/`S` summarize after/up to, or `f` fork a new session. See [Checkpoints & Rewind](../memory/checkpoints.md). |
| `/statusline` | — | Show the active `statusLine.command` configuration (command, timeout, padding) or how to set one. |
| `/loop` | `[--n=<count>] [--interval=<seconds>]` | **Stub.** Reports the planned repeat count (default 3 every 15 s, bounded by `--max-turns`); the repeat execution is not implemented yet. |
| `/feedback` | — | **Stub.** Reports that no feedback endpoint is configured. |
| `/heapdump` | — | Capture a heap profile (requires caliban to be rebuilt with `--features=jemalloc-prof`). |
| `/tui` | — | Toggle fullscreen vs. default TUI mode (pending TUI ergonomics spec). |

---

## General

| Command | Args | Description |
|---------|------|-------------|
| `/help` | — | List all visible registered slash commands. |
| `/quit` | — | Exit caliban. |
| `/exit` | — | Alias for `/quit` (hidden). |

---

```admonish note title="Hidden commands"
`/exit`, `/plugin` (alias for `/plugins`), `/system` (view active system prompt), and `/voice` (reserved for future voice dictation) are registered but do not appear in `/help` output.
```

```admonish tip title="Custom slash commands"
You can add your own slash commands by placing skill files under `.caliban/skills/<name>/SKILL.md`. See [Custom Slash Commands](../extending/slash-commands.md).
```
