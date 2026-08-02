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

**Last refreshed:** 2026-07-27 (primary-source refresh — derived from
[`capability-inventory.md`](capability-inventory.md) snapshot 2026-07-27, now read
directly off the canonical `antigravity.google/docs/*` pages; caliban state
cross-referenced from the [Claude Code parity
matrix](../claude-code/parity-gap-matrix.md) and the
[Grok Build matrix](../grok-build/parity-gap-matrix.md)).

> **Caveat:** rows tagged **⚠** depend on an Antigravity fact still flagged
> uncertain in the inventory (its uncertainties list — the canonical docs are now
> directly readable at HTTP 200, so the old 403 / secondary-source caveat is
> resolved and remaining items are genuine open questions) or on a caliban detail
> inferred from the sibling matrices rather than re-verified against `main`.

---

## A. Install & distribution

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Desktop-app download (macOS/Windows/Linux) | n/a | caliban is a terminal agent, not a GUI app; builds from source via `cargo` |
| Free public-preview access (Google account) | n/a | no hosted account plane; caliban runs against your own provider keys |
| One-line install / self-update channel | 🔴 | caliban builds from source; no install-script or auto-update channel yet (shared with the Grok Build / OpenCode long-tail) |

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
| Runtime slash commands (`/agents`, `/permissions`) | ✅ | caliban `/agents`, `/permissions`-equivalent modes (ADR-0029/0045) |

## D. Config system

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| Global Rules (`~/.gemini/GEMINI.md`) | ✅ | user-scope memory/instructions (CLAUDE.md ancestry, ADR-0036) |
| Workspace Rules (project-scoped) | ✅ | project-scope settings + memory (ADR-0026/0036) |
| `AGENTS.md` project context file | 🟡 | CLAUDE.md is first-class; `AGENTS.md` ingestion ⚠ verify against `main` |
| Layered global → workspace config | ✅ | layered settings (managed/user/project/local) with per-key merge (ADR-0026) |
| Reusable skills (`skills.md`) | ✅ | Agent Skills supported (Claude Code lineage) ⚠ exact `skills.md` layout differs |

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
| Per-agent isolated workspace | ✅ | `isolation: worktree` (ADR-0037) ⚠ Antigravity's isolation mechanism (worktree vs checkout) unconfirmed |
| Cross-surface agent (editor + terminal + browser) | 🟡 | caliban drives editor+terminal; **no browser** surface (see B) |
| Markdown agent/persona definitions | ✅ | `.caliban/agents/*.md` frontmatter + `/agents` |
| Comment-on-work to steer a running agent **(orch)** | 🔴 | no Google-Docs-style commentable work-product stream (Prospero-adjacent) |

## G. Models & providers

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| First-party hosted model (Gemini 3.1 Pro / current roster) | n/a | caliban is model-agnostic; no first-party model |
| Google / Gemini provider | ✅ | Google is a wired provider (per Grok Build matrix G) |
| Multi-model choice in one session (Gemini 3.1 Pro / Claude Sonnet 4.6 / Opus 4.6 / GPT-OSS-120b) | ✅ | provider-agnostic; `/model` runtime swap |
| Fast/heavy split (Gemini 3.1 Pro ↔ 3.6/3.5 Flash) | ✅ | purpose-keyed routing + `FastClassifier` (ADR-0022); router v2 (ADR-0038) |
| Local / OpenAI-compatible inference | ✅ | Ollama + LM Studio probed — Antigravity documents no local/BYO-endpoint path (none documented) |

## H. Tools

| Capability (Antigravity) | Caliban | Notes |
|---|---|---|
| read/write/edit/shell/search | ✅ | full built-in tool set present |
| Diff-gated edits + revert | ✅ | edit review + auto-checkpoint + `/rewind` (ADR-0028) |
| Browser tool (navigate/click/screenshot/record) | 🔴 | no browser automation surface |
| Image input | ✅ | `caliban-images` (ADR-0039) |

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
| Customization surfaces (Hooks / Sidecars / Plugins) | 🟡 | caliban has hooks + `caliban plugin` (ADR-0030); no direct "Sidecars" analogue to Antigravity's customization set (inventory §12) |
| Hosted skills/rules marketplace | 🟡 | `caliban plugin` marketplace (ADR-0030); Antigravity's hosted marketplace ⚠ verify (may be local-only) |

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
