# Caliban ↔ Grok Build parity gap matrix

> **What this is:** a living checklist of feature parity between caliban
> (this project) and **Grok Build** (`x.ai/cli`) — xAI's terminal coding agent,
> a genuine head-to-head competitor. Refresh it whenever a major feature lands
> or Grok Build ships a new capability. Use it — alongside the
> [Claude Code](../claude-code/parity-gap-matrix.md),
> [Codex](../codex/parity-gap-matrix.md),
> [OpenCode](../opencode/parity-gap-matrix.md), and
> [Google Antigravity](../antigravity/parity-gap-matrix.md) matrices — to
> prioritize the next sprint.
>
> **How to use it:** Grok Build is the same *kind* of thing as caliban (a
> terminal coding agent), so most rows are real apples-to-apples comparisons.
> One twist: Grok Build reads the **Claude Code / AGENTS.md** ecosystem
> natively, and caliban is itself a Claude Code-lineage agent, so many config
> rows land ✅ almost by construction. When shipping a feature that closes a
> row, tick it 🔴 → 🟡 or 🟡 → ✅ in the same PR.
>
> **Companion document:** [`capability-inventory.md`](capability-inventory.md)
> — a structured, dated snapshot of Grok Build's documented surface. That file
> is the *source* this matrix is derived from; refresh both together.

**Legend:** ✅ caliban has an equivalent · 🟡 partial · 🔴 gap · **n/a** =
Grok-Build-surface concept with no intended caliban analogue (e.g. hosted
model plane). A ✅ means "caliban does the equivalent thing," not
byte-identical.

> **Counting convention (shared across the sibling matrices).** Counts are
> **capability-table rows in the lettered sections** — the *Grok-Build-distinctive
> gaps* list is excluded. A **down-tick** is a row whose rating got worse,
> *including* a combined row split into worse-scoring halves; an **up-tick** is
> the reverse; deleting a duplicate row is neither. A change that rewrites only
> the Notes cell without moving the rating is a **note-only correction** and is
> counted separately. This matrix has **51** scored rows.

