# Caliban ↔ OpenCode parity gap matrix

> **What this is:** a living checklist of feature parity between caliban
> (this project) and **OpenCode** (`opencode.ai`) — a genuine head-to-head
> terminal-coding-agent competitor. Refresh it whenever a major feature lands
> or OpenCode ships a new capability. Use it — alongside the
> [Claude Code](../claude-code/parity-gap-matrix.md),
> [Codex](../codex/parity-gap-matrix.md),
> [Grok Build](../grok-build/parity-gap-matrix.md), and
> [Google Antigravity](../antigravity/parity-gap-matrix.md) matrices — to prioritize the next sprint.
>
> **How to use it:** unlike OpenClaw (a gateway that *orchestrates* coding
> agents), OpenCode is the same *kind* of thing as caliban, so most rows are
> real apples-to-apples comparisons. When shipping a feature that closes a row,
> tick it 🔴 → 🟡 or 🟡 → ✅ in the same PR.
>
> **Companion document:** [`capability-inventory.md`](capability-inventory.md)
> — a structured, dated snapshot of OpenCode's documented surface, captured from
> `opencode.ai/docs`. That file is the *source* this matrix is derived from;
> refresh both together.

**Legend:** ✅ caliban has an equivalent · 🟡 partial · 🔴 gap · **n/a** =
OpenCode-surface concept with no intended caliban analogue (e.g. hosted plane).
A ✅ means "caliban does the equivalent thing," not byte-identical.

> **Counting convention (shared across the sibling matrices).** Counts are
> **capability-table rows in the lettered sections** — the *OpenCode-distinctive
> gaps* list is excluded. A **down-tick** is a row whose rating got worse,
> *including* a combined row split into worse-scoring halves; an **up-tick** is
> the reverse; deleting a duplicate row is neither. A change that rewrites only
> the Notes cell without moving the rating is a **note-only correction** and is
> counted separately. This matrix has **73** scored rows.

