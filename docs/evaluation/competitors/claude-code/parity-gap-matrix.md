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
> **Companion document:** [`capability-inventory.md`](capability-inventory.md)
> — a structured snapshot of Claude Code's documented surface, captured
> from the public docs (`docs.claude.com/en/docs/claude-code/*`). That
> file is the *source* this matrix is derived from; refresh both
> together.

**Legend:** ✅ parity · 🟡 partial · 🔴 gap · *(deferred)* = scoped in a
shipped PR's v2 follow-up notes.

> **A ✅ means the capability is reachable by a user of the shipped binary.**
> Machinery that exists, compiles, and is unit-tested but has no production
> call site is **🟡 at best** — the commonest single cause among the
> 2026-08-15 pass's 18 down-ticks. Cite a file path, ADR, or PR in the Notes
> column for every tick.
>
> **Counting convention** (so the numbers below are reproducible): a
> *down-tick* is any row whose status got worse than the row that covered it
> before, **including** a combined row split into worse-scoring halves — the
> row's state got worse either way. An *up-tick* is the reverse. Deleting a
> duplicate row is neither. Counts are of **capability-table rows in §A–§N**,
> not raw emoji: the pre-2026-08-15 header counted emoji and so swept in the
> legend and prose, reading "110/14/18" for what was really 103/5/11 rows. The
> status-audit table under *Tier ordering* scores old roadmap items rather
> than capabilities and is excluded from every count here.

