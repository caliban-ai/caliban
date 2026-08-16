# Caliban ↔ Google Antigravity parity gap matrix

> **What this is:** a living checklist of feature parity between caliban
> (this project) and **Google Antigravity** (`antigravity.google`) — Google's
> agent-first IDE platform. Refresh it whenever a major caliban feature lands or
> Antigravity ships a new capability. Use it — alongside the
> [Claude Code](../claude-code/parity-gap-matrix.md),
> [Codex](../codex/parity-gap-matrix.md),
> [OpenCode](../opencode/parity-gap-matrix.md), and
> [Grok Build](../grok-build/parity-gap-matrix.md) matrices — to prioritize the
> next sprint.
>
> **How to use it — read the scope note first.** Antigravity is **not** a pure
> terminal agent like caliban; it's an **IDE platform** with an agent engine, a
> terminal CLI, *and* an **Agent Manager** multi-agent dashboard. So the rows
> split three ways:
> - **Head-to-head** rows (agent engine, CLI, config, permissions, MCP, tools)
>   are real apples-to-apples caliban comparisons.
> - Rows tagged **(orch)** describe Antigravity's **Agent Manager** orchestration
>   surface — that is **[Prospero's](../openclaw/README.md) category**, the
>   orchestration layer *over* caliban, not caliban's. They're tracked here for
>   context; a 🔴 on an (orch) row is not necessarily a caliban gap.
> - Rows tagged **n/a** are Antigravity-platform concepts with no intended
>   caliban analogue (GUI editor chrome, hosted model plane).
>
> When shipping a feature that closes a head-to-head row, tick it 🔴 → 🟡 or
> 🟡 → ✅ in the same PR.
>
> **Companion document:** [`capability-inventory.md`](capability-inventory.md)
> — a structured, dated snapshot of Antigravity's documented surface. That file
> is the *source* this matrix is derived from; refresh both together.

**Legend:** ✅ caliban has an equivalent · 🟡 partial · 🔴 gap · **(orch)** =
Agent-Manager orchestration surface (Prospero's category) · **n/a** =
Antigravity-platform concept with no intended caliban analogue (GUI editor,
hosted model plane). A ✅ means "caliban does the equivalent thing," not
byte-identical.

> **Counting convention (shared across the sibling matrices).** Counts are
> **capability-table rows in the lettered sections** — the *Antigravity-distinctive
> gaps* list is excluded, and **(orch)** rows are counted like any other
> (they are Prospero's remit, but they are still scored rows). A **down-tick**
> is a row whose rating got worse, *including* a combined row split into
> worse-scoring halves; an **up-tick** is the reverse; deleting a duplicate row
> is neither, and **neither is merging two rows that scored one capability**
> (#554). A change that rewrites only the Notes cell without moving the
> rating is a **note-only correction** and is counted separately. This matrix
> has **47** scored rows (A–L): 18 ✅ / 14 🟡 / 11 🔴 / 4 n/a.

**Last refreshed:** 2026-08-16 (**caliban-side re-sweep, #554** — no upstream
re-baselining; the Antigravity inventory snapshot stays 2026-07-27, #505).
#519 swept this file the day before, but predates the findings in #550/#551 and
had left three structural problems in place. **2 down-ticks, 0 up-ticks, 3 rows
removed, 15 note-only corrections** across 18 of the 47 remaining rows; row
count 50 → 47, 23 ✅ / 12 🟡 → 18 ✅ / 14 🟡 (🔴 and n/a unchanged).

- **The duplicates are the headline.** §E was four rows, four ✅, **zero code
  citations**, and two of them — *"Permission / autonomy policy (Deny > Ask >
  Allow rule engine)"* and *"Per-tool / per-command allow-ask-deny rule
  grammar"* — scored one engine twice; they are now **one cited row**.
  §K's *Always-on Rules* and *Reusable skills for pipelines* re-scored §D's
  *Global/Workspace Rules* and *Reusable skills*, and were **removed** in favor
  of §D. Five ✅ across this file rested on three implementations.
- **Two down-ticks.** §E *first-run terminal-command auto-execution policy*
  ✅ → 🟡: the policy is real, but there is **no onboarding or first-run
  chooser at any path** — a new operator gets compiled-in defaults and has to
  go find a flag. §F *per-agent isolated workspace* ✅ → 🟡: `AgentTool`
  advertises `isolation: worktree` unconditionally while `background` defaults
  to false, so the **default call silently gets no isolation**, and the
  `worktree` options object is discarded entirely — filed as **#557**.
- **Stale anchors, the defect #551 found in its own file.** Four #519
  citations no longer point at what they claim: `args.rs:88-95` (→ `:20-25`),
  `compose.rs:1708` (→ `:1718`), `compose.rs:1157-1168` (→ `:1174`/`mode_filter.rs:148`),
  `auto_mode.rs:394` (→ `:395`). Every code citation in the file was re-checked
  against `main` this pass, not just the ones being edited.
- **Where caliban was understated.** §L's MCP row said "stdio + HTTP"; **SSE
  is also constructed in production**, leaving websocket as the only one of
  Antigravity's four transports missing.

Prior refresh 2026-08-15 (**caliban-side scoring sweep, #519** — no
upstream re-baselining; the Antigravity inventory snapshot stays 2026-07-27,
#505). That pass reported re-verifying every ✅ row against `main` at v0.8.0
under the rule now written down in
[`../../README.md`](../../README.md#scoring-rule-for-parity-matrices):
a row is ✅ only when a **production call path from the shipped binary**
reaches it. (Read that claim as scoped to the rows it edited — #554 found §E's
four ✅ still uncited, two of them duplicates, and four of the pass's own
citations already pointing at the wrong lines.) **4 down-ticks, 1 up-tick, 6 note-only corrections** across 11 of
50 rows — the lightest of the four sweeps, because a large share of this matrix
is **(orch)** or **n/a** rows that make no caliban claim. The up-tick is the
one the sweep was told to expect: §D's `AGENTS.md` row was **understated** and
goes to ✅ — this is a calibration problem in both directions, not simply
inflation. Two down-ticks are the "machinery with no caller" pattern #516
found (revert/checkpointing and image ingest, both §H); §F's *"Markdown
agent/persona definitions"* went straight to 🔴, since no agent-definition
loader exists at any path; and §C's combined `/agents`+`/permissions` row is
🟡 because `/agents` is a pure stub. Prior refresh 2026-07-27 (primary-source
refresh — derived from [`capability-inventory.md`](capability-inventory.md)
snapshot 2026-07-27, now read directly off the canonical
`antigravity.google/docs/*` pages; caliban state cross-referenced from the
[Claude Code parity matrix](../claude-code/parity-gap-matrix.md) and the
[Grok Build matrix](../grok-build/parity-gap-matrix.md)).

> **Caveat:** rows tagged **⚠** depend on an Antigravity fact still flagged
> uncertain in the inventory (its uncertainties list — the canonical docs are now
> directly readable at HTTP 200, so the old 403 / secondary-source caveat is
> resolved and remaining items are genuine open questions). The "caliban detail
> inferred from the sibling matrices rather than re-verified against `main`"
> half of this caveat was **retired 2026-08-15 (#519)** — every caliban rating
> in this file is now verified directly against `main`. The ⚠ that were
> caliban-side are resolved in place (§D `AGENTS.md`, §D skills layout); the ⚠
> that remain (§F Antigravity's isolation mechanism, §K its hosted marketplace)
> are genuine *upstream* open questions for the next inventory re-baseline.

---

## A. Install & distribution

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Desktop-app download (macOS/Windows/Linux) | n/a | caliban is a terminal agent, not a GUI app; builds from source via `cargo` |
| Free public-preview access (Google account) | n/a | no hosted account plane; caliban runs against your own provider keys |
| One-line install / self-update channel | 🔴 | No install-script or auto-update channel — rating stands (shared with the Grok Build / OpenCode long-tail); there is still no `update`/`upgrade` verb in `caliban/src/args.rs`. **Note corrected 2026-08-16 (#524):** "caliban builds from source" understated distribution — `cargo install caliban` ships (`.github/workflows/publish.yml`, `[[bin]] name = "caliban"` in `caliban/Cargo.toml`), it is just neither an install script nor a self-updater |

## B. Surfaces & architecture

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| GUI Editor view (tab-completion, inline commands) | n/a | caliban is terminal-first; GUI editor chrome is out of scope |
| Terminal CLI / TUI agent | ✅ | caliban ships an interactive TUI + headless `-p` |
| Agent Manager (spawn/observe many parallel agents) **(orch)** | 🟡 | caliban runs parallel subagents under `caliband`; a fleet **dashboard** to observe/manage many top-level agents is **Prospero's** job, not caliban's |
| Built-in browser agent (Chrome extension) | 🔴 | caliban has no browser-driving/verification surface |

## C. CLI / headless

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Terminal agentic loop (plan → edit → run → verify) | ✅ | caliban's core loop |
| Headless / non-interactive run w/ structured output | ✅ | `-p` + `--output-format json/stream-json` + `--bare` (ADR-0025); Antigravity now has an explicit headless SDK + Pydantic structured output (inventory §14) |
| Headless programmatic agent framework (SDK) | 🟡 | caliban ships headless `-p` + JSON/stream output + the `caliband` daemon (ADR-0025); no pip-installable embeddable agent-framework SDK. Antigravity's `google-antigravity` SDK is a first-party head-to-head surface (custom Python tools, lifecycle hooks, sub-agents, Pydantic output; inventory §14) |
| Runtime slash commands (`/agents`, `/permissions`) | 🟡 | **Down-ticked 2026-08-15 (#519); note corrected 2026-08-16 (#554).** `/permissions` is a genuine overlay — registered at `caliban/src/tui/slash/perms.rs:31`, dispatched at `caliban/src/tui/events.rs:585`, handled at `:1292+`, with a status chip (`caliban/src/tui/render.rs:599-602`) — so that half is ✅; it is scored once, in **§E, *Runtime autonomy switch***, and this row's 🟡 is caused **only** by `/agents`. The #519 note mis-described the keys: **Tab cycles the overlay's View/Edit/Audit tabs**, `BackTab` (Shift+Tab) is what cycles the permission *mode*, and `d` deletes by rule origin (session rules from the runtime store, file rules via `caliban_settings::delete_rule_at`, managed scope read-only). **`/agents` is a pure stub**: it ignores its arguments and prints "full sub-agent fleet overlay arrives with the Sub-agent isolation spec … use `caliban agents list` from a shell for now" (`caliban/src/tui/slash/config.rs:184`). It does not even list agents. The shell CLI it points at is real (`caliban/src/args.rs:688` → `caliban/src/subcommands.rs:56`) |

## D. Config system

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Global Rules (`~/.gemini/GEMINI.md`) | ✅ | **Citation added 2026-08-16 (#554)** — the row was uncited. User-scope memory/instructions: the global tier is read on every run by `caliban_memory::load` (`crates/caliban-memory/src/loader.rs:72` ← `caliban/src/startup/compose.rs:1718`) from `MemoryConfig::global_path`, and is budgeted alongside the project tier (`loader.rs:364-368`). CLAUDE.md ancestry, ADR-0036. Scored once here — §K's *Always-on Rules* row was a restatement and was removed this pass |
| Workspace Rules (project-scoped) | ✅ | **Citation added 2026-08-16 (#554)** — the row was uncited. Project scope is real on both halves: settings at `<workspace>/.caliban/settings.json` (`Scope::Project`, `crates/caliban-settings/src/scope.rs:17-28`) and the project memory tier via the ancestor walk (`crates/caliban-memory/src/project_walk.rs:42` → `loader.rs:132`). ADR-0026/0036 |
| `AGENTS.md` project context file | ✅ | **Up-ticked 2026-08-15 (#519); the ⚠ is resolved — this row was *understated*.** `AGENTS.md` is a first-class live instruction source on the same ancestor walk as CLAUDE.md: `ANCESTRY_FILENAMES = [".caliban.md", "CLAUDE.md", "AGENTS.md"]` (`crates/caliban-memory/src/project_walk.rs:42`), consumed on every run by `build_project_tier` (`crates/caliban-memory/src/loader.rs:132` ← `loader.rs:109` ← `caliban/src/startup/compose.rs:1718`; **anchors corrected 2026-08-16 (#554)** — the #519 pair had drifted, `compose.rs:1708` is now a comment), with closer-dir-wins precedence, inode dedupe, gitignore excludes and `@`-import resolution (ADR-0036). `/init` additionally imports it (`crates/caliban-memory/src/init_import.rs:21`) — a separate path, not the only one |
| Layered global → workspace config | ✅ | **Citation added 2026-08-16 (#554)** — the row was uncited. Five scopes merge per-key — managed/user/project/local/CLI (`crates/caliban-settings/src/scope.rs:17-28`) — loaded in production by `caliban_settings::load_settings` (`caliban/src/startup/compose.rs:679`). ADR-0026. Caveat, not a down-tick: settings load **once** per run. `SettingsWatcher::watch` (`crates/caliban-settings/src/watcher.rs`) has no caller outside its own `#[tokio::test]` at `:149`, so there is no live reload — Antigravity's row claims layering, not hot-reload, so ✅ stands; the reload gap is scored on the Pi matrix (§D *Live config reload*, down-ticked in #550) |
| Reusable skills (`skills.md`) | ✅ | **⚠ resolved 2026-08-15 (#519); re-verified 2026-08-16 (#554).** Agent Skills are real and production — roots resolved at `caliban/src/startup/compose.rs:631` (`crates/caliban-skills/src/loader.rs:11-21`: `<workspace>/.caliban/skills`, `<config>/caliban/skills`, `<data>/caliban/plugins`) and `SkillTool` registered at `compose.rs:654`, gated off by `--no-skills`/`--bare`. Layout is `SKILL.md`-per-directory, not a single `skills.md`; that divergence is cosmetic. Note only **one** skill ships built in (`auto-memory`, `crates/caliban-skills/src/builtins/auto_memory.md`). **This is the single scored home for Agent Skills** — §K's *Reusable skills for pipelines* row scored the same code a second time and was removed this pass |

## E. Permissions / autonomy

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Unified permission engine: Deny > Ask > Allow over a per-tool / per-command rule grammar | ✅ | **Two rows merged into one 2026-08-16 (#554); citations added — both were uncited.** The old *"Permission / autonomy policy (Deny > Ask > Allow rule engine)"* and *"Per-tool / per-command allow-ask-deny rule grammar"* rows scored **one** engine twice; the inventory (§6) likewise describes the precedence and the grammar in a single bullet. Verified real and production: `Action::{Allow,Deny,Ask}` with rules evaluated in order (`crates/caliban-agent-core/src/permissions.rs:190-200`), built-in defaults that Allow read-only tools and Ask on `Bash`/`Write`/`Edit` with an `Ask` catch-all (`:209-232`), six permission modes incl. plan/auto/bypass (`crates/caliban-agent-core/src/permission_mode.rs:17-35`, ADR-0029), config rules layered across all five settings scopes plus a live `RuntimeRuleStore` that takes precedence (`permissions.rs:424,447`; wired at `caliban/src/startup/compose.rs:1158-1161`, and in the worker at `caliban/src/worker.rs:738-743`). Grammar (ADR-0045) is `Tool(<glob>)` / `Tool:<glob>` with `*`, `?`, `**`, `~glob`, workspace-normalized paths, and dotted-key MCP arg accessors (`crates/caliban-agent-core/src/permissions_matcher.rs:1-3,44-53`) — so `mcp__github__create_issue(repo=anthropic/*)` and `mcp__*` both match. **One real narrowing vs Antigravity, which the old note had backwards:** caliban matches by **glob only — there is no regex path**, where Antigravity matches `command` by prefix *or regex*; and caliban keys on tool names rather than Antigravity's six action categories. Equivalent in kind, so ✅ stands. Its launch-era Secure/Review-driven/Agent-driven/Custom presets appear superseded by the rule engine |
| First-run terminal-command auto-execution policy | 🟡 | **Down-ticked from ✅ 2026-08-16 (#554).** The policy is real (see the row above) but the **first-run** half does not exist: there is no onboarding, setup wizard, or first-launch policy chooser at any path — the only `first run` in the tree is a memory-seed comment (`crates/caliban-memory/src/loader.rs:34`), and no `caliban setup`/`init`-style policy prompt is registered in `caliban/src/args.rs`. A new operator gets the compiled-in defaults (`permissions.rs:209-232`, `Bash` → `Ask`) and must reach for `--permission-mode`, `settings.json`, `caliban perms`, or `/permissions` to change them. Safe by default, but nothing *asks* — so the setup-time choice Antigravity ships is absent. Scored here so the merge above doesn't silently drop this half |
| Runtime autonomy switch (`/permissions`) | ✅ | **Citation added 2026-08-16 (#554)** — the row was uncited. Registered at `caliban/src/tui/slash/perms.rs:31`, dispatched at `caliban/src/tui/events.rs:585`, handled at `:1292+`: `BackTab` cycles the permission mode, `Tab` moves across View/Edit/Audit tabs, `d` deletes the rule under the cursor (session rules from the `RuntimeRuleStore`, file rules via `caliban_settings::delete_rule_at`, managed scope read-only), with the active mode shown as a status chip (`caliban/src/tui/render.rs:599-602`). Mode also cycles from the main view via Shift+Tab (`events.rs:673` → `:424`). Same overlay as the `/permissions` half of §C's *Runtime slash commands* row — that row's 🟡 comes from `/agents` alone, not from this |

## F. Agents / subagents

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Parallel agents across tasks/workspaces **(orch)** | 🟡 | caliban runs parallel *subagents* (parallel-subagent probe); many *top-level* agents on separate tasks is Prospero's fan-out |
| Per-agent isolated workspace | 🟡 | **Down-ticked from ✅ 2026-08-16 (#554).** #519 documented the sharp edges accurately and then rated the row on the one path that works; re-reading the tool schema shows the capability is **advertised unconditionally and delivered on one branch of two**. Real on the **background** path: `isolation: worktree` → `caliban/src/startup/compose.rs:959-962` → `crates/caliban-supervisor/src/server.rs:465-479` → `WorktreeManager` (ADR-0037/0052). But the `AgentTool` schema offers `isolation: "none"\|"worktree"` with no mention of `background` (`crates/caliban-tools-builtin/src/agent/agent_tool.rs:201-204`), while `background` defaults to **false** (`:67-70`) — so the documented default call, `isolation: "worktree"` without `background: true`, takes the foreground factory, which never reads `input.isolation` and never changes cwd (`compose.rs:884-927`). The model is told it got an isolated workspace and silently did not. `caliban agents spawn` likewise hardcodes it false (`caliban/src/agents_cli.rs:474`), and the whole `worktree` options object (`WorktreeOptions{base_ref, sparse_paths, symlink_directories}`) is parsed and discarded — `worktree_options` has **no consumer outside its own parse test** (`crates/caliban-tools-builtin/tests/agent_tool.rs:252`), so `WorktreeSpec::new` is always called bare (`server.rs:683`). A capability reachable only on the non-default branch, that fails silently rather than erroring on the default one, is a 🟡. Filed as **#557**. ⚠ Antigravity's own isolation mechanism (worktree vs checkout) remains unconfirmed |
| Cross-surface agent (editor + terminal + browser) | 🟡 | caliban drives editor+terminal; **no browser** surface (see B) |
| Markdown agent/persona definitions | 🔴 | **Down-ticked from ✅ 2026-08-15 (#519).** Both halves were wrong. There is **no `.caliban/agents/*.md` loader** — no agent-definition discovery at any path in any Rust file, and `SpawnSpec.frontmatter_path` (`crates/caliban-supervisor/src/proto.rs:95`) is hardcoded `None` at every production construction site (`caliban/src/startup/compose.rs:954`, `caliban/src/agents_cli.rs:323,465`, `caliban/src/tui/events.rs:1008`, `caliban/src/worker.rs:1075`). Sub-agents are configured only by the inline `AgentTool` JSON input, so a persona cannot be named, shared, or version-controlled. And `/agents` is a stub (see §C) |
| Comment-on-work to steer a running agent **(orch)** | 🔴 | no Google-Docs-style commentable work-product stream (Prospero-adjacent) |

## G. Models & providers

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| First-party hosted model (Gemini 3.1 Pro / current roster) | n/a | caliban is model-agnostic; no first-party model |
| Google / Gemini provider | ✅ | **Note re-anchored to code 2026-08-15 (#519)** — it previously cited a sibling matrix rather than `main`. `ProviderKind::Google` (`caliban/src/args.rs:20-25` — **anchor corrected 2026-08-16 (#554)**; `:88-95` is `provider_name`, not the enum) is constructed by `build_google` (`caliban/src/startup/compose.rs:289`) and has a router arm (`caliban/src/router.rs`), with `apiKeyHelper` refresh wired (`compose.rs:301-310`). Google is one of exactly **four** providers the binary can construct (Anthropic/OpenAI/Ollama/Google) — Bedrock and Vertex are *not* linked into `caliban` |
| Multi-model choice in one session (Gemini 3.1 Pro / Claude Sonnet 4.6 / Opus 4.6 / GPT-OSS-120b) | ✅ | **Citation added 2026-08-16 (#554)** — the row was uncited. Provider-agnostic; `/model <id>` swaps the active model mid-session and bare `/model` lists the known set (`caliban/src/tui/slash/model.rs`, behavior pinned by `:236-268`) |
| Fast/heavy split (Gemini 3.1 Pro ↔ 3.6/3.5 Flash) | ✅ | **Note corrected 2026-08-15 (#519) — real, but doubly gated.** `RequestPurpose` is stamped in production (`FastClassifier` at `crates/caliban-agent-core/src/auto_mode.rs:395`, `MainLoop` at `stream/mod.rs:1103`, `Summarization` at `compact.rs:611`), but purpose→route resolution lives **inside the router** (`crates/caliban-model-router/src/config.rs:363-477`), which loads only when a `caliban.toml` with a `[router]` table is discovered (`caliban/src/router.rs:47-50`). The classifier is further gated on `permission_mode = auto` — built at `caliban/src/startup/compose.rs:1174` and handed to `ModeFilter` (`:1180-1184`), which consults it on the `PermissionMode::Auto` arm alone (`crates/caliban-agent-core/src/mode_filter.rs:148`). **Anchor corrected 2026-08-16 (#554)**: the #519 range `compose.rs:1157-1168` pointed at the runtime-rule store, not the gate. With no `caliban.toml` the `purpose` field is inert and one model serves everything. A configured production path, so ✅ stands |
| Local / OpenAI-compatible inference | ✅ | Ollama + LM Studio probed — Antigravity documents no local/BYO-endpoint path (none documented) |

## H. Tools

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| read/write/edit/shell/search | ✅ | full built-in tool set present |
| Diff-gated edits + revert | 🟡 | **Down-ticked 2026-08-15 (#519).** Edits are **gated** — `Write`/`Edit` default to `Ask` (`crates/caliban-agent-core/src/permissions.rs:209-232`) behind the real 4-button Ask modal (ADR-0027) — but not **diff**-gated: the modal shows a truncated `input_summary` (`caliban/src/tui/ask.rs:42`), and **no diff library exists in the workspace** (`similar`/`diffy`/`imara` appear in no `Cargo.toml`). **Revert does not work at all**: `CheckpointHook` is never constructed by the binary, and `App::with_checkpoint_store` (`caliban/src/tui/app.rs:573`) is `#[allow(dead_code, reason = "wired by main.rs once full /rewind action plumbing lands")]` with zero callers, so `/rewind` always renders "(checkpointing not enabled for this session)" (`caliban/src/tui/overlay.rs:826`). `caliban-checkpoint` is complete and unit-tested (ADR-0028) — machinery, not a shipped path. **Re-verified 2026-08-16 (#554):** the only `CheckpointHook::new` call sites in the tree are `crates/caliban-checkpoint`'s own `tests/plan_mode_marker.rs:23`, `tests/disabled_env.rs:30` and `src/hook.rs:191` (its `#[cfg(test)]` fixture); `App::with_checkpoint_store` still has zero callers. Now tracked by **#549**, filed from the Pi sweep — six scored rows across six matrices rest on this one gap |
| Browser tool (navigate/click/screenshot/record) | 🔴 | no browser automation surface |
| Image input | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** ADR-0039's provider-side `ImageBlock` wire support is real, but **no production path ingests an image**: `resolve_image_attachments` (`caliban/src/tui/attach.rs:218`) is `#[allow(dead_code, reason = "wired into a follow-up TUI input slice")]` with test-only callers (**re-verified 2026-08-16, #554** — the attribute names the gap itself); `paste_image_from_clipboard` (`crates/caliban-images/src/clipboard.rs`) and `parse_drag_drop_escape` (`dnd.rs`) have no callers outside their own modules; the text attach path *skips* image files (`attach.rs:146`); `Read` is text-only; there is no `--image` flag. Material for an IDE-platform comparison, where screenshots are a first-class input |

## I. Plan / verify workflow

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Plan-first with approval before execution | ✅ | **Citation added 2026-08-16 (#554)** — the row was uncited. `/plan` is registered (`caliban/src/tui/slash/existing.rs:20`), `PermissionMode::Plan` restricts the tool set to read-only (`crates/caliban-agent-core/src/permission_mode.rs:24-26`), the mode is reachable by Shift+Tab (`caliban/src/tui/events.rs:673` → `:424`), and the model can enter/leave it itself via the registered `EnterPlanMode`/`ExitPlanMode` tools (`caliban/src/startup/compose.rs:615`, allow-listed by default at `crates/caliban-agent-core/src/permissions.rs:219-220`) |
| Edit/comment the plan before it runs | 🟡 | approve + edit before execution; per-step *commenting* UX 🔴 |
| End-to-end run + browser verification of the change | 🔴 | caliban runs code/tests but cannot self-verify in a browser |

## J. Artifacts & knowledge

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Rich Artifacts (task lists, plans, diagrams, diffs) | 🟡 | caliban surfaces plans + diffs; architecture-diagram / walkthrough artifacts 🔴 |
| Browser recordings / screenshots as work-product | 🔴 | no browser-capture artifact stream (ties to B/H browser gap) |
| Commentable, Google-Docs-style work-product **(orch)** | 🔴 | no shared commentable artifact surface (Prospero-adjacent) |
| Knowledge base / cross-session learning | 🟡 | session context + CLAUDE.md memory; no accumulating learned-knowledge store |

## K. Skills / rules / marketplace

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Customization surfaces (Hooks / Sidecars / Plugins) | 🟡 | **Note narrowed 2026-08-15 (#519).** In-process hooks are real and dispatch (ADR-0024), but *config-file* handlers bind to only three events (`PreToolUse`/`PostToolUse`/`SessionStart`, `crates/caliban-agent-core/src/hooks_router.rs:250-252`) with only `command`/`http` kinds executing (`:344-350`). Plugins install and load, but contribute **skills only** — `skill_roots` is the sole `PluginManager` aggregation with a non-test consumer (`crates/caliban-plugins/src/manager.rs:262` → `caliban/src/main.rs:326`); `hooks_configs`, `mcp_servers`, `agent_roots`, `output_style_roots` are parsed and discarded. Still no direct "Sidecars" analogue (inventory §12) |
| Hosted skills/rules marketplace | 🟡 | **Note corrected 2026-08-15 (#519).** caliban's marketplace is real but **not hosted**: there is no default index URL — `MarketplaceSettings::from_env` reads only `CALIBAN_STRICT_KNOWN_MARKETPLACES` (`crates/caliban-plugins/src/marketplace.rs:108-118`) and the operator must supply `<name>@<url>` (`caliban/src/plugin_cli.rs:188`) or sideload with `--dir`. Transport is an HTTP JSON index + `.tar.gz` + sha256 (ADR-0030, SSRF-guarded per #158); git sources are named as future work (`crates/caliban-plugins/src/discovery.rs:2,11`). Antigravity's hosted marketplace ⚠ verify (may be local-only). **Re-verified 2026-08-16 (#554)**: of the five `PluginManager` aggregations only `skill_roots` is called outside `manager.rs`'s own `#[cfg(test)]` module — matches the treatment #551 gave the Claude Code matrix's plugin-packages row |

> **Two rows removed here 2026-08-16 (#554), not down-ticked.** *Always-on
> Rules (global + workspace)* and *Reusable skills for pipelines (`skills.md`)*
> scored exactly the code already scored in **§D** — *Global Rules* +
> *Workspace Rules*, and *Reusable skills*. Antigravity's own inventory does
> the same thing for the same reason (its §12 back-references its §5 for both
> Rules and skills), which is how the duplication got in. Scored once, in §D;
> what remains here is what §12 adds on top — the customization surfaces and
> the marketplace question. Per the counting convention, deleting a duplicate
> row is neither an up-tick nor a down-tick.

## L. MCP / integrations / CI

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| MCP client (servers + tools) | ✅ | **Note corrected 2026-08-16 (#554) — understated in one direction, overbroad in another.** Transports: rmcp client over **stdio + HTTP + SSE**, all three constructed in production (`crates/caliban-mcp-client/src/client.rs:71,99-100`, parsed at `config.rs:318-329`) — the "stdio + HTTP" in the old note missed SSE. Only websocket, of Antigravity's four (inventory §13), is absent. Scope of the ✅ is **servers + tools**, exactly as the row is titled: MCP **resources** are *not* shipped — `McpResource`/`ResourceMention` are exported from `caliban-mcp-client` (`src/resource.rs:149,233`) but have **zero consumers anywhere in `caliban/src/`**, and no `resources/list` RPC is ever issued, so the `@<server>:<resource>` mention flow its module doc describes does not exist. Same finding #551 down-ticked on the Claude Code matrix (§H *resources*); no row here claims resources, so no rating moves. ADR-0023 |
| Per-project allowed-MCP-tool scoping | ✅ | **Citation added 2026-08-16 (#554)** — the row was uncited. Two production mechanisms compose: servers are declared per project in layered settings (`[mcp_servers.<name>]`, `Scope::Project`/`Local` at `crates/caliban-settings/src/scope.rs:17-28`), and individual MCP tools are gated by the same permission grammar as built-ins, which understands `mcp__<server>__<tool>` names and their arguments (`crates/caliban-agent-core/src/permissions_matcher.rs`, e.g. `mcp__github__create_issue(repo=anthropic/*)` and `mcp__*` — pinned at `:604-615,693-695`). Caveat worth naming: there is **no `allowedTools`-style per-server allowlist key** — the scoping is done by permission rules, not by an MCP-config field |
| Browser extension as an agent tool | 🔴 | no browser-extension integration |
| First-party headless/CI or GitHub Action | 🔴 | GitHub Actions deferred (shared gap); Antigravity now ships a first-party programmatic/headless surface — the SDK + scheduled tasks (inventory §14), a new capability |
| Scheduled / recurring task automation (`/schedule`) | 🔴 | no built-in scheduler; Antigravity's Agent Manager 2.0 runs `/schedule` recurring tasks (inventory §4) |

---

## Antigravity-distinctive gaps worth a ticket

Capabilities Antigravity has that caliban lacks and that aren't already tracked
by the sibling matrices — the highest-signal candidates if we chase Antigravity
parity specifically. (Rows marked **(orch)** are Prospero's remit — note them,
but weigh them against Prospero's roadmap, not caliban's.)

1. **Built-in browser agent + browser-verification** (B/H/I/J) — an agent that
   navigates, screenshots, and **records a real browser** to verify its own
   changes end-to-end. No caliban analogue and no sibling-matrix row; the single
   most distinctive Antigravity capability.
2. **Rich, commentable Artifacts** (J) — architecture diagrams, walkthroughs,
   browser recordings, and Google-Docs-style commenting on the agent's
   work-product. The *sharing/commenting* half is **(orch)** (Prospero), but a
   richer local artifact stream (diagrams/walkthroughs) is caliban-relevant.
3. **Knowledge base / cross-session learning** (J) — an accumulating store of
   useful context and snippets that improves future tasks, beyond per-session
   memory files.
4. **Agent Manager fleet dashboard (orch)** — spawn/observe/steer many parallel
   top-level agents across workspaces. This is **Prospero's** category, the same
   as the OpenClaw comparison; a caliban 🔴 here is expected.
5. **One-line install + self-update** (A) — a packaged, self-updating
   distribution (shared with the Grok Build / OpenCode / Codex long-tail).
6. **Headless programmatic SDK + scheduled tasks** (C/L) — the
   `google-antigravity` Python agent framework (custom tools, lifecycle hooks,
   sub-agents, Pydantic-typed output) plus `/schedule` recurring automation.
   Newly surfaced this pass; caliban has a headless CLI + daemon but no
   embeddable agent-framework SDK or built-in scheduler.

7. **Agent/persona definition files + a real `/agents` surface** (C/F) —
   **new 2026-08-15 (#519).** Antigravity has named, reusable agent personas;
   caliban has **no definition loader at any path** and `/agents` is a stub
   (`caliban/src/tui/slash/config.rs:184`), so every per-agent choice is
   repeated inline on each `AgentTool` call. Shared with the Grok Build and
   OpenCode matrices.
8. **Image input as a first-class channel** (H) — **new 2026-08-15 (#519).**
   Screenshots are a primary input for an agent-first IDE; caliban's ingest
   machinery exists but has no caller (`caliban/src/tui/attach.rs:218` is
   `#[allow(dead_code)]`), so there is no user-reachable way to hand it one.
   Wiring, not design.

The Gemini hosted models and the GUI editor chrome are **out of scope** (n/a) —
caliban is model-agnostic and terminal-first.

---

## Refresh process

1. When a caliban feature lands: edit the relevant row(s) in the same PR,
   ticking 🔴 → 🟡 or 🟡 → ✅.
2. When Antigravity ships something new: refresh
   [`capability-inventory.md`](capability-inventory.md) first (re-fetch the
   upstream docs), then propagate any new rows here.
3. Resolve any **⚠** rows against Antigravity's live docs and caliban `main`
   when you touch them — the canonical `antigravity.google/docs/*` pages are now
   directly readable (HTTP 200), so re-confirm remaining open questions straight
   from the docs.
4. Keep **(orch)** rows in sync with Prospero's own OpenClaw/orchestration
   matrices — don't turn an Agent-Manager gap into a caliban ticket.
5. **Before adding a row, check whether another section already scores that
   implementation** (#554). This matrix inherits the inventory's structure, and
   the inventory back-references itself — its §12 points at its §5 for both
   Rules and skills, and its §6 describes the permission precedence and the
   rule grammar in one bullet. Transcribing those as separate rows is how five
   ✅ here came to rest on three implementations, which inflates the ✅ count
   exactly where the file is used as a prioritization input. When two rows
   would cite the same code, **merge them or make one a cross-reference** —
   neither counts as a rating change.
6. **Re-check the code citations, not just the ratings.** Line anchors go stale
   as the tree moves; #551 and #554 each found anchors pointing at unrelated
   lines in files nobody had touched. Cite a symbol name alongside the line so
   a drifted anchor is recoverable.
7. Bump the **Last refreshed** date at the top.
