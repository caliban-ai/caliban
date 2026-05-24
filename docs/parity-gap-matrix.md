# Caliban ↔ Claude Code parity gap matrix

> **What this is:** a living checklist of feature parity between caliban
> (this project) and Claude Code. Refresh it whenever a major feature
> lands or Claude Code ships a new capability. Use it to prioritize the
> next sprint.
>
> **How to use it:** when planning what to build next, look here first.
> When shipping a feature, tick its row(s) from 🔴 → 🟡 or 🟡 → ✅ in the
> same PR that ships the code.
>
> **Companion document:** [`claude-code-capability-inventory.md`](claude-code-capability-inventory.md)
> — a structured snapshot of Claude Code's documented surface, captured
> from the public docs (`docs.claude.com/en/docs/claude-code/*`). That
> file is the *source* this matrix is derived from; refresh both
> together.

**Legend:** ✅ parity · 🟡 partial · 🔴 gap · *(deferred)* = scoped in a
shipped PR's v2 follow-up notes.

**Last refreshed:** 2026-05-24 (after the 2026-05-24 design sweep — 18 new
ADRs + 19 new specs).

## Design coverage

Every 🔴 row in this matrix has a proposed design doc as of 2026-05-24:

| Theme | Spec | ADR |
|---|---|---|
| A. Permissions/safety (modes + auto-mode) | [`permission-modes-design`](superpowers/specs/2026-05-24-permission-modes-design.md) | [0029](../adrs/0029-permission-modes-and-auto-mode.md) |
| A. Permissions/safety (OS sandbox) | [`os-sandbox-design`](superpowers/specs/2026-05-24-os-sandbox-design.md) | [0032](../adrs/0032-os-sandbox.md) |
| B. Hooks (event surface + handlers) | [`hooks-expansion-design`](superpowers/specs/2026-05-24-hooks-expansion-design.md) | [0024](../adrs/0024-hook-event-taxonomy.md) |
| B. Plugins | [`plugin-system-design`](superpowers/specs/2026-05-24-plugin-system-design.md) | [0030](../adrs/0030-plugin-packaging.md) |
| C. Auto-memory | [`auto-memory-design`](superpowers/specs/2026-05-24-auto-memory-design.md) | [0035](../adrs/0035-auto-memory.md) |
| C. CLAUDE.md ancestry + `@`-imports | [`claudemd-ancestry-design`](superpowers/specs/2026-05-24-claudemd-ancestry-design.md) | [0036](../adrs/0036-claudemd-ancestry-and-imports.md) |
| C. Checkpointing + `/rewind` | [`checkpointing-design`](superpowers/specs/2026-05-24-checkpointing-design.md) | [0028](../adrs/0028-checkpointing-rewind.md) |
| D. Settings hierarchy + `/config` | [`settings-hierarchy-design`](superpowers/specs/2026-05-24-settings-hierarchy-design.md) | [0026](../adrs/0026-settings-layering.md) |
| E. TUI ergonomics (`@file`/`!`/`Ctrl+G`/Ask/transcript) | [`tui-ergonomics-design`](superpowers/specs/2026-05-24-tui-ergonomics-design.md) | [0027](../adrs/0027-tui-ergonomics.md) |
| E. Image / vision input | [`image-input-design`](superpowers/specs/2026-05-24-image-input-design.md) | [0039](../adrs/0039-image-and-vision-input.md) |
| F. Built-in tool gaps (WebSearch / NotebookEdit / MultiEdit / Bg-Bash) | [`builtin-tool-gaps-design`](superpowers/specs/2026-05-24-builtin-tool-gaps-design.md) | — |
| G. Sub-agent isolation + background fleet | [`subagent-worktree-and-fleet-design`](superpowers/specs/2026-05-24-subagent-worktree-and-fleet-design.md) | [0037](../adrs/0037-subagent-isolation-and-background-fleet.md) |
| H. MCP v2 (transports / OAuth / elicitation / resources) | [`mcp-v2-design`](superpowers/specs/2026-05-24-mcp-v2-design.md) | [0023](../adrs/0023-mcp-v2-transports-and-oauth.md) |
| I. Model router v2 (fallback/hedging/breakers/caps) | [`model-router-v2-design`](superpowers/specs/2026-05-24-model-router-v2-design.md) | [0038](../adrs/0038-model-router-v2.md) |
| I. Bedrock + Vertex providers | [`bedrock-vertex-providers-design`](superpowers/specs/2026-05-24-bedrock-vertex-providers-design.md) | [0034](../adrs/0034-bedrock-and-vertex-providers.md) |
| J. Headless `-p` + JSON output | [`headless-mode-design`](superpowers/specs/2026-05-24-headless-mode-design.md) | [0025](../adrs/0025-headless-output-protocol.md) |
| K. OTel export + cost accounting + `/usage` / `/context` / `/compact` | [`otel-and-cost-design`](superpowers/specs/2026-05-24-otel-and-cost-design.md) | [0033](../adrs/0033-opentelemetry-and-cost.md) |
| L. Output styles | [`output-styles-design`](superpowers/specs/2026-05-24-output-styles-design.md) | [0031](../adrs/0031-output-styles.md) |
| M. Slash command coverage (registry + ~24 commands) | [`slash-command-coverage-design`](superpowers/specs/2026-05-24-slash-command-coverage-design.md) | [0040](../adrs/0040-slash-command-registry.md) |

