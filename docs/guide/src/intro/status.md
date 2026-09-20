# Project Status

Caliban v0.12.0 is a pre-1.0 release. The binary (`caliban`) is daily-usable from `main`; the
core agent loop, TUI, headless mode, sessions, permissions, tools, MCP, sub-agents, memory,
sandbox, telemetry, and the driveable server surfaces are all shipped. A number of parity
gaps with Claude Code remain. Per-release detail is in the [Changelog](../changelog.md).

## What is shipped

The table below summarizes the major shipped areas. All items marked ✅ are available on
`main` today.

| Area | Status |
|---|---|
| Interactive TUI (ratatui, transcript, status bar, slash menu, `@file` picker) | ✅ |
| Headless `--print` / `stream-json` I/O protocol | ✅ |
| Persistent named sessions (`--session`, `--resume`, `--continue`) | ✅ |
| Permissions: rule grammar, six modes, `caliban perms` CLI, audit log | ✅ |
| Built-in tools (Read, Write, Edit, MultiEdit, Glob, Grep, Bash, BashOutput, KillShell, WebFetch, WebSearch, NotebookEdit, TodoWrite, AgentTool, Enter/ExitPlanMode, Read/WriteMemoryTopic, ToolSearch, Skill) | ✅ |
| Image / vision input (engine built in `caliban-images`; not yet wired into the binary) | 🟡 |
| MCP client (stdio + HTTP, OAuth, elicitation, per-server permissions) | ✅ |
| Sub-agents (in-process, background fleet via `caliband`, worktree isolation) | ✅ |
| Memory tiers: `CLAUDE.md` ancestry, `@`-imports, auto-memory | ✅ |
| Settings layering (Managed > User > Project > Local, deep-merge) | ✅ |
| Model router v2 (declarative routes, fallback chains, circuit breakers, capability filters) | ✅ |
| Providers in the binary: Anthropic, OpenAI, Google Gemini (+ local OpenAI-compatible servers); Bedrock/Vertex/Azure adapters are library-only | ✅ |
| Checkpoints + `/rewind`, including forking a new session from a checkpoint | ✅ |
| Driveable server surfaces: `caliban mcp serve`, `caliban acp serve`, `caliban http serve` | ✅ |
| `caliband` graceful drain + resume of daemon agents from a persisted session (a supervisor control-protocol command for orchestrators; there is no `caliban agents` subcommand for it) | ✅ |
| Prebuilt macOS arm64 (`aarch64-apple-darwin`) binary on tagged releases | ✅ |
| Plugins, hooks, skills | ✅ |
| OS sandbox (Seatbelt on macOS, bubblewrap on Linux) | ✅ |
| OpenTelemetry + per-request cost tracking | ✅ |

## What is partial or backlog

Some rows in the parity matrix are 🟡 (partial / experimental):

| Area | State |
|---|---|
| Vim editing mode in TUI | 🔴 not yet |
| Cost surfacing in TUI (`/cost` display) | 🟡 backlog |
| GitHub Actions workflow / devcontainer feature | 🔴 planned |
| IDE extensions, GitHub App, remote control, mobile (theme N) | 🔴 parked until CLI parity |

```admonish warning title="Theme N surfaces are parked"
IDE extensions, the GitHub App, claude.ai/code, iOS, Slack integration, Remote Control,
Channels, Routines, Deep links, and Teleport are all tracked in the parity matrix under
theme N. They are explicitly parked until the terminal/CLI feature set reaches full parity
with Claude Code. Do not rely on any of these surfaces being available in the near term.
```

For the full up-to-date breakdown, see [Parity vs Claude Code](../appendix/parity.md).
If you hit something unexpected, see [Troubleshooting](../troubleshooting.md).

## License

Caliban is licensed under AGPL-3.0-only. See [Philosophy](./philosophy.md) for the rationale.
