# Parity vs Claude Code

Caliban tracks feature parity with Claude Code in a living matrix. This page summarises the current state by theme. The full matrix — including per-row notes and ADR cross-references — lives at [`docs/parity-gap-matrix.md`](https://github.com/johnford2002/caliban/blob/main/docs/parity-gap-matrix.md) in the repository.

**Legend:** ✅ parity · 🟡 partial · 🔴 not yet

---

## Theme summary

### A — Permissions & safety ✅

Rule grammar (allow/ask/deny + globs), all six permission modes, the auto-mode classifier, the TUI Ask modal, OS-level sandbox (macOS Seatbelt + Linux bubblewrap), and the full `caliban perms` CLI with TOML writeback and audit log are all shipped. See ADRs 0020, 0029, 0032, and 0045.

### B — Hooks & extensibility ✅

All hook event types (tool, session, compact, config, cwd, file, subagent, permission), hook decision protocol, and plugin packaging are shipped. The `mcp`/`prompt`/`agent` handler types are v1 stubs; per-subagent hook inheritance lands with the fleet spec.

### C — Memory & checkpointing ✅

Three-tier prompt prefix, CLAUDE.md ancestor walk + `@`-imports, auto-memory, `claudeMdExcludes`, auto-checkpoint per prompt, `/rewind`, MicroCompact janitor, and tool-result size cap with overflow persistence are all shipped.

### D — Configuration / settings ✅

Layered settings (managed > user > project > local), `/config` interactive editor, live reload, `apiKeyHelper` pool, and schema validation are shipped (ADR 0026 + 0045). TOML is the primary write format; JSON is accepted on read.

### E — TUI ergonomics 🟡

Status bar, mouse scroll, transcript viewer, `@file` attach, `!` shell escape, external editor (`Ctrl+G`), `Ctrl+O` transcript dump, background bash (`Ctrl+B`), image/vision input, permission Ask modal, and reverse history search are shipped. Notable gaps: vim editing mode (🔴), slash-menu typeahead (🟡 partial), multi-line input (🟡 partial), and voice dictation (🔴).

### F — Built-in tools ✅

Bash, Edit, Glob, Grep, Read, Write, WebFetch, TodoWrite, Skill, AgentTool, NotebookEdit, MultiEdit, WebSearch, and background-bash are shipped. PowerShell tool and `ToolSearch` / `WaitForMcpServers` (relevant once MCP is fully real) are 🔴.

### G — Sub-agents ✅

In-process `AgentTool`, git worktree isolation, background agent fleet (`caliband` daemon), per-agent memory dir, hook inheritance, and supervisor daemon are all shipped (ADR 0037).

### H — MCP ✅

Config validation, real spawn/handshake, stdio + HTTP/SSE + streamable-HTTP transports, per-server permission scoping, `/mcp` slash, OAuth PKCE flow, elicitation, and resource references are shipped (ADR 0023).

### I — Model router & providers ✅

Purpose-keyed routing, fallback chains, hedging, circuit breakers, capability filtering, Anthropic/OpenAI/Ollama/Google/Bedrock/Vertex providers, and effort levels are shipped. Azure Foundry is 🔴; extended-thinking toggle is 🟡 partial.

### J — Headless / CI ✅

`-p` / `--print` mode, all output formats (`text`/`json`/`stream-json`), input formats, `--max-turns`, `--max-budget-usd`, `--bare`, `--json-schema`, `--include-partial-messages`, and `--include-hook-events` are shipped. GitHub Actions workflow and devcontainer feature are 🔴 (separate sub-projects).

### K — Observability / cost ✅

`tracing` instrumentation, `/context`, `/usage`, `/compact`, proactive autocompact, prompt cache markers, cost tracking, OpenTelemetry export, and the custom status line are shipped. `--debug` / `--debug-file` is 🟡 partial. The feedback survey is 🔴.

### L — Output styles ✅

All four built-in output styles (Default, Proactive, Explanatory, Learning) and custom output-style files are shipped (ADR 0031).

### M — Slash command coverage 🟡

Core commands (`/plan`, `/memory`, `/skills`, `/quit`, `/clear`, `/help`, `/init`, `/context`, `/usage`, `/compact`, `/config`, `/hooks`, `/mcp`, `/model`, `/effort`, `/resume`, `/cost`, `/export`, `/rewind`, `/doctor`, `/login`, `/logout`, `/status`) are shipped. Theme customisation and skill-dependent commands (`/code-review`, `/run`, `/verify`, `/batch`) are 🔴.

### N — Long-tail surfaces 🔴

IDE extensions (VS Code / Cursor / JetBrains), GitHub App, claude.ai/code web, iOS app, Slack, Remote Control, Channels, Routines, Deep links, and Teleport are all 🔴. These are parked until terminal/CLI parity is reached.

---

## Notable gaps

| Gap | Status | Notes |
|---|---|---|
| Vim editing mode | 🔴 | TUI input layer |
| Azure Foundry provider | 🔴 | Provider adapter not yet written |
| GitHub Actions workflow | 🔴 | Separate sub-project |
| Devcontainer feature | 🔴 | Separate sub-project |
| `ToolSearch` / `WaitForMcpServers` | 🔴 | Only relevant once MCP is fully real |
| Skill-dependent slash commands | 🔴 | `/code-review`, `/run`, `/verify`, `/batch` |
| Cloud / IDE / mobile surfaces (N) | 🔴 | All large investments; deferred |

```admonish note
The parity matrix is refreshed in the same PR that ships each feature. If a row above contradicts what you see in the matrix file, the matrix file is authoritative.
```
