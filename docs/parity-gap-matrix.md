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

**Last refreshed:** 2026-05-31 (custom statusline: ticked row K — TUI render integration landed, `/statusline` reports active config. Prior refresh 2026-05-31 permissions-v2: updated Permissions rows to reference ADR-0045 + v2 spec; added "Permissions active management" row; updated Layered settings row notes. Prior refresh 2026-05-28 TODO/parity cleanup: validated the Plan A/B/C parity-sweep items against `main` and pruned the stale backlog; corrected the "TUI Ask modal" row to ✅ to match the shipped 4-button modal. Prior refresh 2026-05-26 after Plan C "TUI slash & UX polish": `/clear` resets context_window, `/effort` runtime, `/model` runtime swap, `/cost` breakdown, `/doctor` real checks + `caliban doctor` headless, `/resume` filter, `/context` top-N, `/export`, permission-modal 4-button + runtime rules, custom statusline runner).

## Design coverage

Every 🔴 row in this matrix has a proposed design doc as of 2026-05-24:

| Theme | Spec | ADR |
|---|---|---|
| A. Permissions/safety (v2 schema + TOML polarity + active management) | [`permissions-v2-design`](superpowers/specs/2026-05-31-permissions-v2-design.md) | [0045](../adrs/0045-permissions-v2-and-toml-primary-config.md) |
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
| Rule grammar (allow/ask/deny + globs) | ✅ | ADR-0020; v2 schema: ordered `[[permissions.rules]]` array with `pattern`/`action`/`comment`/`reason`/`expires_at`, globstar `**`, `Bash:~glob` anywhere-match, dotted-key MCP arg accessors — ADR-0045 / [v2 spec](superpowers/specs/2026-05-31-permissions-v2-design.md) |
| Permissions modes: `default`/`acceptEdits`/`plan`/`auto`/`dontAsk`/`bypassPermissions` | ✅ | ADR-0029; Shift+Tab cycles + status-bar chip; `--permission-mode` flag; `CALIBAN_DEFAULT_PERMISSION_MODE` env; `--allow-dangerously-skip-permissions` gate for bypass; `permissions.enforce = true` refuses bypass at startup (ADR-0045) |
| Permissions active management (CLI + TUI editor + modal writeback + audit log) | ✅ | ADR-0045 / [v2 spec](superpowers/specs/2026-05-31-permissions-v2-design.md); `caliban perms` CLI (list/test/explain/add/remove/import/export/audit/lint), `/permissions` overlay editor, modal scope picker with TOML writeback, JSONL decision log under `$XDG_STATE_HOME`, `permissions.enforce` lockdown, always-visible bypass-latch chip with `ctrl+shift+b` drop |
| Auto-mode (classifier-driven `environment`/`allow`/`soft_deny`/`hard_deny`) | ✅ | ADR-0029; `AutoModeClassifier` via router `RequestPurpose::FastClassifier` with `$defaults` curated rule lists, sha256-keyed cache, 4 KiB input truncation |
| TUI Ask modal | ✅ | ADR-0027 + Plan C; 4-button modal (Allow once / Always allow / Reject once / Always reject) — see row E "Permission Ask modal" |
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
| CLAUDE.md ancestor walk + nested-on-demand | ✅ | ADR-0036 |
| `@path/file` imports inside CLAUDE.md (recursion-bounded) | ✅ | ADR-0036 |
| Auto-memory (model-written notes per project) | ✅ | ADR-0035 |
| `claudeMdExcludes` for monorepos | ✅ | ADR-0036 |
| Auto-checkpoint per prompt + `/rewind` | ✅ | ADR-0028; new crate `caliban-checkpoint`; `before_run`/`after_run` hooks + `CheckpointHook` snapshots file-tool pre-images per prompt under `~/.caliban/projects/<cwd>/checkpoints/<session>/prompt-N/`; `/rewind` slash command opens the overlay |
| Esc-Esc / fork-from-checkpoint | ✅ | ADR-0028 — Esc-Esc on empty input opens the rewind overlay (`is_esc_chord` policy, 400 ms window). Fork-from-checkpoint stays 🔴 (sub-agent fleet spec) |
| MicroCompact (LLM-free per-tool supersession janitor) | ✅ | Plan B (`2026-05-26-context-management`); `MicroCompactor` strategy walks history each turn replacing superseded `ToolResult` blocks (per-tool key: `Read`→file_path, `Grep`/`Glob`→exact args, `WebFetch`→url; `Bash` never supersedable) with `[superseded: <tool>(<key>)]` placeholders |
| Tool-result size cap with overflow persistence | ✅ | Plan B; `ToolResultCap` (default 50_000 chars) writes overflow to `~/Library/Caches/caliban/tool-overflows/<session>/<tool_use_id>.txt`, replaces inline content with `[truncated: N chars, full at <path>]` + head/tail 2KB preview |