**Last refreshed:** 2026-08-15 (**caliban-side scoring sweep, #519** — no
upstream re-baselining; the Grok Build inventory snapshot stays 2026-07-27,
#505). Every ✅ row was re-verified against `main` at v0.8.0 under the rule now
written down in [`../../README.md`](../../README.md#scoring-rule-for-parity-matrices):
a row is ✅ only when a **production call path from the shipped binary**
reaches it. **11 down-ticks, 1 up-tick, 7 note-only corrections** across 19 of
51 rows. Two confirmed defects relayed from PR #517 are fixed — `caliban mcp`
(§C and §L; the subcommand does not exist) and image ingest (§H). The single
up-tick is `AGENTS.md` (§D), which was **understated**. The heaviest finding is
a cluster this matrix is uniquely exposed to: Grok's selling point is reading
*other agents'* trees, and caliban reads **none** of them — no `.claude/`
skills, agents, or hook scanners anywhere (zero hits for the `".claude"` path
literal), and plugin-delivered hooks are parsed and discarded
(`PluginManager::hooks_configs` has no consumer). Two more rows fell to the
"machinery with no caller" pattern: checkpoint/`/rewind` (§H) and image ingest
(§H). §F's *"Markdown agent definitions"* row went straight to 🔴 — there is no
agent-definition file format at all. Prior refresh 2026-07-27 (primary-source
re-baseline — derived from [`capability-inventory.md`](capability-inventory.md)
snapshot 2026-07-27, now read **directly** off `docs.x.ai/build/*`; caliban
state cross-referenced from the [Claude Code parity
matrix](../claude-code/parity-gap-matrix.md) and the
[OpenCode matrix](../opencode/parity-gap-matrix.md)).

> **Caveat:** the "caliban detail inferred from the sibling matrices rather
> than re-verified against `main`" caveat was **retired 2026-08-15 (#519)** —
> every caliban rating in this file is now verified directly against `main`,
> and the four ⚠ markers that carried it are resolved in place. The remaining
> ⚠ (§L, Grok's first-party GitHub Action) is a *Grok-side* uncertainty for the
> next upstream re-baseline. The earlier "docs 403'd automated fetch,
> cross-checked from secondary sources" caveat no longer applies — the
> canonical docs are now directly readable, and the 2026-07-27 pass corrected
> several rows the secondary sources got wrong (permissions, sandboxing, LSP —
> see the inventory's "Corrections applied this pass").

---

## A. Install & distribution

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| One-line install script (`curl … x.ai/cli/install.sh`) | 🔴 | caliban builds from source via `cargo`; no install-script channel yet |
| Background self-update (`--no-auto-update` to disable) | 🔴 | no built-in updater |
| Open-source harness/TUI (Apache-2.0) | ✅ | caliban is open source (harness + TUI in-repo) |

## B. Surfaces & architecture

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Interactive fullscreen TUI (mouse, subagent view) | 🟡 | **Down-ticked 2026-08-15 (#519); the ⚠ is now resolved.** The TUI and mouse-wheel scroll are real (`caliban/src/tui/mouse_select.rs`, transcript scroll). There is **no subagent view**: `/agents` is a pure stub that prints "full sub-agent fleet overlay arrives with the Sub-agent isolation spec … use `caliban agents list` from a shell for now" (`caliban/src/tui/slash/config.rs:184`). Nor is caliban's a *fullscreen* renderer — no alt-screen app mode (`/tui` is also a stub, `caliban/src/tui/slash/dx.rs:151`) |
| Headless / non-interactive (`grok -p/--single`) | ✅ | `-p` + `--output-format json/stream-json` (ADR-0025) |
| ACP agent over JSON-RPC (`grok agent stdio`; driven by editors) | 🔴 | no editor-driving protocol server. Grok's is concrete: JSON-RPC over stdin/stdout with `initialize`/`authenticate`/`session/new` (declares `cwd` + `mcpServers`)/`session/prompt`/`session/update`. **Shared gap** with OpenCode `serve`/ACP + Codex `mcp-server`/`app-server` — tracked as epic **#503** |

## C. CLI subcommands

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Headless run w/ structured output (`-p`, `--output-format streaming-json`) | ✅ | `-p` + `--output-format json/stream-json` + `--bare` |
| Auto-approve flag (`--always-approve`) | ✅ | **Note corrected 2026-08-15 (#519) — the flag name was wrong.** There is no `--dangerously-skip-permissions`; the flag is **`--allow-dangerously-skip-permissions`** (`caliban/src/args.rs:597,605`, consumed at `caliban/src/main.rs:312,663`), and it *gates* `--permission-mode bypassPermissions` rather than being the bypass itself. `dontAsk` and `auto` modes cover the softer tiers (`crates/caliban-agent-core/src/permission_mode.rs:17-34`). Capability parity stands |
| MCP management (`grok mcp add/list/remove`) | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** There is **no `caliban mcp` subcommand** — `CalibanCommand` (`caliban/src/args.rs`) has no `Mcp` variant. No `add`/`list`/`remove` verbs exist in any form: servers are declared in TOML (`mcp.toml` / `settings` `[mcpServers]`) and edited by hand. `/mcp` shows live per-server status, but its action keys are toast stubs — "disable not yet wired — edit `disabled = true` in mcp.toml then restart" (`caliban/src/tui/events.rs:1143`). Real: connection, per-server `disabled`, per-server permission scoping, OAuth (ADR-0023) |
| Marketplace skills CLI (`grok skill search/install/list/remove`) | 🟡 | `caliban plugin` marketplace (ADR-0030) covers install/list; skill *search* over a hosted marketplace 🔴 |
| Discovery/inspect (`grok inspect`) | ✅ | `caliban doctor` / `/doctor` surfaces discovered config/MCP/sources |
| Provider auth / login subcommand | 🟡 | `/login`/`/logout`/`/status` are stubs; auth via env + `apiKeyHelper`. Grok has OIDC + API-key + device-code (RFC 8628) + external-broker auth |
| Named/resumable headless sessions (`-s/--session-id`, `-r/--resume`, `-c/--continue`) | 🟡 | `/resume` picker + `-r` resume; named-session create + `--continue`-latest-in-cwd not first-class; no explicit delete/list-and-manage command |

## D. Config system

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Layered global → project config | ✅ | layered settings (managed/user/project/local) with per-key merge (ADR-0026) |
| Reads `CLAUDE.md` + ancestry | ✅ | CLAUDE.md ancestry + `@`-imports (ADR-0036) |
| Reads `.claude/` tree (skills/agents/MCPs/hooks/rules) | 🟡 | **Down-ticked 2026-08-15 (#519).** caliban reads an *equivalent* tree, but not the `.claude/` one — a repo-wide grep for the path literal `".claude"` across `caliban/` and `crates/` returns **zero hits**. Skill roots are `<workspace>/.caliban/skills` + XDG config/data (`crates/caliban-skills/src/loader.rs:11-21`); there is no agents dir at any path (see §F); hooks/MCP come from caliban's own settings tree. `CLAUDE.md` **is** read, but as a memory file (ADR-0036), not as a `.claude/` directory. Grok's distinctive trick — ingesting an existing Claude Code tree in place — is the part that does not work |
| Reads AGENTS.md family | ✅ | **Up-ticked 2026-08-15 (#519); the ⚠ is resolved.** `AGENTS.md` is on the live ancestor walk alongside CLAUDE.md and `.caliban.md`: `ANCESTRY_FILENAMES = [".caliban.md", "CLAUDE.md", "AGENTS.md"]` (`crates/caliban-memory/src/project_walk.rs:42`), consumed on every run via `build_project_tier` (`crates/caliban-memory/src/loader.rs:135-145` ← `caliban/src/startup/compose.rs:1708`), ADR-0036. `/init` additionally imports `AGENTS.md`/`.cursorrules`/`.windsurfrules` (`crates/caliban-memory/src/init_import.rs:21`) — a separate path, not the only one |
| Reads Claude Code marketplaces/plugins natively | 🟡 | caliban plugin marketplace exists; cross-reading *Claude Code* plugin packs 🔴 |
| TOML config w/ local-inference pointer | ✅ | settings support provider/base-URL config incl. local runners |

## E. Permissions & sandboxing

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Per-tool-call approval gating | ✅ | rule grammar + modes (ADR-0029/0045) |
| Autonomy modes (`ask` / `auto` classifier / `always-approve`) | ✅ | ask + auto-approve + bypass modes; Grok's `auto` classifier auto-approves safe calls, prompts on risky — caliban has an equivalent auto tier |
| Allow/deny/ask rule grammar with patterns | ✅ | *at parity, not finer* — Grok has `allow`/`deny`/`ask` arrays with `Bash(git *)`/`Read(src**)`/`Edit(**/*.rs)`/`MCPTool(...)` patterns + verbose object form, `deny > ask > allow` (Claude-Code-class). caliban's grammar matches; **correction: the prior "finer-grained than Grok's" note was wrong** |
| User vs project-scoped permission override | ✅ | four config scopes with merge (ADR-0026); Grok honors `[permission]` in project `.grok/config.toml` |
| Kernel-enforced sandbox (fs allow/deny + network gating) | 🟡 | **Down-ticked 2026-08-15 (#519).** Kernel enforcement is genuinely real and production: macOS **Seatbelt** (`crates/caliban-sandbox/src/{detect.rs:62-101,shim.rs:163-179}`) and Linux **bubblewrap** (`bwrap.rs:86-105`), entered from `build_bash_fence` (`caliban/src/startup/compose.rs:537-556`) — though only when `--workspace`/`--restrict-paths` is passed (`caliban/src/args.rs:78-80`, off by default) and only around `Bash`. What is **not** there is the configurable half the ✅ claimed: the policy is hardcoded (`compose.rs:465-494`), `deny_read`/`deny_write` (`crates/caliban-sandbox/src/config.rs:24,31`) are populated by no production caller, and `SandboxSettings` exposes exactly one key — `network` (`crates/caliban-settings/src/settings.rs:65-71`). Network gating is all-or-nothing; per-domain ACLs are rejected without a proxy that does not ship (#477, open). So there is **no `read_only` equivalent and no user-writable allow/deny map** — the gap is not just Grok's preset ergonomics |

## F. Agents / subagents

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Parallel subagents (up to 8; research/impl/review) | ✅ | **Note corrected 2026-08-15 (#519).** Real and production, but there is **no sub-agent-specific cap** and no named research/impl/review roles: `AgentTool` calls share the generic parallel-tool semaphore (`crates/caliban-agent-core/src/stream/mod.rs:2016-2021`), sized `available_parallelism() - 1` (`crates/caliban-agent-core/src/agent.rs:25-30`) and tunable via `--parallel-tool-limit` / `--no-parallel-tools` (`caliban/src/args.rs:491,495`). Background agents are supervised by `caliband` (ADR-0037) |
| Per-subagent git-worktree isolation | ✅ | **Note corrected 2026-08-15 (#519) — real, with two sharp edges.** `isolation: worktree` is honored only on the **background** path: `caliban/src/startup/compose.rs:959-962` → `crates/caliban-supervisor/src/server.rs:465-479` → `WorktreeManager` (ADR-0037). On the **foreground** path the factory never consults `input.isolation` and never changes cwd (`compose.rs:884-927`), so the flag is a silent no-op there, and `caliban agents spawn` hardcodes `isolation_worktree: false` (`caliban/src/agents_cli.rs:474`). `WorktreeOptions{base_ref, sparse_paths, symlink_directories}` are accepted by the tool schema (`crates/caliban-tools-builtin/src/agent/agent_tool.rs:38-50`) and dropped before `WorktreeSpec::new` (`server.rs:683`). Reachable in production, so the ✅ stands |
| Parallel issue-fixing across worktrees | 🟡 | worktree isolation exists; a packaged multi-issue fan-out workflow 🔴 |
| Arena Mode (competing outputs) | 🔴 | no built-in competing-output/tournament mode |
| Markdown agent definitions + `/agents` | 🔴 | **Down-ticked from ✅ 2026-08-15 (#519).** Both halves were wrong. There is **no `.caliban/agents/*.md` loader** — no agent-definition discovery at any path in any Rust file, and `SpawnSpec.frontmatter_path` (`crates/caliban-supervisor/src/proto.rs:95`) is hardcoded `None` at every production construction site (`caliban/src/startup/compose.rs:954`, `caliban/src/agents_cli.rs:323,465`, `caliban/src/tui/events.rs:1008`, `caliban/src/worker.rs:1075`). Sub-agents are configured only by the inline `AgentTool` JSON input. And `/agents` is not a "🟡 UI" — it is a pure stub string (`caliban/src/tui/slash/config.rs:184`) |
| Recursion/depth control | 🟡 | **Down-ticked 2026-08-15 (#519).** The recursion *guard* is real but structural, not a depth control: the child's registry snapshot is taken before `AgentTool` is registered (`caliban/src/startup/compose.rs:874-880` vs `:1012`), so a sub-agent simply cannot spawn one — depth is fixed at 1. `subagent_depth` has zero hits repo-wide, and ADR-0021 defers depth limits explicitly (`docs/adr/0021-sub-agent-primitive.md:54-55`). `maxTurns` is **not per-subagent**: it is hardcoded `20` (`compose.rs:914`), and `SUB_AGENT_MAX_TURNS` (`crates/caliban-tools-builtin/src/agent/agent_tool.rs:21`) is a dead const |

## G. Models & providers

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Purpose-built coding model (grok-build-0.1, 256K) | n/a | caliban is model-agnostic; no first-party model |
| xAI / Grok provider | 🔴 | **Note corrected 2026-08-15 (#519) — the provider list was wrong.** The binary can construct **four** providers: Anthropic / OpenAI / Ollama / Google (`ProviderKind`, `caliban/src/args.rs:88-95`; `build_provider`, `caliban/src/startup/compose.rs:161-180`). **Bedrock and Vertex are not shipped providers** — `caliban/Cargo.toml` does not depend on `caliban-provider-{bedrock,vertex}`, so no CLI path can construct either (ADR-0034 crates are library-complete but reachable only from their own tests). Still no xAI/Grok backend wired |
| Runtime model swap (`/model`) | ✅ | `/model` runtime swap |
| Fast/heavy model split | ✅ | purpose-keyed routing + `FastClassifier` (ADR-0022); router v2 (ADR-0038) |
| Local / OpenAI-compatible inference | ✅ | Ollama + LM Studio probed |
| Reasoning-effort control | ✅ | `/effort` + `/think` (ADR-0038/#100) |

## H. Tools

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| read/write/edit/shell(`bash`)/search(`grep`)/`webfetch`/git/subagent | ✅ | full built-in tool set present |
| Diff-gated edits before apply | 🟡 | **Down-ticked 2026-08-15 (#519).** Edits *are* gated — `Write`/`Edit` default to `Ask` (`crates/caliban-agent-core/src/permissions.rs:209-232`) and the 4-button Ask modal is real (ADR-0027) — but it is **not diff-gated**: the modal shows a truncated `input_summary` (`caliban/src/tui/ask.rs:42`), and **no diff library exists in the workspace at all** (`similar`/`diffy`/`imara` appear in no `Cargo.toml`). The "auto-checkpoint + `/rewind`" half is worse: `CheckpointHook` is never constructed by the binary and `App::with_checkpoint_store` (`caliban/src/tui/app.rs:573`) carries `#[allow(dead_code, reason = "wired by main.rs once full /rewind action plumbing lands")]` with zero callers, so `checkpoint_store` is always `None` and the overlay renders "(checkpointing not enabled for this session)" (`caliban/src/tui/overlay.rs:826`). **There is no revert.** The `caliban-checkpoint` crate is complete and unit-tested (ADR-0028) — machinery, not a shipped path |
| Image input | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** `caliban-images` exists and the ADR-0039 provider-side `ImageBlock` wire support is real, but **no production path ingests an image**: `resolve_image_attachments` (`caliban/src/tui/attach.rs:218`) is `#[allow(dead_code)]` with test-only callers, `paste_image_from_clipboard` (`clipboard.rs`) and `parse_drag_drop_escape` (`dnd.rs`) have no callers outside their own modules, the text attach path *skips* image files (`attach.rs:146`), `Read` is text-only, and there is no `--image` flag |
| LSP servers (via plugins) | 🔴 | **correction:** Grok *does* integrate LSP (as a plugin extension type) — prior "no LSP" note was wrong. caliban has no LSP integration (no `caliban-lsp` crate / LSP ADR as of `main`) — a real gap, shared with the [OpenCode LSP row](../opencode/parity-gap-matrix.md) |

## I. Plan mode

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Plan mode (Shift+Tab; writes blocked except plan scratchpad) | ✅ | `/plan` + plan permission mode + Shift+Tab cycle |
| Approve / comment-per-step / rewrite the plan | 🟡 | **⚠ resolved 2026-08-15 (#519).** Approve-then-execute is real (`/plan` + `EnterPlanMode`/`ExitPlanMode` tools, `caliban/src/startup/compose.rs:615-616`; enforcement at `crates/caliban-agent-core/src/mode_filter.rs:120`). **Per-step commenting does not exist** — there is no per-step plan structure in the TUI to comment on, and the plan overlay has no key handler beyond close. Rating unchanged |

## J. Skills / plugins / marketplace

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Skills read from `.grok/` + `.claude/` | 🟡 | **Down-ticked 2026-08-15 (#519).** Agent Skills are real and production (`SkillTool` registered at `caliban/src/startup/compose.rs:654`), but only from caliban's own roots: `<workspace>/.caliban/skills`, `<config>/caliban/skills`, `<data>/caliban/plugins` (`crates/caliban-skills/src/loader.rs:11-21`), plus plugin skill roots (`compose.rs:632`). **`.claude/skills` is not a root** — zero hits for the path literal `".claude"` in `caliban/` or `crates/` — and neither is the `.agents/skills` open-standard path. Reading a competitor's tree in place, which is the whole point of this Grok row, does not work |
| Marketplace install (`@xai/…`, self-host from git) | 🟡 | **Note corrected 2026-08-15 (#519).** The marketplace is real but **HTTP-only**: a JSON index + `.tar.gz` + sha256 verification (`crates/caliban-plugins/src/marketplace.rs:1-7`), routed through the SSRF-guarded client (#158). **Self-hosting from git does not exist** — the only `PluginSourceProvider` impl is `DirectorySource` (`crates/caliban-plugins/src/discovery.rs:52-61`), with git/HTTP sources named as future work at `:2,11`. There is also **no default/hosted marketplace**: the user must supply `<name>@<url>` (`caliban/src/plugin_cli.rs:188`), or sideload with `--dir`. Namespaced hosted-skill search remains 🔴 |
| One-install bundles (skills+agents+hooks+MCP) | 🟡 | **⚠ resolved 2026-08-15 (#519) — and it resolves against us.** `plugin.json` *declares* `components: {skills, hooks, agents, output_styles, mcp_servers, commands}` (`crates/caliban-plugins/src/manifest.rs`), and all of them are parsed and namespaced — but only **skills** reach the runtime. `PluginManager::skill_roots` is the sole aggregation with a non-test consumer (`crates/caliban-plugins/src/manager.rs:262` → `caliban/src/main.rs:326` → `caliban/src/startup/compose.rs:632`); `hooks_configs`, `mcp_servers`, `agent_roots`, `output_style_roots` are discarded, and `components.commands` has no aggregation function at all. So a four-way pack installs, but three quarters of it is inert. Rating unchanged |

## K. Hooks

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Hooks via plugins + Claude/Cursor compat (`GROK_CLAUDE_HOOKS_ENABLED`) | 🟡 | **Down-ticked 2026-08-15 (#519).** caliban's in-process hook taxonomy is real and dispatches (ADR-0024), but this row is specifically about *plugin-delivered* and *foreign-tree* hooks, and **neither works**: `PluginManager::hooks_configs` (`crates/caliban-plugins/src/manager.rs:285`) has **no non-test consumer** — `load_hooks_config` (`caliban/src/startup/compose.rs:1025-1033`) reads `Settings::hook_config()` only — so a plugin's hooks are parsed and discarded; and there is no `.claude/`/`.cursor/` hook scanner (zero hits for the `".claude"` path literal). Config-file handlers are further limited to `PreToolUse`/`PostToolUse`/`SessionStart` (`crates/caliban-agent-core/src/hooks_router.rs:250-252`) with only `command`/`http` kinds executing (`:344-350`). **Correction retained:** a native `.grok/hooks.json` with a fixed `pre/post-edit`/`pre/post-commit`/`on-error`/`on-complete` event set was a secondary-source claim not confirmed in Grok's Settings Reference — Grok's hooks arrive via plugins and Claude/Cursor compat scanners |

## L. MCP / ACP / CI

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| MCP client (local + remote) | ✅ | rmcp client, stdio + HTTP (ADR-0023) |
| `mcp add/list/remove` + `/mcps` | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** No `caliban mcp` subcommand exists (`CalibanCommand` in `caliban/src/args.rs` has no `Mcp` variant) and therefore no `add`/`list`/`remove` verbs. `/mcp` lists servers with live status glyphs but its per-server actions are toast stubs (`caliban/src/tui/events.rs:1139-1173`). Same evidence as §C |
| ACP agent (`grok agent stdio`, JSON-RPC, being driven) | 🔴 | no ACP/editor-driving surface. Grok wires MCP servers in at `session/new`. **Shared with OpenCode `serve`/ACP + Codex `mcp-server`/`app-server` + Antigravity SDK** — one driveable surface serves all; epic **#503** |
| Headless streaming-json for CI/GitHub Actions | ✅ | `--output-format stream-json`; GitHub Action itself deferred |
| First-party GitHub Action / PR bot | 🔴 | GitHub Actions deferred sub-project ⚠ verify Grok's first-party offering |

---

## Grok-Build-distinctive gaps worth a ticket

Capabilities Grok Build has that caliban lacks and that aren't already tracked
by the Claude Code / OpenCode matrices — the highest-signal candidates if we
chase Grok Build parity specifically:

1. **ACP agent over JSON-RPC** (B/L) — `grok agent stdio`, a concrete protocol
   surface (with MCP wired in at `session/new`) for editors/automation to drive
   caliban. **Overlaps with the OpenCode `serve`/`attach`/ACP row, Codex
   `mcp-server`/`app-server`, and Antigravity's headless SDK** — one surface
   serves every sibling matrix. Tracked as epic **#503**.
2. **LSP integration** (H) — Grok integrates language servers via plugins;
   caliban has none. **Shared with the OpenCode LSP row** — not Grok-specific.
3. **Arena Mode** (F) — parallel competing agent outputs for comparison; no
   caliban analogue.
4. **Hosted marketplace skill *search*** (`grok skill search`) (C/J) — caliban's
   `plugin` marketplace has install/list but not hosted namespaced search.
5. **One-line install script + self-update** (A) — `caliban upgrade` + a
   packaged binary channel (shared with the OpenCode/Codex long-tail).
6. **Foreign-tree ingestion** (D/J/K) — **widened 2026-08-15 (#519).** Grok
   reads the *Claude Code* tree in place: plugin packs, `.claude/skills`,
   `.claude/` hooks (`GROK_CLAUDE_HOOKS_ENABLED`), and Cursor rules. caliban
   reads **none** of it — the path literal `".claude"` appears nowhere in
   `caliban/` or `crates/`; skill roots are `.caliban/`-only
   (`crates/caliban-skills/src/loader.rs:11-21`). Only `CLAUDE.md` itself is
   read, as a memory file.
7. **xAI / Grok provider backend** (G) — wire xAI as a first-class provider if a
   Grok backend is in scope.
8. **Plugin components beyond skills** (J/K) — **new 2026-08-15 (#519).** Four
   of five `PluginManager` aggregations are dead
   (`crates/caliban-plugins/src/manager.rs:270-293`), so a plugin can ship
   hooks, agents, MCP servers, and output styles that silently never load.
   This is a last-mile wiring bug, not a design gap — cheap, and it unblocks
   both §J and §K.
9. **A sub-agent definition file format** (F) — **new 2026-08-15 (#519).**
   caliban has none at any path, so per-agent model/tool/isolation choices
   must be repeated inline on every `AgentTool` call and cannot be shared,
   reviewed, or version-controlled.

The grok-build-0.1 / Grok-4.x hosted models are **out of scope** (n/a) — caliban
is model-agnostic and ships no first-party model.

---

## Refresh process

1. When a caliban feature lands: edit the relevant row(s) in the same PR,
   ticking 🔴 → 🟡 or 🟡 → ✅.
2. When Grok Build ships something new: refresh
   [`capability-inventory.md`](capability-inventory.md) first (re-fetch the
   upstream docs), then propagate any new rows here.
3. Resolve any **⚠** rows against caliban `main` when you touch them. Grok's
   docs at `docs.x.ai/build/*` are directly readable, so re-fetch the primary
   pages on each re-baseline rather than leaning on secondary sources.
4. Bump the **Last refreshed** date at the top.