**Last refreshed:** 2026-08-15 (**caliban-side scoring sweep, #519** — no
upstream re-baselining; the OpenCode inventory snapshot stays 2026-07-27,
#505/#487). Every ✅ row was re-verified against `main` at v0.8.0 under the rule
now written down in [`../../README.md`](../../README.md#scoring-rule-for-parity-matrices):
a row is ✅ only when a **production call path from the shipped binary**
reaches it. **17 down-ticks, 0 up-ticks, 8 note-only corrections** across 25 of
73 rows — the heaviest sweep of the four, because this matrix inherited the
most caliban ratings from the Claude Code matrix without re-verification. Six
rows went **straight to 🔴**: `external_directory` (§E), built-in agent roles
and Markdown agent definitions (§F), snapshot/undo (§H), image drag-and-drop
and theme/keybind customization (§M). Two confirmed defects relayed from PR
#517 are fixed — `caliban mcp` (§C and §I) and image ingest (§H/§M). The
recurring pattern is #516's: machinery that compiles and is unit-tested with no
non-test caller (the whole `caliban-checkpoint` crate, image ingest, four of
five plugin aggregations, `Settings::additional_directories`). One row is a
**safety** finding rather than a scoring one: §E's per-command bash patterns —
the documented-elsewhere `Bash(git *)` form silently never matches, and
imported Claude Code rules are copied verbatim, so they land dead. Prior
refresh 2026-07-27 (primary-source refresh of the OpenCode column — derived
from [`capability-inventory.md`](capability-inventory.md) snapshot 2026-07-27,
verified against live `opencode.ai/docs/*` (HTTP 200); caliban state
cross-referenced from the [Claude Code parity
matrix](../claude-code/parity-gap-matrix.md) as of its 2026-06-17 refresh and
**not** re-verified against `main` that pass — competitor facts/notes only;
#487).

> **Caveat:** rows tagged **⚠** depend on an OpenCode fact still flagged
> uncertain in the inventory (§14 there). The "caliban detail inferred from the
> Claude Code matrix rather than re-verified against `main`" half of this
> caveat was **retired 2026-08-15 (#519)** — every caliban rating in this file
> is now verified directly against `main`.

---

## A. Install & distribution

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| npm / Bun / pnpm / Yarn / Homebrew / Arch / Choco / Scoop / Mise / Docker | 🔴 | caliban builds from source via `cargo`; no package-manager channels yet |
| Self-update (`opencode upgrade`) | 🔴 | no built-in updater |

## B. Architecture (client/server)

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Client/server core; sessions survive client disconnects | 🔴 | caliban is a single-process TUI + headless `-p`; `caliband` supervises subagents, not a session server clients attach to. Note: default `opencode` **already** runs a TUI-client + server together — client/server is its default shape, not an opt-in mode |
| Headless HTTP server (`opencode serve`) | 🔴 | no API server surface. Gap is **more pronounced** than a bare "no server": OpenCode publishes an **OpenAPI 3.1** spec at `/doc` with HTTP basic auth (`OPENCODE_SERVER_PASSWORD`/`OPENCODE_SERVER_USERNAME`) — a fully specified, driveable API, not just a socket |
| Attach a client to a running backend (`opencode attach`) | 🔴 | no attach model |
| Web UI (`opencode web`) | 🔴 | terminal-first (shared with the Claude Code long-tail) |
| ACP (Agent Client Protocol) server (`opencode acp`) | 🔴 | no editor-driving protocol server |

## C. CLI subcommands

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Non-interactive run (`run`, `--format json`) | ✅ | `-p` + `--output-format json/stream-json` (ADR-0025) |
| Continue / session / fork flags | 🟡 | `/resume` + `--resume`; checkpoint fork partial (see Claude Code matrix) |
| Provider auth (`auth login`) | 🟡 | `/login`/`/logout`/`/status` are stubs; auth via env + `apiKeyHelper` |
| Manage agents (`agent create/list`) | 🟡 | subagent files exist; `/agents` editor is a stub |
| List models (`models`) | 🟡 | `/model` runtime swap; no `models` catalog command |
| MCP management (`mcp add/list/...`) | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** There is **no `caliban mcp` subcommand** — `CalibanCommand` (`caliban/src/args.rs`) has no `Mcp` variant, so no `add`/`list`/`remove` verbs exist. Servers are declared in TOML (`mcp.toml` / settings `[mcpServers]`) and hand-edited; `--no-mcp` disables the lot. `/mcp` renders live per-server status, but its action keys are toast stubs — "disable not yet wired — edit `disabled = true` in mcp.toml then restart" (`caliban/src/tui/events.rs:1143`). Connection, per-server `disabled`, permission scoping, and OAuth are all real (ADR-0023) |
| GitHub automation (`github install/run`) | 🔴 | GitHub Actions deferred |
| Checkout a PR (`pr`) | 🔴 | no PR-checkout command |
| Session list/delete | 🟡 | `/resume` picker; no explicit delete command |
| Usage/cost stats (`stats`) | ✅ | `/usage` + `/cost` (ADR-0033) |
| Export / import session | 🟡 | `/export` ✅; import from JSON/share-URL 🔴 |
| Install plugins (`plugin`) | ✅ | `caliban plugin` (ADR-0030) |
| Diagnostics (`debug`, `db path`) | ✅ | `caliban doctor` / `/doctor` |

## D. Config system

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Merged multi-source config (not replaced) | ✅ | layered settings (managed/user/project/local) with per-key merge (ADR-0026) |
| Project + global + managed + MDM sources | ✅ | four scopes + managed delivery |
| Remote config (`.well-known/opencode`) | 🔴 | no remote-config fetch |
| `{env:VAR}` / `{file:path}` substitution | 🟡 | **Note narrowed 2026-08-15 (#519).** `${VAR}` (with `${VAR:-default}`) expands in **MCP config fields only** (`crates/caliban-settings/src/settings.rs:442-451,711-760`; legacy loader `crates/caliban-mcp-client/src/config.rs:536-541`), plus `${CALIBAN_PLUGIN_ROOT}`/`${CLAUDE_PLUGIN_ROOT}` in plugin files (`crates/caliban-plugins/src/expand.rs:16-49`) and an *allowlisted* env set for HTTP hooks. It is **not** applied to settings generally. No `{file:...}` inclusion; the nearest analogue is CLAUDE.md `@`-imports (ADR-0036) |
| `instructions` glob array | ✅ | CLAUDE.md ancestry + `@`-imports (ADR-0036) |
| Separate TUI theme/keybind config | 🔴 | **Down-ticked from 🟡 2026-08-15 (#519).** The custom statusline runner is real (`crates/caliban-settings/src/statusline.rs:68` → `caliban/src/tui/app.rs:477`), but it is not TUI theme/keybind config. There is **no theme system** (zero `theme` hits in `caliban/src`; the only settings field is an opaque passthrough, `crates/caliban-settings/src/settings.rs:358`) and **no keybinding config at all** (zero hits for `keybinding`/`keymap`; every chord is hardcoded in `caliban/src/tui/events.rs`). Consistent with §M |

## E. Permissions

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| `allow` / `ask` / `deny` resolution | ✅ | rule grammar + modes (ADR-0029/0045) |
| Per-tool + wildcard defaults | ✅ | ordered `[[permissions.rules]]` with globstar |
| Per-command bash patterns, last-match-wins | 🟡 | **Down-ticked 2026-08-15 (#519) — both halves of the old note were wrong.** Per-command patterns work, but the syntax is **`Bash:git *`, not `Bash(git *)`**: `split_pattern` splits on `:` only (`crates/caliban-agent-core/src/permissions_matcher.rs:30-33`), so a paren form leaves `tool_pat = "Bash(git *)"` and the gate at `:83` runs `glob_match("Bash(git *)", "Bash")` → false. **`Bash(git *)` silently never matches**, and `crates/caliban-settings/src/import.rs:129-143` copies imported Claude-Code-style patterns **verbatim**, so importing a real `settings.json` yields dead rules; `caliban perms lint` (`caliban/src/perms_cli.rs:465`) only dedupes and will not flag it. Resolution is also **first**-match-wins over the ordered v2 `rules` array (`permissions.rs:198-200`), not last-match-wins; deny→ask→allow ordering applies only when flattening the *legacy* three-bucket form (`crates/caliban-settings/src/settings.rs:554-571`) |
| Agent-level permission overrides | 🟡 | **Down-ticked 2026-08-15 (#519).** Per-subagent **tool scoping is real** (`tool_allowlist`, `crates/caliban-tools-builtin/src/agent/agent_tool.rs:60` → `caliban/src/startup/compose.rs:886-900`), and background sub-agents inherit a permission slice (`InheritableHookConfig{rules, mode, audit, runtime_rules}`, `caliban/src/hook_inherit.rs:15-31`). But **`permissionMode` does not exist as a per-agent override** — zero hits in any Rust file repo-wide; ADR-0037's frontmatter surface was never built (see §F). Foreground sub-agents get `NoopHooks` outright (`compose.rs:919-926`, admitted at `:855`) |
| `external_directory` gate | 🔴 | **Down-ticked from ✅ 2026-08-15 (#519).** Neither mechanism in the old note exists. There is **no `--add-dir` flag** anywhere in `caliban/src/args.rs`. `Settings::additional_directories` (`crates/caliban-settings/src/settings.rs:416`) parses and merges but has **no reader in `caliban/src/`**, and the field it would feed, `MemoryConfig::additional_dirs`, is hardwired `Vec::new()` at both production constructors (`crates/caliban-memory/src/config.rs:139,169`) — so the consumer loop at `crates/caliban-memory/src/loader.rs:140-141` always iterates empty. Even fully wired it would only widen the CLAUDE.md walk; it never gated the tool path fence. Dead config surface, tracked against ADR-0036 |
| `doom_loop` (repeated-identical-call) guard | 🔴 | turn-loop resilience exists, but no dedicated repeated-call guard. OpenCode's `doom_loop` fires specifically when the **same tool call repeats 3 times with identical input** (confirmed `/docs/permissions/`) |
| `.env` read denied by default | 🟡 | achievable via rules; not a shipped default. OpenCode ships `*.env` / `*.env.*` **deny** rules out of the box (confirmed `/docs/permissions/`) |

## F. Agents / subagents

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Subagents with per-agent model/tools/permissions | 🟡 | **Down-ticked 2026-08-15 (#519).** Per-agent **model** and **tools** are real, but they come from the inline `AgentTool` JSON input, not frontmatter: `model` (`crates/caliban-tools-builtin/src/agent/agent_tool.rs:63` → `caliban/src/startup/compose.rs:885`) and `tool_allowlist` (`:60` → `compose.rs:886-900`). **Per-agent permissions do not exist** — `permissionMode` has zero Rust hits (see §E) |
| Built-in Explore / Plan / general roles | 🔴 | **Down-ticked from ✅ 2026-08-15 (#519).** caliban ships **no built-in agent definitions at all** — the only built-in markdown asset in the tree is `crates/caliban-skills/src/builtins/auto_memory.md` (registered at `caliban/src/startup/compose.rs:653`), and there is no agent-definition loader to register roles with (see the row below). "Explore / Plan / general-purpose analogues" described Claude Code's built-ins, not caliban's. `EnterPlanMode`/`ExitPlanMode` are *tools* (`compose.rs:615-616`), not an agent role |
| Markdown agent definitions + frontmatter | 🔴 | **Down-ticked from ✅ 2026-08-15 (#519).** There is **no `.caliban/agents/*.md` loader** — no agent-definition discovery at any path in any Rust file. `SpawnSpec.frontmatter_path` (`crates/caliban-supervisor/src/proto.rs:95`) is hardcoded `None` at every production construction site (`caliban/src/startup/compose.rs:954`, `caliban/src/agents_cli.rs:323,465`, `caliban/src/tui/events.rs:1008`, `caliban/src/worker.rs:1075`). Plugin `agents/` roots are computed (`crates/caliban-plugins/src/aggregate.rs:42-46`) and never consumed |
| `steps` / max-iteration cap | 🟡 | **Down-ticked 2026-08-15 (#519).** A cap exists and is enforced — the sub-agent stops with "[sub-agent exhausted max_turns without completing]" (`crates/caliban-tools-builtin/src/agent/agent_tool.rs:340`) — but it is **not per-subagent**: the value is hardcoded `max_turns: 20` at `caliban/src/startup/compose.rs:914`, with no input field and no settings key. `SUB_AGENT_MAX_TURNS` (`agent_tool.rs:21`) is a dead const, re-exported at `:374` and read nowhere |
| `subagent_depth` recursion control | 🟡 | **Down-ticked 2026-08-15 (#519).** Recursion is genuinely prevented, but structurally rather than by a depth control: the child's registry snapshot is taken *before* `AgentTool` is registered (`caliban/src/startup/compose.rs:874-880` vs `:1012`), and the allowlist branch skips it (`:890-892`), so a sub-agent cannot spawn one — depth is fixed at 1 and not configurable. `subagent_depth` has zero hits repo-wide; ADR-0021 defers depth limits explicitly (`docs/adr/0021-sub-agent-primitive.md:54-55`) |
| `@`-mention manual subagent invocation | 🟡 | invoked via `AgentTool`/Task; `@agent` mention not a direct match |
| Primary-agent switching (Build/Plan via Tab) | 🟡 | plan mode + Shift+Tab cycle overlap, but not "swap the primary agent" |
| Plan mode | ✅ | `/plan` + plan permission mode |
| Worktree isolation | ✅ | **Note corrected 2026-08-15 (#519).** Real on the **background** path only: `isolation: worktree` → `caliban/src/startup/compose.rs:959-962` → `crates/caliban-supervisor/src/server.rs:465-479` → `WorktreeManager` (ADR-0037). On the **foreground** path the factory never reads `input.isolation` and never changes cwd (`compose.rs:884-927`), so the flag is a silent no-op there; `caliban agents spawn` hardcodes it false (`caliban/src/agents_cli.rs:474`); and `WorktreeOptions{base_ref, sparse_paths, symlink_directories}` are accepted by the tool schema and dropped before `WorktreeSpec::new` (`server.rs:683`). Reachable in production, so the ✅ stands. OpenCode has no first-class worktree isolation |

## G. Models & providers

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Provider breadth (75+ via Models.dev) | 🟡 | **Note corrected 2026-08-15 (#519) — the list was wrong; the gap is wider than stated.** The binary can construct **four** providers: Anthropic / OpenAI / Ollama / Google (`ProviderKind`, `caliban/src/args.rs:88-95`; `build_provider`, `caliban/src/startup/compose.rs:161-180`; router arms `caliban/src/router.rs:90-150`). **Bedrock and Vertex are not among them** — `caliban/Cargo.toml` does not depend on `caliban-provider-{bedrock,vertex}`, so no CLI path can construct either; the ADR-0034 crates are library-complete but reachable only from their own integration tests. Four hardcoded providers vs 75+ |
| Local runners (Ollama / LM Studio / OpenAI-compatible) | ✅ | ollama + LMStudio probed |
| Provider-priority routing / fallback | ✅ | **Note corrected 2026-08-15 (#519) — real, but opt-in.** Sequential fallback (`crates/caliban-model-router/src/dispatch.rs:20,98,134`), hedging (`:144-168` → `hedging.rs::race_hedged`) and per-route circuit breakers (`:83-124,182-253`) are on the router's live dispatch path, and the router becomes *the* provider for the run when loaded (`caliban/src/main.rs:281-296`). It loads **only when a `caliban.toml` with a `[router]` table is discovered** (`caliban/src/router.rs:47-50` returns `Ok(None)` otherwise) — with no such file none of this runs. A configured production path, so ✅ stands |
| `small_model` split for light tasks | ✅ | **Note corrected 2026-08-15 (#519) — real, but doubly gated.** The purpose is stamped in production (`RequestPurpose::FastClassifier` at `crates/caliban-agent-core/src/auto_mode.rs:394`; `MainLoop` at `stream/mod.rs:1103`; `Summarization` at `compact.rs:611`), but purpose→route resolution lives **inside the router** (`crates/caliban-model-router/src/config.rs:363-477`), so an actual fast/heavy split needs a `caliban.toml` route keyed `purpose = "fast_classifier"`. The classifier itself is additionally gated on `permission_mode = auto` (`caliban/src/startup/compose.rs:1157-1168`). On the single-provider path `purpose` is inert — the same model serves both |
| Browser OAuth for providers | 🟡 | MCP OAuth shipped; provider-login OAuth not |
| `--thinking` / reasoning controls | ✅ | `/effort` + `/think` (ADR-0038/#100) |

## H. Tools

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| `read`/`write`/`edit`/`bash`/`glob`/`grep`/`webfetch`/`websearch`/`task`/`skill` | ✅ | **Note filled 2026-08-15 (#519).** All registered in `build_registry` (`caliban/src/startup/compose.rs:592-616`, plus `Skill` :654, `ToolSearch` :847, `AgentTool` :1012), reached from `caliban/src/main.rs:372` and `caliban/src/worker.rs:595`. Also `MultiEdit`, `NotebookEdit`, `BashOutput`, `KillShell`, `TodoWrite`, `EnterPlanMode`/`ExitPlanMode`, memory-topic tools. Naming note: `task` is spelled **`AgentTool`** (`crates/caliban-tools-builtin/src/agent/agent_tool.rs:172`). No stubs in this set |
| Snapshot file-tracking + `/undo`/`/redo` | 🔴 | **Down-ticked from ✅ 2026-08-15 (#519).** `caliban-checkpoint` is complete and unit-tested (store/recorder/restore/prune/hook, ADR-0028) and **entirely unreachable from the binary**: `CheckpointHook` has no construction site outside the crate's own tests, so nothing is ever snapshotted; `App::with_checkpoint_store` (`caliban/src/tui/app.rs:573`) carries `#[allow(dead_code, reason = "wired by main.rs once full /rewind action plumbing lands")]` and has zero callers, so `app.checkpoint_store` is always `None` (`app.rs:550`) and `/rewind` renders "(checkpointing not enabled for this session)" (`caliban/src/tui/overlay.rs:826`). Even with a store, the advertised `[c]/[v]/[b]/[s]` actions are inert — `Overlay::Rewind` has no entry in the key dispatcher (`caliban/src/tui/events.rs:562-589`). There is no undo, no redo, and no file snapshotting |
| LSP integration (diagnostics/symbols to the agent) | 🔴 | no Language-Server integration — an OpenCode-distinctive gap. OpenCode's LSP is **default-off** (`"lsp": true` to enable) but ships **30+ built-in servers** (Python/TS/Rust/Go/PHP…) with auto-install once on |
| Auto-formatters on edit (`formatter`) | 🔴 | no post-edit formatter hook |
| User-defined custom tools (`.opencode/tools/`) | 🟡 | **Note narrowed 2026-08-15 (#519).** Extension is via MCP or skills; **plugins cannot contribute tools at all** — `plugin.json`'s component set is skills/hooks/agents/output_styles/mcp_servers/commands (`crates/caliban-plugins/src/manifest.rs`), with no `tools` key, and four of those five aggregations are never consumed anyway (see §L). No `.caliban/tools/` dir concept exists |
| Image input | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** ADR-0039's provider-side `ImageBlock` wire support is real, but **no production path ingests an image**: `resolve_image_attachments` (`caliban/src/tui/attach.rs:218`) is `#[allow(dead_code, reason = "wired into a follow-up TUI input slice")]` with test-only callers (`:480,499`); `paste_image_from_clipboard` (`crates/caliban-images/src/clipboard.rs`) and `parse_drag_drop_escape` (`dnd.rs`) have no callers outside their own modules; the text attach path *skips* image files (`attach.rs:146`); `Read` is text-only (`crates/caliban-tools-builtin/src/fs/read.rs`); and there is no `--image` flag |

## I. MCP

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| MCP client (local + remote servers) | ✅ | rmcp client, stdio + HTTP (ADR-0023) |
| `mcp add/list/auth/logout` CLI | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** No `caliban mcp` subcommand exists — `CalibanCommand` (`caliban/src/args.rs`) has no `Mcp` variant, so none of `add`/`list`/`auth`/`logout` is available from a shell. OAuth itself is real but runs *implicitly* on connect (`crates/caliban-mcp-client/src/manager.rs:212-217,315-317`; PKCE + loopback + keyring, `oauth.rs`), with no login/logout verb to drive or revoke it. Same evidence as §C |
| Driven via HTTP server / ACP / SDK | 🔴 | see B — no server/ACP surface for being driven. OpenCode's driving surface is substantial: **OpenAPI 3.1** HTTP API + a typed **`@opencode-ai/sdk`** (JS/TS, `createOpencode()` / `createOpencodeClient()`) + a **Go SDK** + `acp` — the caliban gap here is deeper than "no socket." (Dedicated MCP-*server* mode leaning-refuted upstream — canonical MCP slug 404'd this pass) |

## J. Sharing / sessions / persistence

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Hosted share links (`/share`, `share` config) | 🔴 | no hosted share plane (n/a-adjacent — local-first) |
| Export / import (`--sanitize`, share-URL import) | 🟡 | `/export` ✅; sanitized/URL import 🔴 |
| Persistent session store (SQLite) | ✅ | **Note corrected 2026-08-15 (#519) — substrate was wrong, capability stands.** Not SQLite: one pretty-JSON file per session on disk (`FsSessionBackend`, `crates/caliban-sessions/src/backend/fs.rs:12-15`), selected at `caliban/src/startup/storage.rs:73` from `caliban/src/main.rs:545`, with debounced atomic writes (`crates/caliban-sessions/src/store.rs`). The `gonzalo` remote backend sits behind an off-by-default cargo feature; `git`/`s3` substrates parse but hard-error as unwired (`storage.rs:76-78`). Persistence + transcripts are genuinely production |

## K. GitHub / GitLab / CI

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| GitHub Action automation (`github install/run`) | 🔴 | deferred sub-project |
| PR checkout (`pr`) | 🔴 | no PR-checkout helper |
| GitLab Duo integration | 🔴 | no GitLab integration |
| Headless JSON for scripting (`run -f json`, `--pure`) | ✅ | `--output-format json` + `--bare` |

## L. Developer surface / enterprise

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Plugins (npm-loaded) | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** Install/update/remove/enable/disable and the HTTP marketplace (JSON index + `.tar.gz` + sha256) are real (`crates/caliban-plugins/src/{cli,marketplace}.rs`, ADR-0030). But an installed plugin only ever contributes **skills**: of `PluginManager`'s five aggregations, only `skill_roots` has a non-test consumer (`crates/caliban-plugins/src/manager.rs:262` → `caliban/src/main.rs:326` → `caliban/src/startup/compose.rs:632`); `hooks_configs` (:285), `mcp_servers` (:293), `agent_roots` (:276) and `output_style_roots` (:270) are parsed, namespaced, expanded and discarded, and `compose.rs:1527,1679` pass `enabled_plugins: &[]`. No npm loading — sources are HTTP marketplace or local `--dir` sideload (`crates/caliban-plugins/src/discovery.rs:52-61`) |
| SDK / documented Server API | 🔴 | no embedding SDK / HTTP API. OpenCode ships a **generated, type-safe JS/TS SDK** (`@opencode-ai/sdk`), a **Go SDK**, and an **OpenAPI 3.1** server spec (`/doc`) — the 🔴 reflects a genuinely richer competitor surface, not merely a missing endpoint |
| Managed config + MDM | ✅ | managed settings scope (ADR-0026/0045) |
| Resource-access policies (`experimental.policies`) | 🟡 | permissions + sandbox cover much of this; no separate policy engine |
| Hosted model gateway (OpenCode Zen) | n/a | no first-party hosted gateway |

## M. TUI ergonomics

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Plan mode toggle | ✅ | `/plan` + Shift+Tab |
| Undo/redo | 🔴 | **Down-ticked from ✅ 2026-08-15 (#519).** `/rewind` opens an overlay that is always empty — `checkpoint_store` is never populated (`caliban/src/tui/app.rs:550`; the only setter, `with_checkpoint_store` at `:573`, is `#[allow(dead_code)]` with zero callers), so `caliban/src/tui/overlay.rs:826` short-circuits to "(checkpointing not enabled for this session)". `CheckpointHook` is never constructed either, so nothing is snapshotted in the first place. See §H |
| Image drag-and-drop | 🔴 | **Down-ticked from ✅ 2026-08-15 (#519).** `parse_drag_drop_escape` / `DragDropPayload` (`crates/caliban-images/src/dnd.rs`) are exported at `lib.rs:24` and have **no callers outside their own module** — the TUI key/paste path never invokes them, and there is no bracketed-paste DnD handler. ADR-0039 describes the design; nothing reaches it. See §H |
| Theme + keybind customization | 🔴 | **Down-ticked from 🟡 2026-08-15 (#519).** Neither half is partial — both are absent. **No `/theme` command and no colour system**: `grep -rni theme` over `caliban/src` returns nothing; the sole repo hit is a doc comment calling the TUI settings table opaque (`crates/caliban-settings/src/settings.rs:358`). **No keybinding configuration of any kind**: `keybinding`/`key_binding`/`keymap` have zero hits across `caliban/` and `crates/`; every chord is a hardcoded match arm in `caliban/src/tui/events.rs`. The old 🟡 ("keybinds partial") had no basis |

---

## OpenCode-distinctive gaps worth a ticket

Capabilities OpenCode has that caliban lacks and that aren't already tracked by
the Claude Code matrix — the highest-signal candidates if we chase OpenCode
parity specifically:

1. **Client/server core + `serve`/`attach` + ACP + typed SDKs** (B/I) — a
   backend other front-ends (web, IDE, another agent) attach to, exposed via an
   **OpenAPI 3.1** spec (`/doc`), a typed **`@opencode-ai/sdk`** (JS/TS) and a
   **Go SDK**. This is OpenCode's biggest architectural difference (and its
   *default* runtime shape, not an opt-in mode) and **overlaps with the "caliban
   as a worker backend" note under [OpenClaw](../openclaw/README.md)** (the full
   OpenClaw comparison lives in the Prospero repo) — a server/ACP/SDK surface
   would serve both.
2. **LSP integration** (H) — feed Language-Server diagnostics/symbols to the
   agent. No caliban analogue; high coding-quality leverage.
3. **Auto-formatters on edit** (H) — run prettier/gofmt/etc. after file edits.
4. **`doom_loop` guard** (E) — a dedicated repeated-identical-tool-call circuit
   breaker (OpenCode's fires at **3 identical calls**).
5. **Self-update** (A) — `caliban upgrade`.
6. **GitHub Action + PR checkout** (K) — shared with the Codex/Claude Code
   long-tail; already a known deferred sub-project.
7. **Agent definition files + built-in roles** (F) — **new 2026-08-15 (#519).**
   OpenCode ships Markdown agent definitions with frontmatter *and* built-in
   Explore/Plan/general roles; caliban has **neither** — no definition loader
   at any path, and no built-in agent assets. Every per-agent choice must be
   repeated inline on each `AgentTool` call. Shared with the Grok Build and
   Antigravity matrices.
8. **Restore the last mile on work already built** (E/H/M) — **new 2026-08-15
   (#519).** Four ✅ rows fell this pass purely for want of a call site:
   `caliban-checkpoint` (undo/redo, §H/§M), image ingest (§H/§M),
   `Settings::additional_directories` (§E), and four of five plugin
   aggregations (§L). None needs new design; all four are wiring.

Hosted share plane, web UI, and OpenCode Zen are **deliberately out of scope**
(n/a) — caliban is a local-first terminal agent.

---

## Refresh process

1. When a caliban feature lands: edit the relevant row(s) in the same PR,
   ticking 🔴 → 🟡 or 🟡 → ✅.
2. When OpenCode ships something new: refresh
   [`capability-inventory.md`](capability-inventory.md) first (re-fetch the
   upstream docs), then propagate any new rows here.
3. Resolve any **⚠** rows against OpenCode's live docs / caliban `main` when you
   touch them.
4. Bump the **Last refreshed** date at the top.