## D. Configuration / settings

| Capability | Caliban | Notes |
|---|---|---|
| Layered settings (managed / user / project / local) with merge semantics | ✅ | ADR-0026; new crate `caliban-settings` loads JSON/TOML at four canonical scopes with documented per-key merge rules + `--settings` / `--setting-sources` CLI flags + `parent_settings_behavior: "block"` lockdown. Legacy per-feature TOMLs (`permissions.toml`, `mcp.toml`, `hooks.toml`) still load when the unified file is absent. TOML restored as primary write format per ADR-0045; JSON accepted on read with WARN. `Settings.model` / `Settings.fallback_model` are consumed at startup via `EffectiveModel::resolve` (CLI > Settings > builtin default; provenance surfaced in `/config`). |
| `/config` interactive editor | ✅ | ADR-0026 (Phase 1); existing `/config` overlay now surfaces the merged effective settings + scope chain (provenance per key). Tabbed write-back editor lands with ADR 0040 slash registry. |
| Live reload (`ConfigChange` hook) | ✅ | ADR-0026; `SettingsWatcher` (notify, 250 ms debounce) fires on every scope file change; `ConfigChangeCtx` already exists in `caliban_agent_core::hooks`. `model` / `output_style` are flagged restart-required in the diff. |
| `apiKeyHelper` (dynamic auth refresh) | ✅ | ADR-0026; `ApiKeyHelperPool` invokes the helper script without a shell, caches per `refreshIntervalMs` (default 5 min, configurable via `CALIBAN_API_KEY_HELPER_TTL_MS`), and logs slow-helper warnings at `slowHelperWarningMs` (default 10 s). Wired into `startup::build_provider` and `router::build_one` for Anthropic / OpenAI / Google; provider construction wraps the inner adapter in `RefreshingProvider<P>` which invalidates the cached key and rebuilds the adapter on a 401/403 from the upstream, retrying the failed request once. |
| Schema validation (`https://json.schemastore.org/...`) | ✅ | ADR-0026; embedded schema at `caliban-settings/src/schema.json` validated via `jsonschema` 0.17 (Draft-7); invalid documents warn but don't abort (per spec). Forward-looking public path: `https://caliban.dev/schemas/settings/v1.json`. |

## E. TUI ergonomics

