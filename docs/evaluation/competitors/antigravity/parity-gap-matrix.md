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
> is neither. A change that rewrites only the Notes cell without moving the
> rating is a **note-only correction** and is counted separately. This matrix
> has **50** scored rows.

**Last refreshed:** 2026-08-15 (**caliban-side scoring sweep, #519** — no
upstream re-baselining; the Antigravity inventory snapshot stays 2026-07-27,
#505). Every ✅ row was re-verified against `main` at v0.8.0 under the rule now
written down in [`../../README.md`](../../README.md#scoring-rule-for-parity-matrices):
a row is ✅ only when a **production call path from the shipped binary**
reaches it. **4 down-ticks, 1 up-tick, 6 note-only corrections** across 11 of
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
| Runtime slash commands (`/agents`, `/permissions`) | 🟡 | **Down-ticked 2026-08-15 (#519).** `/permissions` is a genuine overlay — Tab/BackTab cycle the mode, `d` deletes a runtime rule, with a real key handler (`caliban/src/tui/events.rs:585,1292+`) and a status chip (`caliban/src/tui/render.rs:599-602`) — so that half is ✅. **`/agents` is a pure stub**: it ignores its arguments and prints "full sub-agent fleet overlay arrives with the Sub-agent isolation spec … use `caliban agents list` from a shell for now" (`caliban/src/tui/slash/config.rs:184`). It does not even list agents. The shell CLI it points at is real (`caliban/src/args.rs:688` → `caliban/src/subcommands.rs:56`) |

## D. Config system

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Global Rules (`~/.gemini/GEMINI.md`) | ✅ | user-scope memory/instructions (CLAUDE.md ancestry, ADR-0036) |
| Workspace Rules (project-scoped) | ✅ | project-scope settings + memory (ADR-0026/0036) |
| `AGENTS.md` project context file | ✅ | **Up-ticked 2026-08-15 (#519); the ⚠ is resolved — this row was *understated*.** `AGENTS.md` is a first-class live instruction source on the same ancestor walk as CLAUDE.md: `ANCESTRY_FILENAMES = [".caliban.md", "CLAUDE.md", "AGENTS.md"]` (`crates/caliban-memory/src/project_walk.rs:42`), consumed on every run by `build_project_tier` (`crates/caliban-memory/src/loader.rs:135-145` ← `caliban/src/startup/compose.rs:1708`), with closer-dir-wins precedence, inode dedupe, gitignore excludes and `@`-import resolution (ADR-0036). `/init` additionally imports it (`crates/caliban-memory/src/init_import.rs:21`) — a separate path, not the only one |
| Layered global → workspace config | ✅ | layered settings (managed/user/project/local) with per-key merge (ADR-0026) |
| Reusable skills (`skills.md`) | ✅ | **⚠ resolved 2026-08-15 (#519).** Agent Skills are real and production — `SkillTool` registered at `caliban/src/startup/compose.rs:654` (gated off by `--no-skills`/`--bare`), discovered from `<workspace>/.caliban/skills`, `<config>/caliban/skills` and `<data>/caliban/plugins` (`crates/caliban-skills/src/loader.rs:11-21`). Layout is `SKILL.md`-per-directory, not a single `skills.md`; that divergence is cosmetic. Note only **one** skill ships built in (`auto-memory`, `crates/caliban-skills/src/builtins/auto_memory.md`) |

## E. Permissions / autonomy

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Permission / autonomy policy (Deny > Ask > Allow rule engine) | ✅ | permission modes incl. ask + plan + bypass (ADR-0029); Antigravity's launch-era Secure/Review-driven/Agent-driven/Custom presets appear superseded by its rule engine |
| First-run terminal-command auto-execution policy | ✅ | permission modes + Bash allow/ask/deny rules (ADR-0029); parity with Antigravity's per-command/regex/wildcard rule engine |
| Runtime autonomy switch (`/permissions`) | ✅ | runtime mode switching |
| Per-tool / per-command allow-ask-deny rule grammar | ✅ | rule grammar (ADR-0029/0045); Antigravity has an equivalent per-command/regex/wildcard rule engine over six action categories (inventory §6) |

## F. Agents / subagents

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Parallel agents across tasks/workspaces **(orch)** | 🟡 | caliban runs parallel *subagents* (parallel-subagent probe); many *top-level* agents on separate tasks is Prospero's fan-out |
| Per-agent isolated workspace | ✅ | **Note corrected 2026-08-15 (#519).** Real on the **background** path: `isolation: worktree` → `caliban/src/startup/compose.rs:959-962` → `crates/caliban-supervisor/src/server.rs:465-479` → `WorktreeManager` (ADR-0037/0052). Two sharp edges: on the **foreground** path the factory never reads `input.isolation` and never changes cwd (`compose.rs:884-927`), so the flag is a silent no-op there, and `caliban agents spawn` hardcodes it false (`caliban/src/agents_cli.rs:474`); `WorktreeOptions{base_ref, sparse_paths, symlink_directories}` are accepted by the tool schema and dropped before `WorktreeSpec::new` (`server.rs:683`). Reachable in production, so ✅ stands. ⚠ Antigravity's own isolation mechanism (worktree vs checkout) remains unconfirmed |
| Cross-surface agent (editor + terminal + browser) | 🟡 | caliban drives editor+terminal; **no browser** surface (see B) |
| Markdown agent/persona definitions | 🔴 | **Down-ticked from ✅ 2026-08-15 (#519).** Both halves were wrong. There is **no `.caliban/agents/*.md` loader** — no agent-definition discovery at any path in any Rust file, and `SpawnSpec.frontmatter_path` (`crates/caliban-supervisor/src/proto.rs:95`) is hardcoded `None` at every production construction site (`caliban/src/startup/compose.rs:954`, `caliban/src/agents_cli.rs:323,465`, `caliban/src/tui/events.rs:1008`, `caliban/src/worker.rs:1075`). Sub-agents are configured only by the inline `AgentTool` JSON input, so a persona cannot be named, shared, or version-controlled. And `/agents` is a stub (see §C) |
| Comment-on-work to steer a running agent **(orch)** | 🔴 | no Google-Docs-style commentable work-product stream (Prospero-adjacent) |

## G. Models & providers

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| First-party hosted model (Gemini 3.1 Pro / current roster) | n/a | caliban is model-agnostic; no first-party model |
| Google / Gemini provider | ✅ | **Note re-anchored to code 2026-08-15 (#519)** — it previously cited a sibling matrix rather than `main`. `ProviderKind::Google` (`caliban/src/args.rs:88-95`) is constructed by `build_google` (`caliban/src/startup/compose.rs:287`) and has a router arm (`caliban/src/router.rs`), with `apiKeyHelper` refresh wired (`compose.rs:294-310`). Google is one of exactly **four** providers the binary can construct (Anthropic/OpenAI/Ollama/Google) — Bedrock and Vertex are *not* linked into `caliban` |
| Multi-model choice in one session (Gemini 3.1 Pro / Claude Sonnet 4.6 / Opus 4.6 / GPT-OSS-120b) | ✅ | provider-agnostic; `/model` runtime swap |
| Fast/heavy split (Gemini 3.1 Pro ↔ 3.6/3.5 Flash) | ✅ | **Note corrected 2026-08-15 (#519) — real, but doubly gated.** `RequestPurpose` is stamped in production (`FastClassifier` at `crates/caliban-agent-core/src/auto_mode.rs:394`, `MainLoop` at `stream/mod.rs:1103`, `Summarization` at `compact.rs:611`), but purpose→route resolution lives **inside the router** (`crates/caliban-model-router/src/config.rs:363-477`), which loads only when a `caliban.toml` with a `[router]` table is discovered (`caliban/src/router.rs:47-50`). The classifier is further gated on `permission_mode = auto` (`caliban/src/startup/compose.rs:1157-1168`). With no `caliban.toml` the `purpose` field is inert and one model serves everything. A configured production path, so ✅ stands |
| Local / OpenAI-compatible inference | ✅ | Ollama + LM Studio probed — Antigravity documents no local/BYO-endpoint path (none documented) |

## H. Tools

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| read/write/edit/shell/search | ✅ | full built-in tool set present |
| Diff-gated edits + revert | 🟡 | **Down-ticked 2026-08-15 (#519).** Edits are **gated** — `Write`/`Edit` default to `Ask` (`crates/caliban-agent-core/src/permissions.rs:209-232`) behind the real 4-button Ask modal (ADR-0027) — but not **diff**-gated: the modal shows a truncated `input_summary` (`caliban/src/tui/ask.rs:42`), and **no diff library exists in the workspace** (`similar`/`diffy`/`imara` appear in no `Cargo.toml`). **Revert does not work at all**: `CheckpointHook` is never constructed by the binary, and `App::with_checkpoint_store` (`caliban/src/tui/app.rs:573`) is `#[allow(dead_code, reason = "wired by main.rs once full /rewind action plumbing lands")]` with zero callers, so `/rewind` always renders "(checkpointing not enabled for this session)" (`caliban/src/tui/overlay.rs:826`). `caliban-checkpoint` is complete and unit-tested (ADR-0028) — machinery, not a shipped path |
| Browser tool (navigate/click/screenshot/record) | 🔴 | no browser automation surface |
| Image input | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** ADR-0039's provider-side `ImageBlock` wire support is real, but **no production path ingests an image**: `resolve_image_attachments` (`caliban/src/tui/attach.rs:218`) is `#[allow(dead_code)]` with test-only callers; `paste_image_from_clipboard` (`crates/caliban-images/src/clipboard.rs`) and `parse_drag_drop_escape` (`dnd.rs`) have no callers outside their own modules; the text attach path *skips* image files (`attach.rs:146`); `Read` is text-only; there is no `--image` flag. Material for an IDE-platform comparison, where screenshots are a first-class input |

## I. Plan / verify workflow

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Plan-first with approval before execution | ✅ | `/plan` + plan permission mode + Shift+Tab cycle |
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
| Always-on Rules (global + workspace) | ✅ | memory files + settings (ADR-0026/0036) |
| Reusable skills for pipelines (`skills.md`) | ✅ | Agent Skills (Claude Code lineage) |
| Customization surfaces (Hooks / Sidecars / Plugins) | 🟡 | **Note narrowed 2026-08-15 (#519).** In-process hooks are real and dispatch (ADR-0024), but *config-file* handlers bind to only three events (`PreToolUse`/`PostToolUse`/`SessionStart`, `crates/caliban-agent-core/src/hooks_router.rs:250-252`) with only `command`/`http` kinds executing (`:344-350`). Plugins install and load, but contribute **skills only** — `skill_roots` is the sole `PluginManager` aggregation with a non-test consumer (`crates/caliban-plugins/src/manager.rs:262` → `caliban/src/main.rs:326`); `hooks_configs`, `mcp_servers`, `agent_roots`, `output_style_roots` are parsed and discarded. Still no direct "Sidecars" analogue (inventory §12) |
| Hosted skills/rules marketplace | 🟡 | **Note corrected 2026-08-15 (#519).** caliban's marketplace is real but **not hosted**: there is no default index URL — `MarketplaceSettings::from_env` reads only `CALIBAN_STRICT_KNOWN_MARKETPLACES` (`crates/caliban-plugins/src/marketplace.rs:108-118`) and the operator must supply `<name>@<url>` (`caliban/src/plugin_cli.rs:188`) or sideload with `--dir`. Transport is an HTTP JSON index + `.tar.gz` + sha256 (ADR-0030, SSRF-guarded per #158); git sources are named as future work (`crates/caliban-plugins/src/discovery.rs:2,11`). Antigravity's hosted marketplace ⚠ verify (may be local-only) |

## L. MCP / integrations / CI

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| MCP client (servers + tools) | ✅ | rmcp client, stdio + HTTP (ADR-0023); Antigravity supports stdio + Streamable HTTP + SSE + websocket (inventory §13) |
| Per-project allowed-MCP-tool scoping | ✅ | MCP config + permission rules scope tools per project |
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
5. Bump the **Last refreshed** date at the top.