**Last refreshed:** 2026-08-15 (primary-source re-baseline of **both** halves,
matching the 2026-07-27 standard set for the other competitors in #505.
**(a) Upstream:** [`capability-inventory.md`](capability-inventory.md)
re-captured from `docs.claude.com` / `code.claude.com` at Claude Code
**v2.1.233** — the doc site grew ~24 → 100+ pages, three slugs moved
(`iam`→`authentication`, `ide-integrations`→`vs-code`+`jetbrains`,
`sdk/overview`→`agent-sdk/overview`), and new upstream surface was propagated
into sections A/B/C/D/E/F/G/H/I/J/K/M/N below. **(b) Caliban:** every 🔴/🟡 row
plus ~20 ✅ rows re-verified against `main` at v0.8.0 (165 PRs merged since the
last tick). Row counts moved **103 ✅ / 5 🟡 / 11 🔴 → 104 ✅ / 24 🟡 / 26 🔴**
(119 → 154 rows, **+35 net**; 29 carry an explicit **New row** marker, the
other 6 are marked *Previously unrowed* or *Split from* — two combined §M rows
became four, and four capabilities were promoted out of another row's notes).
**2 up-ticks** (`caliban doctor`
🔴→✅, GitHub Actions 🔴→🟡) and **18 down-ticks** — the recurring pattern is
machinery built and tested but never wired to a production call site (OTel
metric emitters #467, OTLP mTLS #465, sandbox domain ACLs #477, image ingest,
structured-output wire mode, the `ConfigChange`/`CwdChanged`/`Notification`
hooks, and the whole settings live-reload watcher). Also deletes one duplicate
row (§G's ✅ "Hook inheritance for subagents", which contradicted §B's 🟡 —
that deletion is neither an up- nor a down-tick). Prior refresh 2026-06-17 (#15 slash-menu
typeahead: ticked row E "Slash-menu typeahead" 🟡 → ✅; `SlashCommandRegistry::suggest`
now does case-insensitive fuzzy subsequence matching with start/word-boundary +
contiguity ranking, superseding plain substring filtering. Prior refresh
2026-06-17 (#101 multi-line input: ticked row E "Multi-line input (`\`+Enter,
Option+Enter, Shift+Enter native)" 🟡 → ✅; trailing-backslash continuation
wired into the plain-Enter handler (Shift/Alt+Enter already shipped). Prior
refresh 2026-06-13 (#100 extended-thinking toggle: ticked row I
"Extended-thinking toggle wiring" 🟡 → ✅; `/think` runtime control decoupled
from `/effort`, honored on the Anthropic + OpenAI wire. Prior refresh
2026-06-01 two-stage tool surface — ticked F.ToolSearch + F.WaitForMcpServers
🔴 → 🟡 per ADR-0046; v1 machinery shipped opt-in via `tools.lazy_mcp`. Prior
refresh 2026-05-31 custom statusline: ticked row K — TUI render integration
landed, `/statusline` reports active config. Prior refresh 2026-05-31
permissions-v2: updated Permissions rows to reference ADR-0045 + v2 spec; added
"Permissions active management" row; updated Layered settings row notes. Prior
refresh 2026-05-28 TODO/parity cleanup: validated the Plan A/B/C parity-sweep
items against `main` and pruned the stale backlog; corrected the "TUI Ask modal"
row to ✅ to match the shipped 4-button modal. Prior refresh 2026-05-26 after
Plan C "TUI slash & UX polish": `/clear` resets context_window, `/effort`
runtime, `/model` runtime swap, `/cost` breakdown, `/doctor` real checks +
`caliban doctor` headless, `/resume` filter, `/context` top-N, `/export`,
permission-modal 4-button + runtime rules, custom statusline runner).

## Design coverage

Every 🔴 row in this matrix had a proposed design doc as of 2026-05-24. The
table below adds the eight ADRs accepted since (0047–0054), which had no
Design-coverage entry before this refresh. **All relative links verified to
resolve as of 2026-08-15.**

| Theme | Spec | ADR |
|---|---|---|
| A. Permissions/safety (v2 schema + TOML polarity + active management) | [`permissions-v2-design`](../../../superpowers/specs/2026-05-31-permissions-v2-design.md) | [0045](../../../adr/0045-permissions-v2-and-toml-primary-config.md) |
| A. Permissions/safety (modes + auto-mode) | [`permission-modes-design`](../../../superpowers/specs/2026-05-24-permission-modes-design.md) | [0029](../../../adr/0029-permission-modes-and-auto-mode.md) |
| A. Permissions/safety (OS sandbox) | [`os-sandbox-design`](../../../superpowers/specs/2026-05-24-os-sandbox-design.md) | [0032](../../../adr/0032-os-sandbox.md) |
| A. Workspace write fence (default-restricted) | [`workspace-write-fence-design`](../../../superpowers/specs/2026-07-03-workspace-write-fence-design.md) | [0048](../../../adr/0048-workspace-default-restricted.md) |
| A. Sandbox egress confinement | [`sandbox-egress-confinement-design`](../../../superpowers/specs/2026-07-12-sandbox-egress-confinement-design.md) | [0054](../../../adr/0054-sandbox-confinement-posture.md) |
| B. Hooks (event surface + handlers) | [`hooks-expansion-design`](../../../superpowers/specs/2026-05-24-hooks-expansion-design.md) | [0024](../../../adr/0024-hook-event-taxonomy.md) |
| B. Hooks (config-hook execution bridge) | [`config-hook-execution-bridge-design`](../../../superpowers/specs/2026-06-14-config-hook-execution-bridge-design.md) | — |
| B. Plugins | [`plugin-system-design`](../../../superpowers/specs/2026-05-24-plugin-system-design.md) | [0030](../../../adr/0030-plugin-packaging.md) |
| C. Auto-memory | [`auto-memory-design`](../../../superpowers/specs/2026-05-24-auto-memory-design.md) | [0035](../../../adr/0035-auto-memory.md) |
| C. CLAUDE.md ancestry + `@`-imports | [`claudemd-ancestry-design`](../../../superpowers/specs/2026-05-24-claudemd-ancestry-design.md) | [0036](../../../adr/0036-claudemd-ancestry-and-imports.md) |
| C. Checkpointing + `/rewind` | [`checkpointing-design`](../../../superpowers/specs/2026-05-24-checkpointing-design.md) | [0028](../../../adr/0028-checkpointing-rewind.md) |
| C. Path locations (XDG-first) | — | [0050](../../../adr/0050-xdg-first-path-locations.md) |
| D. Settings hierarchy + `/config` | [`settings-hierarchy-design`](../../../superpowers/specs/2026-05-24-settings-hierarchy-design.md) | [0026](../../../adr/0026-settings-layering.md) |
| E. TUI ergonomics (`@file`/`!`/`Ctrl+G`/Ask/transcript) | [`tui-ergonomics-design`](../../../superpowers/specs/2026-05-24-tui-ergonomics-design.md) | [0027](../../../adr/0027-tui-ergonomics.md) |
| E. Image / vision input | [`image-input-design`](../../../superpowers/specs/2026-05-24-image-input-design.md) | [0039](../../../adr/0039-image-and-vision-input.md) |
| F. Built-in tool gaps (WebSearch / NotebookEdit / MultiEdit / Bg-Bash) | [`builtin-tool-gaps-design`](../../../superpowers/specs/2026-05-24-builtin-tool-gaps-design.md) | — |
| F. Two-stage tool surface (`ToolSearch`) | [`two-stage-tool-surface`](../../../superpowers/plans/2026-05-31-two-stage-tool-surface.md) | [0046](../../../adr/0046-two-stage-tool-surface.md) |
| G. Sub-agent isolation + background fleet | [`subagent-worktree-and-fleet-design`](../../../superpowers/specs/2026-05-24-subagent-worktree-and-fleet-design.md) | [0037](../../../adr/0037-subagent-isolation-and-background-fleet.md) |
| G. Interactive background sub-agents | [`interactive-background-subagents-design`](../../../superpowers/specs/2026-06-10-interactive-background-subagents-design.md) | [0047](../../../adr/0047-interactive-background-subagents.md) |
| G. caliband network transport | [`caliband-authn-tls-hardening-design`](../../../superpowers/specs/2026-07-06-caliband-authn-tls-hardening-design.md) | [0051](../../../adr/0051-caliband-network-transport.md) |
| G. Workspace-scoped caliband | — | [0052](../../../adr/0052-workspace-scoped-caliband.md) |
| H. MCP v2 (transports / OAuth / elicitation / resources) | [`mcp-v2-design`](../../../superpowers/specs/2026-05-24-mcp-v2-design.md) | [0023](../../../adr/0023-mcp-v2-transports-and-oauth.md) |
| I. Model router v2 (fallback/hedging/breakers/caps) | [`model-router-v2-design`](../../../superpowers/specs/2026-05-24-model-router-v2-design.md) | [0038](../../../adr/0038-model-router-v2.md) |
| I. Bedrock + Vertex providers | [`bedrock-vertex-providers-design`](../../../superpowers/specs/2026-05-24-bedrock-vertex-providers-design.md) | [0034](../../../adr/0034-bedrock-and-vertex-providers.md) |
| J. Headless `-p` + JSON output | [`headless-mode-design`](../../../superpowers/specs/2026-05-24-headless-mode-design.md) | [0025](../../../adr/0025-headless-output-protocol.md) |
| J. Result-frame enrichment (Claude Code contract) | [`result-frame-enrichment-design`](../../../superpowers/specs/2026-07-03-result-frame-enrichment-design.md) | [0049](../../../adr/0049-result-frame-cc-enrichment.md) |
| K. OTel export + cost accounting + `/usage` / `/context` / `/compact` | [`otel-and-cost-design`](../../../superpowers/specs/2026-05-24-otel-and-cost-design.md) | [0033](../../../adr/0033-opentelemetry-and-cost.md) |
| K. OTel GenAI semconv vocabulary | — | [0053](../../../adr/0053-otel-genai-semconv-only.md) |
| L. Output styles | [`output-styles-design`](../../../superpowers/specs/2026-05-24-output-styles-design.md) | [0031](../../../adr/0031-output-styles.md) |
| M. Slash command coverage (registry + ~40 commands) | [`slash-command-coverage-design`](../../../superpowers/specs/2026-05-24-slash-command-coverage-design.md) | [0040](../../../adr/0040-slash-command-registry.md) |

**Design-coverage gaps (no spec, no ADR):**

- **Auth surface** — `/login`, `/logout`, `/status`, `/setup-token` are all
  stubs that defer to "the Auth spec", and no such spec exists in
  `docs/superpowers/specs/`. This is now the single largest stub cluster in
  the matrix (§M).
- **Native structured output** — `--json-schema` defers to "ADR 0032", but
  0032 is the OS sandbox. No ADR covers provider-side structured output.
- **Storage substrate / gonzalo facade** — 0.8.0's headline feature (three
  specs, #470/#471/#473) has no ADR and no row here. Deliberate: it has no
  Claude Code analogue, so it is out of scope for *this* matrix.

Long-tail surfaces in section N (IDE / GitHub App / web / iOS / Slack /
Remote Control / Channels / Routines / Deep links / Teleport / and the
2026-08-15 additions) do **not** have specs yet — they're parked until
terminal/CLI parity is reached.

---

## A. Permissions & safety

| Capability | Caliban | Notes |
|---|---|---|
| Rule grammar (allow/ask/deny + globs) | ✅ | ADR-0020; v2 schema: ordered `[[permissions.rules]]` array with `pattern`/`action`/`comment`/`reason`/`expires_at`, globstar `**`, `Bash:~glob` anywhere-match, dotted-key MCP arg accessors — ADR-0045 / [v2 spec](../../../superpowers/specs/2026-05-31-permissions-v2-design.md). Hardened 0.4.0–0.6.0: `deny:mcp__*` outranks server allows (#213), static Deny preserved under acceptEdits (#169), unparseable config fails closed (#410) |
| Permissions modes: `default`/`acceptEdits`/`plan`/`auto`/`dontAsk`/`bypassPermissions` | ✅ | ADR-0029; Shift+Tab cycles + status-bar chip; `--permission-mode` flag; `CALIBAN_DEFAULT_PERMISSION_MODE` env; `--allow-dangerously-skip-permissions` gate for bypass; `permissions.enforce = true` refuses bypass at startup (ADR-0045); lockdown refuses mode-weakening flags (#178). Upstream added a `manual` alias for `default` (v2.1.200) — cosmetic, not adopted |
| Permissions active management (CLI + TUI editor + modal writeback + audit log) | ✅ | ADR-0045 / [v2 spec](../../../superpowers/specs/2026-05-31-permissions-v2-design.md); `caliban perms` CLI (list/test/explain/add/remove/import/export/audit/lint), `/permissions` overlay editor, modal scope picker with TOML writeback, JSONL decision log under `$XDG_STATE_HOME`, `permissions.enforce` lockdown, always-visible bypass-latch chip with `ctrl+shift+b` drop |
| Auto-mode (classifier-driven `environment`/`allow`/`soft_deny`/`hard_deny`) | ✅ | ADR-0029; `AutoModeClassifier` via router `RequestPurpose::FastClassifier` with `$defaults` curated rule lists, sha256-keyed cache, 4 KiB input truncation |
| TUI Ask modal | ✅ | ADR-0027 + Plan C; 4-button modal — see row E "Permission Ask modal" |
| Workspace write fence (default-restricted) | ✅ | **New row (ADR-0048, #237/#273).** `--workspace` restricts file writes to the workspace unless `--no-restrict-paths`; `..` collapsed before the fence check (#327); relative edit patterns resolve against the workspace root (#177) |
| OS-level sandbox (Seatbelt / bubblewrap) | 🟡 | **Down-ticked 2026-08-15.** ADR-0032; both backends real (`crates/caliban-sandbox/{seatbelt,bwrap,detect}.rs`), macOS + Linux/WSL, Windows native deferred; 0.6.0 hardening sweep closed real fail-opens (#402/#407/#415/#476). **Gap:** the runtime policy is *hardcoded* in `caliban/src/startup/compose.rs::workspace_fence_policy` — Claude Code's `filesystem.allow/denyRead|Write`, `httpProxyPort`, `socksProxyPort`, `allowUnixSockets`, `allowMachLookup`, `bwrapPath` have **no user-facing settings surface**. `SandboxSettings` (`crates/caliban-settings/src/settings.rs:69`) has exactly one field, `network` |
| Sandbox network egress control (`allowedDomains`/`deniedDomains`, proxy ports) | 🟡 | **New row (ADR-0054, #406/#480) — breaking in 0.7.0.** `--workspace` denies egress by default, loopback preserved; opt out via `--sandbox-network=allow` or `sandbox.network`. All-or-nothing: **no per-hostname allowlist** — `validate_policy` (`crates/caliban-sandbox/src/shim.rs:360`) hard-rejects `allowed_domains`/`denied_domains` without a proxy port, and no loopback proxy ships, so neither list is usable. Neither backend can filter by name (`seatbelt.rs:133`, `bwrap.rs:89`). Tracked in #477; credential-store `deny_read` in #481 |
| Sandboxed-child environment scrubbing | ✅ | **New row (#405/#482).** Secret-named vars (`*KEY*`, `*SECRET*`, `*TOKEN*`, `*PASSWORD*`, `*CREDENTIAL*`, `OTEL_EXPORTER_OTLP_HEADERS`) dropped from a sandboxed Bash command's environment; `[sandbox.env] passthrough` keeps named vars (`crates/caliban-sandbox/src/config.rs:62`). Name-based filter — a secret in an innocuously-named var is not caught |

## B. Hooks & extensibility

| Capability | Caliban | Notes |
|---|---|---|
| `before_tool` / `after_tool` (in-process) | ✅ | |
| `SessionStart` / `SessionEnd` / `UserPromptSubmit` | ✅ | ADR-0024 (in-process surface) |
| `PreCompact` / `PostCompact` | ✅ | ADR-0024 (in-process surface) |
| `ConfigChange` / `CwdChanged` / `FileChanged` | 🟡 | **Down-ticked 2026-08-15.** Only **`FileChanged`** is dispatched (`crates/caliban-agent-core/src/tool.rs:50`). `Hooks::config_change` and `Hooks::cwd_changed` are declared with their `Ctx` types in `hooks.rs` but have **zero `.config_change(` / `.cwd_changed(` call sites anywhere in the workspace** — not even in tests. Same shape as the image-ingest and metric-emitter gaps: the surface exists, nothing fires it |
| Subagent lifecycle events (`SubagentStart`/`Stop`, `TaskCreated`/`Completed`) | ✅ | ADR-0024 (in-process surface) |
| `PermissionRequest` / `PermissionDenied` | ✅ | ADR-0024 (in-process surface) |
| Hook decision protocol (JSON stdout / exit codes) | ✅ | ADR-0024; exit-2 denials no longer swallowed (#171) |
| `SessionStart` context injection (`additionalContext` → system prompt) | ✅ | #106 surface (`session_start` → `SessionStartOutcome`) + #121 config-hook execution: a `[[hooks.SessionStart]]` command/http handler's `additionalContext` reaches the prompt end-to-end |
| Handler types: `command` / `http` / `mcp_tool` / `prompt` / `agent` | 🟡 | **Down-ticked 2026-08-15.** Only `command` + `http` execute. `mcp`/`prompt`/`agent` are still v1 stubs that log *"config hook kind not yet executable at runtime; skipping"* and return `Allow` (`crates/caliban-agent-core/src/hooks_router.rs:344`, pinned by test `bridge_builds_command_and_skips_stub_kinds`). Upstream renamed the `mcp` handler to **`mcp_tool`** and added `statusMessage`, `once`, `asyncRewake`, `shell`, `allowedEnvVars`, and a `model` field — none adopted |
| Config hooks (`[[hooks.*]]`) execute at runtime | 🟡 | **Down-ticked 2026-08-15.** #121 composes config handlers into the agent chain, but `hooks_router.rs::event_supported` (line 250) admits **only `PreToolUse` / `PostToolUse` / `SessionStart`** — a `[[hooks.UserPromptSubmit]]`, `PreCompact`, `SessionEnd`, or subagent-event handler is warn-and-skipped (#185 H4). Of the in-process events above, everything except `ConfigChange`/`CwdChanged`/`Notification` fires; you just cannot attach a *config* handler to them. `disable_all_hooks` honored; `allow_managed_hooks_only` fires none until scope provenance lands (#124) |
| Hook event coverage vs upstream's 31 events | 🟡 | **New row.** Upstream documents 31 events. Caliban's taxonomy splits three ways. **(1) Declared and dispatched:** the core loop (see the rows above). **(2) Declared but never dispatched** — the surface exists, nothing fires it: **`Notification`** (`Hooks::notification` + `NotificationCtx`, `crates/caliban-agent-core/src/hooks.rs:493`; implemented by the headless sink at `caliban/src/headless/hooks_sink.rs:217`, but the only `.notification(` *call* sites are in `crates/caliban-agent-core/tests/hooks_events.rs`), plus `ConfigChange` and `CwdChanged`. **(3) Absent entirely** — no trait method at all: `Setup`, `UserPromptExpansion`, `StopFailure`, `PostToolUseFailure`, `PostToolBatch`, `TeammateIdle`, `InstructionsLoaded`, `WorktreeCreate`/`Remove`, `Elicitation`/`ElicitationResult`, and (new at v2.1.2xx) **`MessageDisplay`** / **`DirectoryAdded`**. Decision protocol also lacks the new `escalate` decision and `retry` field |
| Hook inheritance for subagents | 🟡 | **Contradiction resolved 2026-08-15** — the duplicate ✅ row in §G was wrong and has been deleted. `inherit_hooks: true` is the default (`crates/caliban-tools-builtin/src/agent/agent_tool.rs:90`) but propagates only the **permission** slice (`InheritableHookConfig` = rules/mode/audit/runtime_rules, `caliban/src/hook_inherit.rs`) to **background** sub-agents (`caliban/src/worker.rs:608`). **Foreground sub-agents get `NoopHooks`** — `install_sub_agent`'s factory never calls `.hooks()` (`caliban/src/startup/compose.rs:884-927`; its own doc comment at :854 says "deferred to v2"). Config `[[hooks.*]]` handlers and closure hooks never cross to any sub-agent |
| Plugin packages (bundle skills + hooks + agents + MCP + output-styles) | ✅ | ADR-0030; `caliban-plugins` orchestrator parses `plugin.json`, expands `${CALIBAN_PLUGIN_ROOT}` (+ `${CLAUDE_PLUGIN_ROOT}` alias), namespaces items, and feeds existing loaders. Marketplace install + trust gating + `caliban plugin {install,list,enable,disable,remove,info,update}` (`crates/caliban-plugins/src/cli.rs`); marketplace fetches routed through the SSRF-guarded client (#158) |

## C. Memory & checkpointing

| Capability | Caliban | Notes |
|---|---|---|
| Three-tier prompt prefix (global / project / auto) | ✅ | ADR-0018 |
| CLAUDE.md ancestor walk + nested-on-demand | ✅ | ADR-0036 |
| `@path/file` imports inside CLAUDE.md (recursion-bounded) | ✅ | ADR-0036. Note upstream **reduced its own max import depth from 5 to 4 hops** and now skips code spans/fences when parsing imports — worth matching on the next pass |
| Auto-memory (model-written notes per project) | ✅ | ADR-0035; routed through the gonzalo storage facade in 0.8.0 (#470/#484) |
| `claudeMdExcludes` for monorepos | ✅ | ADR-0036 |
| Auto-checkpoint per prompt + `/rewind` | ✅ | ADR-0028; crate `caliban-checkpoint`; `CheckpointHook` snapshots file-tool pre-images per prompt under `$XDG_DATA_HOME/caliban/projects/<cwd-hash>/checkpoints/<session>/prompt-NNN/` (ADR-0050). Durability hardened in 0.6.0 (transactional restore, eviction ordering, atomic index — #412/#444); byte-cap sweeper #180; symlink escape closed #448. Residual: restore is still unconfined against symlink TOCTOU (#497) |
| Esc-Esc / fork-from-checkpoint | ✅ | ADR-0028 — Esc-Esc on empty input opens the rewind overlay (`is_esc_chord` policy, 400 ms window). Fork-from-checkpoint stays 🔴 (sub-agent fleet spec) |
| MicroCompact (LLM-free per-tool supersession janitor) | ✅ | Plan B (`2026-05-26-context-management`); `MicroCompactor` replaces superseded `ToolResult` blocks (per-tool key: `Read`→file_path, `Grep`/`Glob`→exact args, `WebFetch`→url; `Bash` never supersedable) with `[superseded: <tool>(<key>)]`. Ordering bug fixed #170 |
| Tool-result size cap with overflow persistence | ✅ | Plan B; `ToolResultCap` (default 50 000 chars) writes overflow to **`$XDG_CACHE_HOME/caliban/tool-overflows/<session-id>/<tool-use-id>.txt`** (XDG-first per ADR-0050 — the old `~/Library/Caches/…` path in this row was stale), replaces inline content with `[truncated: N chars, full content at <path>]` + head/tail preview (`crates/caliban-agent-core/src/post_process.rs`). Window clamp fixed #182 |
| Checkpoint limitations vs upstream | 🟡 | **New row.** Upstream documents a 100-checkpoint snapshot cap, a "Never mind" option, guided summaries, rewind past a cleared conversation (v2.1.191+), and explicit non-restoration of subagent edits and sym/hard-linked paths. caliban's overlay has none of the menu affordances beyond restore; the limitation set is undocumented |

## D. Configuration / settings

| Capability | Caliban | Notes |
|---|---|---|
| Layered settings (managed / user / project / local) with merge semantics | ✅ | ADR-0026; crate `caliban-settings` loads JSON/TOML at four canonical scopes with documented per-key merge rules + `--settings` / `--setting-sources` CLI flags + `parent_settings_behavior: "block"` lockdown. Legacy per-feature TOMLs still load when the unified file is absent. TOML primary per ADR-0045; JSON accepted on read with WARN. `Settings.model` / `fallback_model` consumed at startup via `EffectiveModel::resolve`. Scope attribution fixed #463. Known drift: schema rejects a valid key / accepts a phantom key (#498) |
| `/config` interactive editor | ✅ | ADR-0026 (Phase 1); `/config` overlay surfaces the merged effective settings + scope chain (provenance per key). Tabbed write-back editor still deferred |
| Live reload (`ConfigChange` hook) | 🟡 | **Down-ticked 2026-08-15.** ADR-0026's `SettingsWatcher` (notify, 250 ms debounce, `crates/caliban-settings/src/watcher.rs:31`) is real and unit-tested — but it is **never constructed in the binary**. The only `SettingsWatcher::watch` call in the tree is inside its own `#[tokio::test]` (`watcher.rs:149`); `rg 'SettingsWatcher' caliban/src/` returns nothing. Settings are loaded once into `settings_snapshot` at startup, so **no key live-reloads today** and the `ConfigChange` hook never fires (see §B). The `model` / `output_style` restart-required diff logic exists but is unreachable |
| `apiKeyHelper` (dynamic auth refresh) | ✅ | ADR-0026; `ApiKeyHelperPool` invokes the helper without a shell, caches per `refreshIntervalMs` (default 5 min, `CALIBAN_API_KEY_HELPER_TTL_MS`), warns at `slowHelperWarningMs`. Wired into `startup::build_provider` and `router::build_one`; `RefreshingProvider<P>` invalidates and rebuilds on a 401/403, retrying once |
| Schema validation | ✅ | ADR-0026; embedded schema at `caliban-settings/src/schema.json` validated via `jsonschema` (Draft-7); invalid documents warn but don't abort |
| Settings-key surface vs upstream | 🟡 | **New row.** Upstream's `settings.json` grew from ~80 to ~140 top-level keys at v2.1.233 (autocompact, workflows, agent teams, cross-session, artifacts, theme, gateway/federation auth, managed version gating, `sandbox.credentials`, …). caliban implements the Claude-Code-compatible core; the long tail is unimplemented and mostly out of scope until the corresponding features exist |

## E. TUI ergonomics

| Capability | Caliban | Notes |
|---|---|---|
| Status bar, plan-mode chip, spinner, elapsed | ✅ | |
| Mouse-wheel scroll, transcript | ✅ | |
| `@file` mention + autocomplete | ✅ | ADR-0027; gitignore-aware via `ignore` crate; submit-time attach with size cap |
| `!` shell escape | ✅ | ADR-0027; routes through `Bash` tool + `PermissionsHook`. Upstream now auto-responds to shell output (`respondToBashCommands`) and adds path autocomplete — not adopted |
| External editor (`Ctrl+G` → `$VISUAL` / `$EDITOR`) | ✅ | ADR-0027; alt-screen suspend/resume |
| Vim editing mode | 🔴 | `editor_mode` parses and is *displayed* in `/config` (`crates/caliban-settings/src/settings.rs:351`), but **nothing consumes it** — `caliban/src/tui/input.rs` and `events.rs` have no modal state and no vim keymap. Upstream also added `vimInsertModeRemaps` (v2.1.208+) |
| `Ctrl+O` transcript viewer + dump-to-scrollback | ✅ | ADR-0027; `q`/Esc close, `[` dump, `v` open-in-$VISUAL, scroll keys, `?` help. Upstream made these keys rebindable — not adopted |
| Background bash | ✅ | `Bash{background:true}` → `spawn_background` (`crates/caliban-tools-builtin/src/shell/bash.rs:190`) + `BashOutput` + `KillShell`. **Note corrected 2026-08-15:** the TUI's `Ctrl+B` is *not* Claude Code's "background this running Bash" — it hands the in-flight foreground **sub-agent** to the supervisor and cancels the parent turn (ADR-0037, `caliban/src/tui/events.rs:691`). The tool-level capability is genuine; the chord is a different feature |
| Image / vision input | 🟡 | **Down-ticked 2026-08-15.** ADR-0039 machinery is all present (`caliban-images` ingest/blob/routing, per-adapter wire shapes, capability filter + strict-routing fallback) and `caliban/src/tui/attach.rs:218::resolve_image_attachments` builds `ImageBlock`s — but **nothing calls it**. The TUI send path pushes `Message::user_text` only (`caliban/src/tui/events.rs:940`), there is no `--image` flag, and `caliban_images` has no consumer outside `attach.rs`. No user-reachable way to send an image at v0.8.0 |
| Slash-menu typeahead | ✅ | #15 — fuzzy subsequence matching (case-insensitive) with start/word-boundary + contiguity ranking; `cfg`→`/config` |
| Permission Ask modal | ✅ | ADR-0027 + Plan C: 4-button modal — `y` / `A` / `n` / `R` / Esc. "Always" branches append session-scoped `RuntimeRule` via `RuntimeRuleStore`. Pattern derived per-tool with `caliban_agent_core::derive_pattern`. Double-prompt fixed #58; live rules #55 |
| Reverse history search (`Ctrl+R` / `Ctrl+S`) | ✅ | ADR-0027; `caliban/src/tui/reverse_history.rs`; session → project → all-projects scopes; persisted per project |
| Multi-line input (`\`+Enter, Option+Enter, Shift+Enter native) | ✅ | #101 — all three chords wired: native Shift+Enter (kitty keyboard-enhancement flags), Alt/Option+Enter fallback, trailing-`\`+Enter continuation |
| Voice dictation | 🔴 | `/voice` is a hidden stub returning "voice dictation not available in this build" (`caliban/src/tui/slash/dx.rs:158`); no audio capture anywhere in the workspace |
| Clipboard image paste (`Ctrl+V` → `[Image #N]`) | 🔴 | **New row (upstream v2.1.2xx).** `caliban-images` has a clipboard ingest path but no key binding and no send-path consumer — see "Image / vision input" |
| Fullscreen renderer + `/theme` colors + emoji shortcodes | 🔴 | **New row (upstream).** Upstream made fullscreen a first-class renderer (search dialog, `Ctrl+L`×2 → `/clear`), added `theme` with 7 built-ins incl. daltonized/ANSI variants, `:`-triggered emoji completion, `Ctrl+Z` suspend, `Ctrl+S` prompt stash, and `Ctrl+_` undo. caliban has none of these |

## F. Built-in tools

| Capability | Caliban | Notes |
|---|---|---|
| Bash, Edit, Glob, Grep, Read, Write, WebFetch, TodoWrite, Skill, AgentTool, EnterPlanMode/ExitPlanMode | ✅ | All registered in `caliban/src/startup/compose.rs:592-616` (+ `SkillTool` :654, `AgentTool` :1012). Verified 2026-08-15 — no stubs |
| WebSearch | ✅ | `crates/caliban-tools-builtin/src/web/web_search.rs` — Brave/Tavily/Exa via `BRAVE_API_KEY`/`TAVILY_API_KEY`/`EXA_API_KEY` |
| NotebookEdit (Jupyter) | ✅ | `crates/caliban-tools-builtin/src/fs/notebook_edit.rs`; nbformat v4; atomic write; FileChanged |
| MultiEdit semantics (atomic multi-replace) | ✅ | `crates/caliban-tools-builtin/src/fs/multi_edit.rs`; sequential + rollback-on-miss. Open design question on the whitespace-fuzzy fallback contract (#418) |
| PowerShell tool | 🔴 | No PowerShell tool, no `defaultShell` selector; `Bash` is the only shell tool. Low priority |
| `ToolSearch` (lazy MCP schema loading) | 🟡 | ADR-0046 v1 machinery all present and wired — `ToolSearch` registered from `install_tool_search` (`compose.rs:835`), per-server `lazy = false`, LRU activation cap (24), `inherit_active_mcp`, `/context` active set. **But `tools.lazy_mcp` is still default-`false` at v0.8.0** (`crates/caliban-agent-core/src/agent.rs:160`) — the promised v1.1 flip has not landed, so the context saving is off out of the box. Matching is plain substring over MCP tool name/description (no fuzzy, no `+term`); `select:a,b` works. Upstream meanwhile moved `ENABLE_TOOL_SEARCH` to `true|false|auto|auto:N` |
| `WaitForMcpServers` | 🔴 | **Down-ticked 2026-08-15.** Zero Rust hits anywhere in the tree — the tool does not exist. The prior 🟡 was awarded for "ADR-0046 covers the design space", which is design coverage, not implementation |
| `EndConversation` | 🔴 | **New row (upstream).** Referenced by the skills page's `disallowed-tools` rule; caliban has no analogue |

## G. Sub-agents

| Capability | Caliban | Notes |
|---|---|---|
| In-process synchronous `AgentTool` + recursion guard | ✅ | ADR-0021; no-edit nudge resets on real edits (#244); prompt truncated on a char boundary (#219) |
| Subagent in isolated git worktree | ✅ | ADR-0037 — `caliban-worktrees` crate; `isolation: worktree` frontmatter; per-source isolation wired by ADR-0052 |
| Background subagents (`--bg`, `caliban agents`, attach/respawn/rm) | ✅ | ADR-0037 — `caliban-supervisor` + `caliband` daemon + CLI. Signal races closed #115/#138; `agents logs` reads the worker transcript #143 |
| Interactive background sub-agents (idle / await-input) | ✅ | **New row (ADR-0047, #81).** `InputProvider` mode idles awaiting input and resumes interactively, backed by a bidirectional per-agent socket (`SocketInputProvider`), a worker→daemon status channel with `AgentStatus::Idle`, and a `--interactive` spawn path. `caliban agents attach` streams a live transcript with a send path (#79) |
| `caliband` network transport (NDJSON over TCP + TLS + bearer token) | ✅ | **New row (ADR-0051, #280/#321).** Remote clients (e.g. prospero) can drive the daemon across the network rather than only over a Unix socket. Fail-closed token + TLS (#288/#395). Hardening tracked in #319/#320; gRPC migration deferred to #314. Worker status over the TLS control plane fixed in 0.8.0 (#510/#512) |
| Workspace-scoped `caliband` (multi-source supervision) | ✅ | **New row (ADR-0052, #281/#325).** The supervisor manages a workspace spanning multiple sources with per-source worktree isolation wired end to end. Follow-ups in #324 |
| Subagent-local memory dir | ✅ | ADR-0037 — `<base>/agents/<id>/` per-agent session dir |
| Subagent fleet supervisor daemon | ✅ | ADR-0037 — per-repo `caliband` over UDS; multi-arch GHCR image (#279/#298) + `caliban-operator` + Helm charts |
| Subagent frontmatter surface vs upstream | 🟡 | **New row.** caliban supports the core set (`tools`, `model`, `isolation`, `inherit_hooks`, `inherit_active_mcp`, …). Upstream adds `disallowedTools`, `permissionMode` (incl. `auto`/`dontAsk`/`manual`), `maxTurns`, `skills`, `mcpServers`, `hooks`, `memory` (user/project/local), `effort` (incl. `xhigh`), `color`, `initialPrompt` — plus new built-ins `claude` and **`fork`**, `/subtask`, `/tasks`, `@agent-<name>` mentions, `--append-subagent-system-prompt`, and `CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH`/`_MAX_CONCURRENT_SUBAGENTS`. Note upstream **removed** its `/agents` interactive editor in v2.1.198 |

## H. MCP

| Capability | Caliban | Notes |
|---|---|---|
| Config + name validation (caliban-mcp-client v1) | ✅ | ADR-0017 |
| Real spawn / handshake / `list_tools` (rmcp 1.7) | ✅ | ADR-0023 Phase A; version pinned by ADR-0044 |
| HTTP / SSE transports | ✅ | ADR-0023 Phase B (rmcp streamable-http client; legacy SSE folded in) |
| Per-server permission scoping | ✅ | ADR-0023 Phase B (`[server.X.permissions]` composes with global rules); `deny:mcp__*` outranks server allows (#213) |
| `/mcp` slash + per-server enable/auth | ✅ | ADR-0023 Phase C — glyphs (●/◐/○), `d/r/a/s/t` key hints |
| OAuth flow + callback port | ✅ | ADR-0023 Phase C — PKCE + loopback, RFC 8414 discovery, manual config, keyring + file-store fallback. Hardened: https + issuer match enforced (#339), atomic `0600` token store (#341), cache-before-discovery + DCR `client_secret` persistence (#333), flow wired into the connection path (#300). Residual: manual-mode token/auth endpoints bypass the https guard (#496) |
| OAuth Dynamic Client Registration (RFC 7591) | ✅ | **New row (#313/#315).** `oauth = "auto"` servers self-register a client when the provider supports DCR — ahead of Claude Code's documented surface, which only supports pre-configured credentials |
| Elicitation (server-initiated input) | ✅ | ADR-0023 Phase C — `ElicitationBridge` mpsc → TUI, 5-min cap, auto-decline in `--print` |
| `${CLAUDE_PROJECT_DIR}` expansion | ✅ | Phase B `config::expand_value` (we use `mcp.toml`, not `.mcp.json`); settings-path MCP env expansion completed #309/#311 |
| `MCP_TIMEOUT` / `MCP_TOOL_TIMEOUT` / `MAX_MCP_OUTPUT_TOKENS` envs | ✅ | ADR-0023 — `CALIBAN_MCP_TIMEOUT`/`CALIBAN_MCP_TOOL_TIMEOUT` primary, `MCP_*` honoured for parity |
| Resources (`@server:resource` references) | ✅ | ADR-0023 Phase C — `McpResource` cache, `list_changed` invalidation, URI-template positional expansion |
| Code-graph MCP consumption (gonzalo) | ✅ | **New row (#308/#310).** `search`/`node`/`callers`/`callees`/`impact`/`explore` wired over stdio or HTTP; hermetic contract test via an in-tree mock server (#344) |
| WebSocket transport (`type: "ws"`) | 🔴 | **New row (upstream).** New third transport with `alwaysLoad`/`headersHelper`/`timeout`; caliban has stdio + HTTP/SSE only |
| `headersHelper` + `claude mcp login`/`logout` | 🔴 | **New row (upstream v2.1.186+).** Dynamic per-server auth headers with 10 s timeout and 401/403 re-run, plus CLI-driven OAuth login/logout. caliban has neither |
| MCP tool-call auto-backgrounding + idle timeout + discovery cache | 🔴 | **New row (upstream v2.1.187–221).** Calls > 2 min move to a background task; 5 min/30 min idle timeouts; remote tool-list discovery cache. caliban has none |

## I. Model router & providers

| Capability | Caliban | Notes |
|---|---|---|
| Purpose-keyed routing | ✅ | ADR-0022 |
| Fallback chain, hedging, circuit breakers | ✅ | ADR-0038; `caliban-model-router` v2 (`fallback.rs`, `hedging.rs`, `breaker.rs`); breaker recovery state machine fixed #215/#183 |
| Capability-based filtering (vision / thinking / tool_use) | ✅ | ADR-0038; `capabilities.rs`; `tool_use` parsed as a string enum #172 |
| `caliban.toml` binary wiring | ✅ | ADR-0038; `discovery.rs` walk-up + binary `router::try_load` |
| Anthropic / OpenAI / Ollama / Google providers | ✅ | Ollama gained dynamic model discovery + real context-window detection (#316/#60) |
| Bedrock | ✅ | ADR-0034; `caliban-provider-bedrock` |
| Vertex | ✅ | ADR-0034; `caliban-provider-vertex` |
| Foundry | 🔴 | No `caliban-provider-foundry` crate, no `ProviderKind::Foundry` (`caliban/src/args.rs:20`), no `router::build_one` arm (`caliban/src/router.rs:90`), no `rates.yaml` entry. Tracked in #30 |
| Effort levels | 🟡 | **Down-ticked 2026-08-15 on upstream drift.** caliban ships `low`/`medium`/`high`/`max`/`auto` (`caliban/src/tui/slash/model.rs:116`; `EffortLevel` in `crates/caliban-model-router/src/config.rs:17` is Low/Medium/High). Upstream added **`xhigh`** and **`ultracode`** at v2.1.203+, and threads `effort` through hooks, subagent frontmatter, and OTel attributes |
| Extended-thinking toggle wiring | ✅ | #100; `ThinkingSetting{Auto,Off,On(budget)}` on every live request, decoupled from `Effort`. Runtime control via `/think on\|off\|auto\|<budget>`. Honored by the Anthropic (`thinking`) and OpenAI (`reasoning`) converters. Per-turn thinking cap added #62 |
| Advisor model (`--advisor` / `advisorModel`) | 🔴 | **New row (upstream).** Server-side advisor tool with its own model selection. caliban has no analogue; adjacent multi-model ideas are tracked in #264/#266/#267/#268 |

## J. Headless / CI

| Capability | Caliban | Notes |
|---|---|---|
| `-p` / `--print` mode | ✅ | ADR-0025; `caliban/src/headless/`, dispatched via `run_headless` |
| `--output-format text` / `json` / `stream-json` | ✅ | ADR-0025; NDJSON frames with `system/init`, `message`, `tool_use`, `tool_result`, `text`, `hook_event`, `result`. Text mode surfaces non-success stops (#175) |
| `--input-format text` / `stream-json` | ✅ | ADR-0025; `parse_stream_json_payload` handles `user` and `control/interrupt`; 10 MiB stdin cap; `stream-json` input activates headless mode (#218) |
| Result-frame enrichment (Claude Code contract) | ✅ | **New row (ADR-0049, #222/#276).** Result frame carries the final message plus the additive Claude-Code-contract fields; kept terminal (#218); `duration_ms` is whole-session in multi-frame `stream-json` (#331). Per-user-frame result semantics tracked in #332 |
| `--max-turns`, `--max-budget-usd` | ✅ | **Note corrected 2026-08-15 — the placeholder-cost caveat was stale.** `--max-budget-usd` now prices each turn against the vendored `rates.yaml` via `BudgetTracker::record_with_model` → `CostAccumulator` (`caliban/src/headless/budget.rs`, production call site `caliban/src/headless/mod.rs:714`), aborting when strictly exceeded; `is_placeholder()` hard-returns `false`. Unknown (provider, model) pairs still price at $0 with a debounced WARN |
| `--bare` (skip discovery; default in CI) | ✅ | Opt-in per ADR-0025; gates hooks/skills/MCP/auto-memory/CLAUDE.md loaders |
| `--json-schema` + structured output | 🟡 | **Down-ticked 2026-08-15.** Still best-effort: a prompt-injected schema directive plus a shallow local check of top-level `type`, top-level `required`, and one-level `properties.*.type` (`caliban/src/headless/schema.rs`). No `$ref`/nested/`enum`/array validation, and **no provider-side structured-output wire mode** — `response_format`/`json_schema` appear nowhere in the provider crates. The old "lands with ADR 0032" pointer was wrong (0032 is the OS sandbox); native structured output is unspecced. Applied reliably even with a system message (#214/#174) |
| `--include-partial-messages` / `--include-hook-events` | ✅ | Partial-messages emit `text` delta frames; hook events flow through the outer `CompositeHooks` layer (`HeadlessHookSink`) |
| `--verbose` / observability flags | ✅ | **New row (#27).** Full headless tool I/O for observability; opt-in tool-dispatch timing on `tool_result` frames (`t_ms`, #28/#398); bounded `result_text` on the agent stream (#391) |
| GitHub Actions workflow | 🟡 | **Up-ticked 2026-08-15.** The docs half shipped — `docs/guide/src/automation/ci.md` is a full CI-patterns page with four copyable GitHub Actions recipes, an exit-code table, and `jq` parsing of `stream-json` — and the multi-arch GHCR image (#279/#298/#302) makes `container:`-based jobs work today. Still **no `action.yml` / published reusable Action**, and no `caliban-action` repo in the org. Tracked in #39 |
| Devcontainer feature | 🔴 | No `.devcontainer/`, no `devcontainer-feature.json`, no published feature. The root `Dockerfile` is the runtime GHCR image, not a devcontainer feature. Tracked in #40 |
| `caliban doctor` from shell | ✅ | **Up-ticked 2026-08-15 — this row was simply stale.** `CalibanCommand::Doctor { deep }` (`caliban/src/args.rs:732`) is dispatched in `caliban/src/main.rs:99` *before* TUI/provider startup, so it works when the CLI otherwise won't start. Runs 11 real checks via the shared `caliban/src/diagnostics.rs` runner; exits 1 on any Fail. Same runner backs the TUI `/doctor` |
| Subagent text forwarding + richer `system/init` | 🔴 | **New row (upstream v2.1.205–219).** `--forward-subagent-text` with `parent_tool_use_id` correlation; `system/init` gains `capabilities`, `mcp_servers`, `mcp_server_errors`; `hook_started`/`hook_progress`/`hook_response` events. caliban has none |

## K. Observability / cost

| Capability | Caliban | Notes |
|---|---|---|
| `tracing` instrumentation under `caliban::*` targets | ✅ | |
| `--debug` + `--debug-file` | 🟡 | **Notes filled 2026-08-15 (cell was empty).** Both flags ship and work: `caliban/src/args.rs:433`/`:439`; `compose.rs::resolve_debug_log_path` → `$XDG_CACHE_HOME/caliban/debug.log` (ADR-0050); `--debug-file` implies `--debug`; `CALIBAN_DEBUG`/`CALIBAN_DEBUG_FILE` honored; `default_debug_filter` silences `ignore`/`globset` spam. **Gaps:** no `CLAUDE_CODE_DEBUG_LOGS_DIR` analogue — a single append-only file with no directory mode, no per-session naming, and no rotation, so concurrent processes interleave — and `--debug` is a bare bool, not upstream's category filter (`--debug mcp,!1p`); `RUST_LOG` is the only selector |
| `/context` slash | ✅ | ADR-0033; per-message-kind breakdown + 80 % warning; surfaces the lazy-MCP active set |
| `/usage` slash + per-session token + $ | ✅ | ADR-0033; per-model breakdown + cache savings (cache tokens reach totals since #423) |
| `/compact` slash + manual trigger | ✅ | ADR-0033; a **real** compactor was only wired in #292/#294 (2026-07-03) — this row claimed ✅ before that landed. Strategy correctness fixed #329; oversized `tool_use`/`thinking` shrunk on compaction #449 |
| Proactive autocompact (threshold-based + 2-strike backoff) | ✅ | Plan B; fires at `estimate_tokens(history) / max_input_tokens >= auto_compact_threshold` (default 0.75); 2 consecutive failures disable autocompact for the run |
| Conversation-level prompt cache marker | ✅ | Plan B; `apply_prompt_cache` marks the last user message with `cache_control: Ephemeral` at ≥ `min_cache_block_tokens` (default 1024). Caveat: prompt-cache tokens double-counted in `gen_ai.usage.input_tokens` (#493) |
| Cost ($) tracking | ✅ | ADR-0033; `rust_decimal` against the vendored, date-aware `crates/caliban-telemetry/rates.yaml` (6 providers), wired on both paths — TUI `App.cost_accumulator` (`caliban/src/tui/events.rs:185` → `/cost`, `/usage`, status bar) and headless `BudgetTracker`. Caveat: real cost never reaches the OTel metrics pipeline (see "Metric set" / #467) |
| OpenTelemetry export (OTLP metrics / logs / traces) | 🟡 | **Down-ticked 2026-08-15.** Traces ✅ (real `TracerProvider`, #375/#383) and metrics ✅ (real `SdkMeterProvider` + `PeriodicReader` honoring `OTEL_METRIC_EXPORT_INTERVAL`, force-flushed at shutdown, #427/#468); the shipped binary compiles in `otlp`+`otlp-http`+`otlp-grpc`. **Logs are not exported at all** — no `LoggerProvider` or log bridge exists in `caliban-telemetry` despite the `opentelemetry-otlp/logs` feature being enabled. Also: `enable_telemetry` in settings is inert — `caliban/src/main.rs:232` only logs an info line; the env var is the sole switch (#494). Documented knobs parsed but never honored (#499) |
| `gen_ai` chat-generation span per model request | ✅ | **New row (ADR-0053, #378/#384).** `crates/caliban-agent-core/src/stream/mod.rs:1532` opens the span with `gen_ai.operation.name="chat"`, `gen_ai.provider.name`, `gen_ai.request.model`, backfilling `response.model`/`finish_reasons`/`usage.*_tokens`. Nests under the run span (#385). Test `tests/genai_span.rs` |
| `execute_tool` span per tool call (`gen_ai.tool.*`) | ✅ | **New row (ADR-0053, #386).** `crates/caliban-agent-core/src/stream/hook_dispatch.rs:43` — `gen_ai.operation.name="execute_tool"`, `gen_ai.tool.name`, `gen_ai.tool.call.id`; INTERNAL span kind per semconv. Test `tests/execute_tool_span.rs` |
| Prompt / completion capture on spans (`OTEL_LOG_USER_PROMPTS`) | ✅ | **New row (ADR-0053, #380/#387).** `gen_ai.input.messages` / `gen_ai.output.messages` recorded only when the operator opts in; **off by default**. Bounded per #428 (per-part char cap + total serialized-byte cap). Tests `genai_content_{on,off}.rs` |
| OTLP exporter transport auth (headers vs mTLS) | 🟡 | **New row.** Headers ✅ — gRPC auth headers via tonic metadata plus a headers-helper applied at startup (`crates/caliban-telemetry/src/headers.rs`, #426/#466). mTLS 🔴 — `init.rs:392` *parses* `OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE`/`_CLIENT_KEY`/`_CERTIFICATE` into struct fields that **no exporter builder ever consumes**. Tracked in #465 |
| Metric set (`session.count`, `token.usage`, `cost.usage`, …) | 🟡 | **Down-ticked 2026-08-15.** Typed emitters exist for all six and the OTLP instruments are live, but **only `caliban.session.count` is emitted from production code** (`init.rs:571`/`:701`). `cost.usage`, `token.usage`, `lines_of_code.count`, `code_edit_tool.decision`, `active_time.total` are exercised **only in unit tests**; the four `RECOVERY_*` names are bare `&str` constants with no emitter. Root cause is architectural: the TUI and headless each hold their own `CostAccumulator`, neither of which is `Telemetry.cost`, and neither touches `MetricEmitter`. **Also:** the prefix is `caliban.*`, not `claude_code.*` (deliberate, ADR-0053), and `pull_request.count`/`commit.count` have no analogue. Tracked in #467 |
| Turn-loop resilience (MaxTokens 2-stage recovery, stream-idle watchdog, refusal surfacing, reactive compact, TurnDecision) | ✅ | Plan A; recovery behavior is real. Note the `RECOVERY_*` counter names are *published strings only* — no counter is incremented (see "Metric set") |
| `/doctor`, `/heapdump` diagnostics | 🟡 | `/doctor` is real — 11 checks (settings, sandbox, checkpoint + session stores, skills, CLAUDE.md, workspace, plus 4 provider probes), `--deep` adds auth pings, shared with `caliban doctor`. `/heapdump` is an unconditional stub (`caliban/src/tui/slash/dx.rs:39`) whose own advice is **unfollowable**: there is no `jemalloc-prof` feature in `caliban/Cargo.toml` and no jemalloc dependency vendored. Tracked in #24. Upstream's `/heapdump` now also emits a `-diagnostics.json` sidecar |
| Status line (custom script) | ✅ | `StatuslineRunner` in `crates/caliban-settings/src/statusline.rs:68`, configured via the `statusLine` key (Claude Code spelling, `status_line` alias), constructed in `caliban/src/tui/app.rs:477` from the live settings handle and prefixed onto the status bar; refreshed off-thread after each `TurnEnd`/`RunEnd` so it never runs in the render path |
| `feedbackSurveyRate` + `/feedback` | 🔴 | `/feedback` is a stub (`caliban/src/tui/slash/dx.rs:61`) pointing at a `feedback_url` setting that **does not exist** in `caliban-settings`; `feedback_survey_rate` lives only in the settings-hierarchy spec. Tracked in #25 |
| `/insights` usage report + `/usage` attribution | 🔴 | **New row (upstream).** `/insights` renders an HTML usage report; `/usage` gained per-skill / per-subagent / per-plugin / per-MCP-server attribution and behavior flags. caliban's `/usage` is per-model only |

## L. Output styles

| Capability | Caliban | Notes |
|---|---|---|
| Default / Proactive / Explanatory / Learning | ✅ | ADR-0031; four built-ins ship in `caliban-output-styles`. Selection is via `CALIBAN_OUTPUT_STYLE`; a `Settings.output_style` key now exists (`crates/caliban-settings/src/settings.rs:349`) [^l-force] |
| Custom output-style files (frontmatter + body) | ✅ | ADR-0031; project (`<ws>/.caliban/output-styles/`) > user (XDG) > plugin > built-in [^l-force] |

[^l-force]: **Footnote corrected 2026-08-15.** `force_for_plugin: true` is parsed
and `select_active` implements it (`crates/caliban-output-styles/src/loader.rs:308`),
and plugin packaging (ADR-0030) *has* now shipped — but the flag is still inert
for a different reason: `enabled_plugins` is hard-coded to an empty vector at
`caliban/src/startup/compose.rs:1680` ("v2: enabled_plugins is empty until ADR
0030 ships the plugin system"), so no plugin style can ever win the override.
Upstream separately **removed** its `/output-style` command in v2.1.91 and now
loads nested project style dirs between cwd and repo root.

## M. Slash command coverage

> **Honesty pass 2026-08-15.** The registry
> (`caliban/src/tui/slash.rs::SlashCommandRegistry`, ADR-0040) has **40
> registered names** (36 visible + 4 hidden: `/exit`, `/plugin`, `/voice`,
> `/system`). Of those 40, **ten are pure stubs** (`/agents`, `/login`,
> `/logout`, `/status`, `/setup-token`, `/heapdump`, `/feedback`, `/tui`,
> `/loop`, `/voice`) and **three are partial** (`/output-style`,
> `/statusline`, `/resume`); the other 27 are real. The previous version of
> this section bundled stubs into ✅ rows whose own notes admitted they were
> stubs; those rows are now split so each command carries its true status.
> **All 40 registered names now have a row.** The 🔴 rows at the bottom of the
> table are commands that are *not registered at all* — absent, not stubbed.

| Command | Caliban | Notes |
|---|---|---|
| `/plan`, `/memory`, `/skills`, `/quit` (+ hidden `/exit`) | ✅ | Ported to the `SlashCommand` trait (ADR-0040). `/memory delete` gated behind `--force` (#112) |
| `/clear`, `/help`, `/init` | ✅ | ADR-0040; `/init` writes `CLAUDE.draft.md` from `AGENTS.md` / `.cursorrules` / `.windsurfrules` / `README.md` / `git status`. Upstream's `/init` now reads Copilot rules by default and gates `AGENTS.md` behind `CLAUDE_CODE_NEW_INIT=1` |
| `/context`, `/usage`, `/compact`, `/cost`, `/export` | ✅ | ADR-0033 logic surfaced through the registry (ADR-0040); `/cost` prints cumulative + per-(provider,model) USD; `/export [path] [--format json]` writes the session transcript |
| `/config`, `/hooks`, `/mcp`, `/model`, `/effort`, `/think`, `/permissions` | ✅ | ADR-0040. `/model <id>` runtime-swaps via `Agent::try_swap_model` (same-provider); `/effort` and `/think` write `ArcSwap` state consumed on the next turn; `/permissions` is a real overlay (Tab cycles mode, `d` deletes a rule). **`/permissions` and `/think` had no M row before this refresh** |
| `/plugin`, `/plugins` | ✅ | ADR-0030; drives `caliban_plugins::Cli::list` + `render_overlay` with enable/disable status |
| `/rewind` | ✅ | ADR-0028; overlay lists per-prompt checkpoints (newest first); Esc-Esc opens the same overlay |
| `/recap`, `/btw` | ✅ | ADR-0040. Upstream's `/btw` has since become a threaded overlay with fork/copy/clear — not adopted |
| `/doctor` | ✅ | **Split out of the old `/doctor, /heapdump, /feedback` row.** Real: `crate::diagnostics::Diagnostics::run`, `--deep` supported |
| `/system` (hidden) | ✅ | **Previously unrowed** — the last of the 40 registered names to get a row. Real: opens the active-system-prompt overlay (`caliban/src/tui/slash/observe.rs:390` → `Overlay::System` → `overlay.rs:238::system_lines`). Marked `hidden: true`, "present for backwards compat; not in spec" — no Claude Code analogue |
| `/output-style` | 🟡 | **Previously unrowed.** Lists the registry and the active style, but selection is still env-var only (`CALIBAN_OUTPUT_STYLE`); no picker |
| `/statusline` | 🟡 | **Split from the old `/statusline, /tui` row.** Reports the live `statusLine` command/timeout/padding; no editor |
| `/resume` | 🟡 | **Down-ticked.** Real listing + name-substring filter, but the in-place picker overlay is still deferred (`caliban/src/tui/slash/session.rs:180`) pending `Overlay` support for non-`Copy` variants |
| `/agents` | 🔴 | **Down-ticked from ✅.** Pure stub — prints "full sub-agent fleet overlay arrives with the Sub-agent isolation spec… use `caliban agents list`" (`caliban/src/tui/slash/config.rs:184`). The CLI exists; the TUI does not |
| `/login`, `/logout`, `/status`, `/setup-token` | 🔴 | **Down-ticked from ✅.** All four are stubs deferring to an "Auth spec" that **does not exist** in `docs/superpowers/specs/`. `/status` prints only `provider.name()`. `/setup-token` was previously unrowed. Largest stub cluster in the matrix |
| `/heapdump`, `/feedback` | 🔴 | **Split out of the old ✅ row** and now consistent with §K. Both are pure stubs; `/heapdump` names a build feature that does not exist |
| `/tui` | 🔴 | **Split from the old `/statusline, /tui` row.** Pure stub — "alternate-screen toggle arrives with the TUI ergonomics spec" |
| `/loop` | 🔴 | **Down-ticked from ✅.** Parses `--n`/`--interval` and prints a plan, then defers: "execution lands with the polling scheduler spec" (`caliban/src/tui/slash/dx.rs:100`). **It never loops** |
| `/voice` | 🔴 | Hidden stub — see §E "Voice dictation" |
| Immediate (mid-turn) slash dispatch | ✅ | **Previously unrowed (#13/#78).** `SlashCommandMeta.immediate` / `is_immediate_slash` (`caliban/src/tui/slash.rs:269`) — 33 commands execute immediately instead of requiring a confirmation round-trip |
| `/theme` | 🔴 | No `/theme` command and no TUI color system; the only "theme" reference is a doc comment calling the TUI settings table opaque |
| `/code-review`, `/security-review`, `/review`, `/ultrareview` | 🔴 | Zero matches in the repo, as commands or as skills. Depends on the Skills polish sub-project |
| `/run`, `/verify`, `/debug`, `/batch` | 🔴 | The bundled-skills mechanism exists (`crates/caliban-skills/src/builtins.rs`, `include_str!`-embedded) but ships **exactly one** skill — `auto-memory`. Upstream ships **nine**: the original eight (`/code-review` aka `/review`, `/batch`, `/debug`, `/loop`, `/claude-api`, `/run`, `/verify`, `/run-skill-generator`) plus `/doctor`, which moved from built-in command to bundled skill in v2.1.205 |
| `/subtask`, `/tasks`, `/insights`, `/import`, `/schedule`, `/reload-plugins`, `/rename`, `/copy`, `/fast` | 🔴 | **New row (upstream).** Commands added upstream since 2026-05-24 with no caliban analogue. Upstream also **removed** `/output-style` (v2.1.91) and `/fork` (superseded by `/subtask`) |

## N. Long-tail surfaces (cloud / IDE / mobile)

All 🔴, all large investments. Tracking here only so we remember they exist:
IDE extension (VS Code / Cursor / JetBrains), GitHub App, claude.ai/code web,
iOS **and Android** app, Slack integration, Remote Control, Channels (research
preview), Routines (scheduled remote agents), Deep links, Teleport.

**Added upstream since 2026-05-24** (also all 🔴): **Cowork**, **Agent teams**
(`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS`, `teammateMode`, `TeammateIdle`),
**cross-session messaging** (`@`-mention live sessions on the machine),
**dynamic workflows**, **Claude apps gateway** (`claude gateway`) and
**self-hosted environments** (`claude self-hosted-runner`, `--environment`),
**Agent View** as a documented research-preview surface, **Chrome /
computer-use**, **GitHub Code Review**, and the **Managed Agents** hosted API.

> **Footnote — Remote Control adjacency (do not read as parity).** caliban has
> real remote-drive machinery: `caliban attach` / `agents attach` over the
> local supervisor, a `caliband` NDJSON-over-TCP+TLS control plane with a
> bearer token (ADR-0051), workspace-scoped multi-source supervision
> (ADR-0052), interactive await-input sub-agents (ADR-0047), a multi-arch
> GHCR image with a Kubernetes operator and Helm charts, and `prospero`'s
> embedded fleet dashboard above many `caliband`s. That is *an operator
> driving a headless daemon on their own network* — a different product from
> Claude Code's Remote Control, which drives your **local interactive
> session** from a phone or the web through a hosted relay. The rows stay 🔴.
> Nothing scheduled exists at all, so **Routines is unambiguously 🔴** — the
> only gesture toward it, `/loop`, is a stub that prints a plan and returns.

---

## Tier ordering (refresh when shipping)

**Rewritten 2026-08-15.** Every item in the old Tiers 1–4 has **shipped** in
the sense that its ADR is accepted and its crate exists — but "shipped" is not
"✅", and an earlier draft of this section claimed the stronger thing ("every
numbered item is ✅"), which is exactly the optimistic drift this refresh
exists to remove. Audited against the body of this matrix, **6 of the 12
numbered items are ✅ and 6 are not**:

| Old tier item | Status now | Why |
|---|---|---|
| T1 #1 Hook event surface (B) | 🟡 | Four §B rows are 🟡 — handler types, config-hook events, `ConfigChange`/`CwdChanged`, subagent inheritance |
| T1 #2 Settings hierarchy + `/config` (D) | 🟡 | Loading and `/config` are ✅, but live reload is 🟡 — `SettingsWatcher` is never constructed in the binary |
| T1 #3 Headless `-p` + JSON (J) | ✅ | `--json-schema` is 🟡, but that is structured output, not the headless protocol |
| T2 #4 TUI ergonomics (E) | 🟡 | Image/vision input is 🟡 (no caller); vim mode and voice are 🔴 |
| T2 #5 Slash coverage (K, M) | 🟡 | Ten registered stubs + three partials in §M |
| T2 #6 Checkpointing + `/rewind` (C) | ✅ | |
| T3 #7 Real MCP wiring (H) | ✅ | |
| T3 #8 Permission modes + auto-mode (A) | ✅ | |
| T3 #9 Plugin system (B) | ✅ | Loader is real; `enabled_plugins` being hardcoded empty is a §L wiring bug, not a plugin-system gap |
| T4 #10 OS sandbox (A) | 🟡 | Backends real, but one settings key and no per-hostname egress allowlist |
| T4 #11 OTel + cost (K) | 🟡 | No logs pipeline; 1 of 6 metrics emits |
| T4 #12 Bedrock + Vertex (I) | ✅ | |

Tier 5 is likewise mostly-but-not-entirely done: auto-memory, NotebookEdit,
WebSearch, background fleet, status line and output styles are ✅; vim mode,
voice, and the GitHub Action / devcontainer packaging are not. **The honest
summary is that the old tiers got caliban to feature-complete-on-paper, and the
remaining work is finishing the last mile of things already built.** That is a
different shape of work, so the tiers below replace the old list rather than
extending it — and the four 🟡 items above are folded into them rather than
being declared done.

**Tier 1 — Truth debt (cheap, do first):**
1. Keep this matrix honest as features land. The 2026-08-15 pass down-ticked
   18 rows; 13 of those were the same shape — machinery built and tested,
   last mile into production unwired. Prefer 🟡 at merge time over a ✅ that
   has to be undone.
2. Close the stub clusters that currently *look* shipped: §M `/agents`,
   `/login`/`/logout`/`/status`/`/setup-token`, `/heapdump`, `/feedback`,
   `/tui`, `/loop`.

**Tier 2 — Wire the last mile (machinery already exists):**
3. Emit cost/token/active-time metrics from the turn loop (#467) — unblocks
   the K "Metric set" row.
4. Consume the parsed OTLP mTLS material (#465).
5. Give `caliban_images::resolve_image_attachments` a caller — an `--image`
   flag and a TUI paste path (§E).
6. Construct `SettingsWatcher` in the binary so settings live-reload and the
   `ConfigChange` hook actually fire (§D, §B) — and dispatch `CwdChanged` and
   `Notification`, which have no call site either.
7. Flip `tools.lazy_mcp` on by default per ADR-0046's v1.1 promise (§F).
8. Populate `enabled_plugins` at `startup/compose.rs:1680` so
   `force_for_plugin` stops being inert (§L).

**Tier 3 — Real feature gaps, ranked:**
9. **Auth surface** — the largest stub cluster, and it has *no spec and no
   ADR*. Write the spec first.
10. Hook inheritance for foreground sub-agents (`NoopHooks` today) + config
    handlers on events beyond the three `event_supported` allows (§B).
11. Sandbox configuration surface + the per-hostname egress allowlist (#477,
    #481) — §A.
12. `WaitForMcpServers` (§F), `/agents` fleet overlay, `/resume` picker.
13. Native structured output for `--json-schema` (needs an ADR).

**Tier 4 — Ecosystem / packaging:**
14. Publish a reusable GitHub Action (#39) and a devcontainer feature (#40).
15. Foundry provider (#30); PowerShell tool.
16. Skills polish sub-project — currently **one** bundled skill vs upstream's
    nine; unblocks `/code-review`, `/run`, `/verify`, `/debug`, `/batch`.

**Tier 5 — TUI polish & long tail:**
Vim mode, voice dictation, `/theme` + a color system, the fullscreen renderer,
emoji shortcodes, and everything in §N.

---

## Refresh process

1. When a feature lands: edit the relevant row(s) in this matrix in the
   same PR, ticking 🔴 → 🟡 or 🟡 → ✅ as appropriate. **Cite the evidence**
   (file path, ADR number, or PR number) in the Notes column.
2. When Claude Code ships something new: refresh
   [`capability-inventory.md`](capability-inventory.md)
   first (re-fetch the upstream docs, bump its snapshot date **and** its
   currency marker), then propagate any new rows here.
3. Bump the **Last refreshed** line at the top. That line is a running
   history — append your entry and keep the prior "Prior refresh …" chain
   intact.
4. Periodically re-verify the ✅ rows too, not just the 🔴/🟡 ones. A ✅ that
   was true when it was written can rot; of the 2026-08-15 pass's 18
   down-ticks, 17 were rows previously marked ✅.