| Capability | Caliban | Notes |
|---|---|---|
| Status bar, plan-mode chip, spinner, elapsed | ✅ | |
| Mouse-wheel scroll, transcript | ✅ | |
| `@file` mention + autocomplete | ✅ | ADR-0027; gitignore-aware via `ignore` crate; submit-time attach with size cap |
| `!` shell escape | ✅ | ADR-0027; routes through `Bash` tool + `PermissionsHook` |
| External editor (`Ctrl+G` → `$VISUAL` / `$EDITOR`) | ✅ | ADR-0027; alt-screen suspend/resume around `$VISUAL`/`$EDITOR`/`vi` |
| Vim editing mode | 🔴 | |
| `Ctrl+O` transcript viewer + dump-to-scrollback | ✅ | ADR-0027; `q`/Esc close, `[` dump, `v` open-in-$VISUAL, scroll keys, `?` help |
| Background bash (`Ctrl+B`) | ✅ | `Bash{background:true}` + `BashOutput` + `KillShell`; TUI `Ctrl+B` follow-on |
| Image / vision input | ✅ | ADR-0039; `caliban-images` ingest (clipboard, `@path`, DnD), per-adapter wire shapes, capability filter + strict-routing fallback, blob storage, graphics-protocol detection |
| Slash-menu typeahead | 🟡 | |
| Permission Ask modal | ✅ | ADR-0027 + Plan C 2026-05-26: 4-button modal — `y` Allow once / `A` Always allow / `n` Reject once / `R` Always reject / Esc Deny. "Always" branches append session-scoped `RuntimeRule` via `RuntimeRuleStore` (no disk persistence). Pattern derived per-tool with `caliban_agent_core::derive_pattern`. |
| Reverse history search (`Ctrl+R` / `Ctrl+S`) | ✅ | ADR-0027; session → project → all-projects scopes; persisted per project |
| Multi-line input (`\`+Enter, Option+Enter, Shift+Enter native) | 🟡 | |
| Voice dictation | 🔴 | |

## F. Built-in tools

| Capability | Caliban | Notes |
|---|---|---|
| Bash, Edit, Glob, Grep, Read, Write, WebFetch, TodoWrite, Skill, AgentTool, EnterPlanMode/ExitPlanMode | ✅ | |
| WebSearch | ✅ | Brave/Tavily/Exa via env-toggle |
| NotebookEdit (Jupyter) | ✅ | nbformat v4; atomic write; FileChanged |
| MultiEdit semantics (atomic multi-replace) | ✅ | sequential + rollback-on-miss |
| PowerShell tool | 🔴 | low priority |
| `ToolSearch` (lazy MCP schema loading) | 🔴 | only matters once MCP is real |
| `WaitForMcpServers` | 🔴 | same |

## G. Sub-agents

| Capability | Caliban | Notes |
|---|---|---|
| In-process synchronous `AgentTool` + recursion guard | ✅ | ADR-0021 |
| Subagent in isolated git worktree | ✅ | ADR-0037 — `caliban-worktrees` crate; `isolation: worktree` frontmatter |
| Background subagents (`--bg`, `claude agents`, attach/respawn/rm) | ✅ | ADR-0037 — `caliban-supervisor` + `caliband` daemon + CLI |
| Subagent-local memory dir | ✅ | ADR-0037 — `<base>/agents/<id>/` per-agent session dir |
| Hook inheritance for subagents | ✅ | ADR-0037 — `inherit_hooks: true` default; closure hooks dropped with warn at process boundary |
| Subagent fleet supervisor daemon | ✅ | ADR-0037 — per-repo `caliband` over UDS |

## H. MCP

| Capability | Caliban | Notes |
|---|---|---|
| Config + name validation (caliban-mcp-client v1) | ✅ | ADR-0017 |
| Real spawn / handshake / `list_tools` (rmcp 1.7) | ✅ | ADR-0023 Phase A |
| HTTP / SSE transports | ✅ | ADR-0023 Phase B (rmcp streamable-http client; legacy SSE folded in) |
| Per-server permission scoping | ✅ | ADR-0023 Phase B (`[server.X.permissions]` composes with global rules) |
| `/mcp` slash + per-server enable/auth | ✅ | ADR-0023 Phase C — Phase C glyphs (●/◐/○), `d/r/a/s/t` key hints rendered |
| OAuth flow + `--mcp-oauth-port` | ✅ | ADR-0023 Phase C — PKCE + loopback, RFC 8414 discovery, manual config, keyring + file-store fallback |
| Elicitation (server-initiated input) | ✅ | ADR-0023 Phase C — `ElicitationBridge` mpsc → TUI, 5-min cap, auto-decline in `--print` |
| `${CLAUDE_PROJECT_DIR}` expansion in `.mcp.json` | ✅ | Implemented in Phase B `config::expand_value` (we use `mcp.toml` not `.mcp.json`) |
| `MCP_TIMEOUT` / `MCP_TOOL_TIMEOUT` / `MAX_MCP_OUTPUT_TOKENS` envs | ✅ | ADR-0023 — `CALIBAN_MCP_TIMEOUT`/`CALIBAN_MCP_TOOL_TIMEOUT` primary, `MCP_*` honoured for parity |
| Resources (`@server:resource` references) | ✅ | ADR-0023 Phase C — `McpResource` cache, `list_changed` invalidation, URI-template positional expansion |

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
| `/context` slash | ✅ | ADR-0033; per-message-kind breakdown + 80% warning |
| `/usage` slash + per-session token + $ | ✅ | ADR-0033; per-model breakdown + cache savings |
| `/compact` slash + manual trigger | ✅ | ADR-0033; routes through configured `Compactor` |
| Proactive autocompact (threshold-based + 2-strike backoff) | ✅ | Plan B (`2026-05-26-context-management`); fires when `estimate_tokens(history) / max_input_tokens >= auto_compact_threshold` (default 0.75); 2 consecutive failures disable autocompact for the run |
| Conversation-level prompt cache marker | ✅ | Plan B; `apply_prompt_cache` marks the last user message with `cache_control: Ephemeral` when its estimated tokens >= `min_cache_block_tokens` (default 1024), turning `cache_read` curve from flat to linear-with-history on Anthropic |
| Cost ($) tracking | ✅ | ADR-0033; `rust_decimal` math against vendored `rates.yaml` |
| OpenTelemetry export (OTLP metrics / logs / traces) | ✅ | ADR-0033; gated by `CALIBAN_ENABLE_TELEMETRY=1`, `OTEL_*` env contract honored; OTLP transport behind the `otlp` cargo feature |
| Metric set (`session.count`, `lines_of_code.count`, `cost.usage`, `token.usage`, etc.) | ✅ | ADR-0033; `caliban-telemetry::MetricEmitter` mirrors Claude Code's `claude_code.*` names |
| Turn-loop resilience (MaxTokens 2-stage recovery, stream-idle watchdog, refusal/content-filter surfacing, reactive-compact on ContextTooLong, failure-aware hook dispatch, TurnDecision) | ✅ | Plan A 2026-05-26; counter names exposed via `caliban_telemetry::metrics::RECOVERY_*` |
| `/doctor`, `/heapdump` diagnostics | 🟡 | `/doctor` real checks + `caliban doctor` headless shipped 2026-05-26 (Plan C Task 7); `/heapdump` still a stub naming the jemalloc-prof feature. |
| Status line (custom script) | ✅ | `StatuslineRunner` shipped 2026-05-26 (Plan C Task 12) in `caliban-settings`; TUI render-prefix integration landed 2026-05-31 — refreshed off-thread after each `TurnEnd`/`RunEnd`, cached so it never runs in the render path, prefixed onto the status bar; `/statusline` reports the active config. |
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
| `/plan`, `/memory`, `/skills`, `/quit` | ✅ | Ported to the `SlashCommand` trait (ADR 0040). |
| `/plugin`, `/plugins` | ✅ | ADR-0030; text overlay lists installed plugins with enable/disable status. Full interactive UI lands with the Plugin Manager overlay spec. |
| `/clear`, `/help`, `/init` | ✅ | ADR 0040; `/init` writes `CLAUDE.draft.md` from `AGENTS.md` / `.cursorrules` / `.windsurfrules` / `README.md` / `git status`. |
| `/context`, `/usage`, `/compact` | ✅ | ADR-0033 logic; surfaced through the registry as of ADR 0040. |
| `/config`, `/hooks`, `/mcp`, `/agents`, `/model`, `/effort` | ✅ | ADR 0040 + Plan C 2026-05-26: `/model <id>` now runtime-swaps via `Agent::try_swap_model` (same-provider in v1); `/effort low|medium|high|max|auto` writes `Arc<ArcSwap<Effort>>` consumed by OpenAI `reasoning.effort` and Anthropic `thinking.budget_tokens` on the next turn. `/agents` remains a stub. |
| `/resume`, `/recap`, `/btw`, `/loop` | ✅ | ADR 0040 + Plan C 2026-05-26: `/resume [query]` accepts a name substring filter; full picker-overlay swap-in-place deferred until Overlay enum supports non-Copy variants. |
| `/cost`, `/export` | ✅ | Plan C 2026-05-26 (Tasks 6 + 10): `/cost` prints cumulative + per-(provider,model) USD; `/export [path] [--format json]` writes the in-memory session transcript. |
| `/rewind` | ✅ | ADR-0028; overlay lists per-prompt checkpoints (newest first); Esc-Esc opens the same overlay; also reachable via `/rewind` through the ADR 0040 registry. |
| `/doctor`, `/heapdump`, `/feedback` | ✅ | ADR 0040; `/doctor` runs health checks (skills, hooks, MCP, provider, workspace); `/heapdump`/`/feedback` are stubs naming their ETA path. |
| `/login`, `/logout`, `/status` | ✅ | ADR 0040; stubs that name the Auth spec where each is wired. |
| `/statusline`, `/tui` | ✅ | ADR 0040; stubs that name the Settings hierarchy + TUI ergonomics specs. |
| `/theme` | 🔴 | Deferred per spec — TUI color customization. |
| `/code-review`, `/security-review`, `/review`, `/ultrareview` | 🔴 | (skill-level — depends on the Skills system polish sub-project) |
| `/run`, `/verify`, `/debug`, `/batch` | 🔴 | (bundled skills — same dependency) |

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