Long-tail surfaces in section N (IDE / GitHub App / web / iOS / Slack /
Remote Control / Channels / Routines / Deep links / Teleport) do **not**
have specs yet — they're parked until terminal/CLI parity is reached.

---

## A. Permissions & safety

| Capability | Caliban | Notes |
|---|---|---|
| Rule grammar (allow/ask/deny + globs) | ✅ | ADR-0020 |
| Permission modes: `default`/`acceptEdits`/`plan`/`auto`/`dontAsk`/`bypassPermissions` | 🟡 | plan-mode only |
| Auto-mode (classifier-driven `environment`/`allow`/`soft_deny`/`hard_deny`) | 🔴 | |
| TUI Ask modal | 🔴 | *(deferred PR #8)* |
| OS-level sandbox (Seatbelt / bubblewrap) | ✅ | ADR-0032; v1 ships macOS + Linux/WSL; Windows native deferred |

## B. Hooks & extensibility

| Capability | Caliban | Notes |
|---|---|---|
| `before_tool` / `after_tool` (in-process) | ✅ | |
| `SessionStart` / `SessionEnd` / `UserPromptSubmit` | ✅ | ADR-0024 |
| `PreCompact` / `PostCompact` | ✅ | ADR-0024 |
| `ConfigChange` / `CwdChanged` / `FileChanged` | ✅ | ADR-0024 |
| Subagent lifecycle events (`SubagentStart`/`Stop`, `TaskCreated`/`Completed`) | ✅ | ADR-0024 |
| `PermissionRequest` / `PermissionDenied` | ✅ | ADR-0024 |
| Hook decision protocol (JSON stdout / exit codes) | ✅ | ADR-0024 |
| Handler types: `command` / `http` / `mcp` / `prompt` / `agent` | ✅ | `command`+`http` fully wired; `mcp`/`prompt`/`agent` are v1 stubs that wire in ADRs 0023 / 0037 |
| Hook inheritance for subagents | 🟡 | `SubagentStart`/`Stop` fire from parent; per-subagent inheritance lands with ADR 0037 |
| Plugin packages (bundle skills + hooks + agents + MCP + output-styles) | ✅ | ADR-0030; `caliban-plugins` orchestrator parses `plugin.json`, expands `${CALIBAN_PLUGIN_ROOT}` (+ `${CLAUDE_PLUGIN_ROOT}` alias), namespaces items, and feeds existing loaders. Marketplace install + trust gating + `caliban plugin {install,list,enable,disable,remove,info,update}`. settings.json keys land with ADR 0026 (env-only for now). |

## C. Memory & checkpointing

| Capability | Caliban | Notes |
|---|---|---|
| Three-tier prompt prefix (global / project / auto) | ✅ | ADR-0018 |
| CLAUDE.md ancestor walk + nested-on-demand | 🟡 | |
| `@path/file` imports inside CLAUDE.md (recursion-bounded) | 🔴 | |
| Auto-memory (model-written notes per project) | ✅ | ADR-0035 |
| `claudeMdExcludes` for monorepos | 🔴 | |
| Auto-checkpoint per prompt + `/rewind` | 🔴 | |
| Esc-Esc / fork-from-checkpoint | 🔴 | |

## D. Configuration / settings

| Capability | Caliban | Notes |
|---|---|---|
| Layered settings (managed / user / project / local) with merge semantics | 🔴 | currently ad-hoc TOMLs |
| `/config` interactive editor | 🔴 | |
| Live reload (`ConfigChange` hook) | 🔴 | |
| `apiKeyHelper` (dynamic auth refresh) | 🔴 | |
| Schema validation (`https://json.schemastore.org/...`) | 🔴 | |

## E. TUI ergonomics

| Capability | Caliban | Notes |
|---|---|---|
| Status bar, plan-mode chip, spinner, elapsed | ✅ | |
| Mouse-wheel scroll, transcript | ✅ | |
| `@file` mention + autocomplete | 🔴 | |
| `!` shell escape | 🔴 | |
| External editor (`Ctrl+G` → `$VISUAL` / `$EDITOR`) | 🔴 | |
| Vim editing mode | 🔴 | |
| `Ctrl+O` transcript viewer + dump-to-scrollback | 🔴 | |
| Background bash (`Ctrl+B`) | 🔴 | |
| Image / vision input | 🔴 | |
| Slash-menu typeahead | 🟡 | |
| Permission Ask modal | 🔴 | *(deferred PR #8)* |
| Reverse history search (`Ctrl+R` / `Ctrl+S`) | 🟡 | |
| Multi-line input (`\`+Enter, Option+Enter, Shift+Enter native) | 🟡 | |
| Voice dictation | 🔴 | |

## F. Built-in tools

| Capability | Caliban | Notes |
|---|---|---|
| Bash, Edit, Glob, Grep, Read, Write, WebFetch, TodoWrite, Skill, AgentTool, EnterPlanMode/ExitPlanMode | ✅ | |
| WebSearch | 🔴 | |
| NotebookEdit (Jupyter) | 🔴 | |
| MultiEdit semantics (atomic multi-replace) | 🔴 | |
| PowerShell tool | 🔴 | low priority |
| `ToolSearch` (lazy MCP schema loading) | 🔴 | only matters once MCP is real |
| `WaitForMcpServers` | 🔴 | same |

## G. Sub-agents

| Capability | Caliban | Notes |
|---|---|---|
| In-process synchronous `AgentTool` + recursion guard | ✅ | ADR-0021 |
| Subagent in isolated git worktree | 🔴 | |
| Background subagents (`--bg`, `claude agents`, attach/respawn/rm) | 🔴 | |
| Subagent-local memory dir | 🔴 | |
| Hook inheritance for subagents | 🔴 | *(deferred PR #9)* |
| Subagent fleet supervisor daemon | 🔴 | |

## H. MCP

| Capability | Caliban | Notes |
|---|---|---|
| Config + name validation (caliban-mcp-client v1) | ✅ | ADR-0017 |
| Real spawn / handshake / `list_tools` (rmcp 1.7) | ✅ | ADR-0023 Phase A |
| HTTP / SSE transports | 🔴 | |
| `/mcp` slash + per-server enable/auth | 🔴 | |
| OAuth flow + `--mcp-oauth-port` | 🔴 | |
| Elicitation (server-initiated input) | 🔴 | |
| `${CLAUDE_PROJECT_DIR}` expansion in `.mcp.json` | 🔴 | |
| `MCP_TIMEOUT` / `MCP_TOOL_TIMEOUT` / `MAX_MCP_OUTPUT_TOKENS` envs | 🔴 | |
| Resources (`@server:resource` references) | 🔴 | |

## I. Model router & providers

| Capability | Caliban | Notes |
|---|---|---|
| Purpose-keyed routing | ✅ | ADR-0022 |
| Fallback chain, hedging, circuit breakers | ✅ | ADR-0038; `caliban-model-router` v2 (`fallback.rs`, `hedging.rs`, `breaker.rs`) |
| Capability-based filtering (vision / thinking / tool_use) | ✅ | ADR-0038; `capabilities.rs` derives needs + route requires |
| `caliban.toml` binary wiring | ✅ | ADR-0038; `discovery.rs` walk-up + binary `router::try_load` |
| Anthropic / OpenAI / Ollama / Google providers | ✅ | |
| Bedrock | ✅ | ADR-0034; `caliban-provider-bedrock` |
| Vertex | ✅ | ADR-0034; `caliban-provider-vertex` |
| Foundry | 🔴 | |
| Effort levels (`low`/`medium`/`high`) | ✅ | ADR-0038; per-route `effort` + `effort_map` |
| Extended-thinking toggle wiring | 🟡 | |

## J. Headless / CI

| Capability | Caliban | Notes |
|---|---|---|
| `-p` / `--print` mode | ✅ | ADR-0025; `caliban/src/headless/`, dispatches via `run_headless` in `caliban/src/main.rs` |
| `--output-format text` / `json` / `stream-json` | ✅ | ADR-0025; NDJSON frames with `system/init`, `message`, `tool_use`, `tool_result`, `text`, `hook_event`, `result` |
| `--input-format text` / `stream-json` | ✅ | ADR-0025; `parse_stream_json_payload` handles `user` and `control/interrupt` frames; 10 MiB stdin cap |
| `--max-turns`, `--max-budget-usd` | ✅ | `--max-turns` enforced by agent loop; `--max-budget-usd` parsed and persisted, placeholder cost (0.0) until ADR 0033 wires real pricing — flag warns when no-op |
| `--bare` (skip discovery; default in CI) | ✅ | Opt-in per ADR-0025; gates hooks/skills/MCP/auto-memory/CLAUDE.md loaders |
| `--json-schema` + structured output | ✅ | Best-effort local validation (top-level type, required fields, per-field types); native structured-output via router lands with ADR 0032 |
| `--include-partial-messages` / `--include-hook-events` | ✅ | Partial-messages emit `text` delta frames; hook events flow through outer `CompositeHooks` layer (`HeadlessHookSink`) |
| GitHub Actions workflow | 🔴 | Separate sub-project; gated on this landing |
| Devcontainer feature | 🔴 | Separate sub-project; gated on this landing |
| `claude doctor` from shell | 🔴 | Separate diagnostic command (K. Observability) |

## K. Observability / cost

| Capability | Caliban | Notes |
|---|---|---|
| `tracing` instrumentation under `caliban::*` targets | ✅ | |
| `--debug` + `--debug-file` | 🟡 | |
| `/context` slash | 🔴 | |
| `/usage` slash + per-session token + $ | 🔴 | |
| `/compact` slash + manual trigger | 🔴 | |
| Cost ($) tracking | 🔴 | |
| OpenTelemetry export (OTLP metrics / logs / traces) | 🔴 | |
| Metric set (`session.count`, `lines_of_code.count`, `cost.usage`, `token.usage`, etc.) | 🔴 | |
| `/doctor`, `/heapdump` diagnostics | 🔴 | |
| Status line (custom script) | 🔴 | |
| `feedbackSurveyRate` + `/feedback` | 🔴 | |

## L. Output styles

| Capability | Caliban | Notes |
|---|---|---|
| Default / Proactive / Explanatory / Learning | ✅ | ADR-0031; four built-ins ship in `caliban-output-styles`; selected via `CALIBAN_OUTPUT_STYLE` env until ADR 0026 settings hierarchy lands [^l-force] |
| Custom output-style files (frontmatter + body) | ✅ | ADR-0031; project (`<ws>/.caliban/output-styles/`) > user (XDG) > plugin > built-in [^l-force] |

[^l-force]: `force_for_plugin: true` is parsed from frontmatter and routed through `select_active`, but inert in v1 — no plugins ship until ADR 0030 (plugin packaging) lands.

## M. Slash command coverage

| Command | Caliban | Notes |
|---|---|---|
| `/plan`, `/memory`, `/skills`, `/quit` | ✅ | |
| `/plugin`, `/plugins` | ✅ | ADR-0030; text overlay lists installed plugins with enable/disable status. Full interactive UI lands with ADR 0040. |
| `/clear`, `/help`, `/init` | 🔴 | |
| `/context`, `/usage`, `/compact` | 🔴 | |
| `/config`, `/hooks`, `/mcp`, `/agents`, `/model`, `/effort` | 🔴 | |
| `/resume`, `/recap`, `/btw`, `/loop` | 🔴 | |
| `/rewind` | 🔴 | |
| `/doctor`, `/heapdump`, `/feedback` | 🔴 | |
| `/login`, `/logout`, `/status` | 🔴 | |
| `/statusline`, `/theme`, `/tui` | 🔴 | |
| `/code-review`, `/security-review`, `/review`, `/ultrareview` | 🔴 | (skill-level) |
| `/run`, `/verify`, `/debug`, `/batch` | 🔴 | (bundled skills) |

## N. Long-tail surfaces (cloud / IDE / mobile)

All 🔴, all large investments. Tracking here only so we remember they exist:
IDE extension (VS Code / Cursor / JetBrains), GitHub App,
claude.ai/code web, iOS app, Slack integration, Remote Control,
Channels (research preview), Routines (scheduled remote agents), Deep
links, Teleport.

---

## Tier ordering (refresh when shipping)

**Tier 1 — Foundation lift (unlocks everything downstream):**
1. Hook event surface expansion (B)
2. Settings hierarchy + `/config` (D)
3. Headless `-p` mode + JSON output (J)

**Tier 2 — High-visibility UX:**
4. TUI ergonomics pack (E)
5. Slash command coverage (K, M)
6. Checkpointing + `/rewind` (C)

**Tier 3 — Capability gaps:**
7. Real MCP wiring (H)
8. Permission modes + auto-mode (A)
9. Plugin system (B last row)

**Tier 4 — Production hardening:**
10. OS sandbox (A)
11. OpenTelemetry export + cost (K)
12. Bedrock + Vertex providers (I)

**Tier 5 — Long-tail:**
Auto-memory, image input, vim mode, NotebookEdit, WebSearch, background
subagents fleet, GitHub Actions, devcontainer, status line, output
styles, etc.

---

## Refresh process

1. When a feature lands: edit the relevant row(s) in this matrix in the
   same PR, ticking 🔴 → 🟡 or 🟡 → ✅ as appropriate.
2. When Claude Code ships something new: refresh
   [`claude-code-capability-inventory.md`](claude-code-capability-inventory.md)
   first (re-fetch the upstream docs), then propagate any new rows here.
3. Bump the **Last refreshed** date at the top.
