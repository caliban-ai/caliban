# Google Antigravity documented-capability inventory

> **Static snapshot — captured 2026-07-20.**
>
> Structured snapshot of **Google Antigravity**'s documented surface, captured
> from the canonical docs at `https://antigravity.google/docs/*`, the launch
> post `developers.googleblog.com/build-with-google-antigravity-...`, and the
> Google Codelabs walkthroughs. This is the *source* feeding
> [`parity-gap-matrix.md`](parity-gap-matrix.md). It is intentionally a
> point-in-time capture, not a live mirror.
>
> **Scope note:** Google Antigravity is **not** a terminal coding agent in the
> narrow sense caliban / Claude Code / Codex / OpenCode / Grok Build are — it is
> an **agent-first IDE platform** (a VS Code-lineage editor from the Windsurf /
> Codeium team) with two headline surfaces: a classic **Editor view** and an
> **Agent Manager** "mission control" that spawns and observes many autonomous
> agents in parallel. Only a *slice* of it is a real apples-to-apples parity
> target for caliban:
> - **Head-to-head with caliban:** the agent *engine* (plan → edit → run →
>   verify), the **Antigravity CLI / terminal agent**, config/rules ingestion,
>   permissions/autonomy, MCP, and tools. These rows are genuine.
> - **Prospero-adjacent (orchestration layer):** the **Agent Manager** — launch
>   / fleet / observe / comment-on-work across many parallel agents — is closer
>   to **Prospero's** category (the orchestration layer over caliban) than to a
>   single terminal agent, the same way [`openclaw/`](../openclaw/README.md) is.
>   Those rows are flagged **(orch)** and are tracked here only for context.
> - **Out of scope (n/a):** the GUI editor chrome (tab-completion, inline
>   command palette) and Google's hosted model plane.
>
> **⚠ Fetch caveat:** the canonical `antigravity.google` docs, the Google
> Developers blog launch post, and the `codelabs.developers.google.com`
> walkthroughs all **403 automated fetch** (bot protection), so the detail below
> was cross-checked from secondary write-ups (tutorials, reviews, comparison
> articles) rather than read directly off the canonical pages. Rows carrying
> residual uncertainty are marked **⚠ verify** (see §14); re-confirm them against
> the live docs on the next re-baseline.
>
> **Currency markers:** launched **2026-11-18** in **free public preview**
> alongside Gemini 3 Pro; cross-platform (macOS / Windows / Linux). Default model
> at launch: **Gemini 3 Pro** (generous preview rate limits, reported to refresh
> every ~5 hours); also **Claude Sonnet 4.5** and **GPT-OSS** selectable, with
> **Gemini 3.5 Flash** added later. Pricing drifted post-launch — a credit /
> subscription structure and reported price increases landed by **2026-03**
> (user protest reported). Use these to gauge drift on the next re-baseline.
>
> **Re-baseline cadence:** refresh manually before each parity-prioritization
> review. When refreshing, re-fetch the upstream docs, update the sections
> below, bump the snapshot date + currency markers in this header, and propagate
> any new rows into `parity-gap-matrix.md` in the same commit.
>
> Conventions: *surfaces* = user-visible primitives; "Config = X" lines name the
> canonical configuration mechanism. **(orch)** marks orchestration-layer surface
> that is Prospero's category, not caliban's. Items still carrying upstream
> uncertainty are marked **⚠ verify** (see §14).

## 1. Overview / surfaces

- **What it is:** Google's **agentic development platform** — an "agent-first"
  IDE where you manage autonomous coding agents that **plan, implement, test,
  and verify** work across the **editor, terminal, and browser**, rather than
  editing text yourself.
- **Key surfaces:**
  - **Editor view** — a state-of-the-art, AI-powered IDE (VS Code lineage) with
    tab completion and inline commands for the synchronous "hands-on" workflow.
  - **Agent Manager** — a dedicated "mission control" dashboard to spawn,
    orchestrate, and observe **multiple agents working asynchronously** across
    different workspaces/tasks **(orch)**.
  - **Antigravity CLI / Terminal agent** — a lightweight, keyboard-centric TUI
    that brings the agentic loop to the terminal (fast interactions, SSH
    sessions).
  - **Browser agent** — drives a real browser via a **Chrome extension** to
    click, navigate, screenshot, and record for verification.
- **Runtime / lineage:** built on the **VS Code** foundation; from the
  **Windsurf / Codeium** team (acquired into Google). Distributed as a desktop
  app for macOS / Windows / Linux.
- **Repo / docs:** canonical docs `antigravity.google/docs/*`; download / product
  `antigravity.google`; launch post `developers.googleblog.com/build-with-google-antigravity-our-new-agentic-development-platform`.

## 2. Install & access

- **Install:** download the desktop app from `antigravity.google` (macOS /
  Windows / Linux); a separate **Chrome browser extension** enables the browser
  agent (Chrome Web Store id `eeijfnjmjelapkebgockoeaadonbchdd`).
- **Access / pricing:** launched **free during public preview** (2026-11-18),
  sign in with a Google account. Preview rate limits reported to refresh on a
  ~5-hour cadence. ⚠ verify — by 2026-03 a credit-based / subscription pricing
  structure had been introduced and prices reportedly rose (user protest
  reported); confirm current tier names/prices on re-baseline.
- **Not open source:** Antigravity (and its models) are proprietary Google
  products; no open-sourced harness.

## 3. Antigravity CLI / terminal agent

- **What it is:** a lightweight, **keyboard-centric terminal UI** that brings the
  core agentic loop (plan → edit → run → verify) to the terminal — aimed at fast
  interactions and remote/SSH sessions.
- **Positioning:** the closest analogue to caliban's own surface; Google has run
  dedicated "Agentic Coding with the Antigravity CLI" material.
- ⚠ verify — exact binary name, subcommand list, headless / non-interactive
  (`-p`-style) flag, and structured-output format are **not confirmed** off the
  canonical docs (they 403'd automated fetch). Treat §3 as "a terminal agent
  exists and runs the agentic loop," not an exhaustive CLI reference.

## 4. Editor view & Agent Manager

- **Editor view:** VS Code-style editor with AI tab-completion, inline commands,
  and a synchronous chat/composer — the "be hands-on" surface. Largely **n/a**
  for caliban parity (GUI editor chrome).
- **Agent Manager (orch):** a multi-agent pane / dashboard. Spin up **parallel
  agents** on different tasks/workspaces, watch each one's plan and progress,
  approve steps, and **leave comments/feedback on any Artifact** (Google
  Docs-style commenting) to steer an agent mid-task.
- **Slash commands (observed):** `/agents` (monitor subagents, inspect detail
  views, handle approvals), `/permissions` (set autonomy level). ⚠ verify — full
  slash-command set.

## 5. Config system

- **Rules (system instructions):** persistent, always-on guidelines the agent
  must honor before planning/generating.
  - **Global Rules** — apply to every workspace; personal/org coding philosophy.
    Stored at **`~/.gemini/GEMINI.md`**.
  - **Workspace Rules** — scoped to the current project.
- **`AGENTS.md`** — project-root context file (the cross-tool `AGENTS.md`
  standard) read at session start to seed project context and personas.
- **`skills.md` / skills** — reusable capability definitions used to build
  autonomous developer pipelines (per the Codelabs "agents.md + skills.md"
  walkthrough). ⚠ verify — exact skill file layout/discovery.
- ⚠ verify — whether a `CLAUDE.md` compatibility path exists (caliban's native
  file); Antigravity's documented context files are `GEMINI.md` + `AGENTS.md`.

## 6. Permissions / autonomy

- **Terminal Command Auto Execution policy** chosen at first setup — governs how
  much the agent does without asking.
- **Autonomy levels (observed):** **Secure**, **Review-driven** (recommended for
  production — agent asks before running terminal commands and before finalizing
  plans), **Agent-driven** (more autonomous), and **Custom**.
- **`/permissions`** switches level at runtime: **request-review**,
  **always-proceed**, or **strict**.
- ⚠ verify — whether a finer-grained per-tool / per-command allow-ask-deny rule
  grammar exists beyond these coarse modes.

## 7. Plan / verify workflow

- **Plan first:** the agent breaks a task into a **detailed implementation plan**
  and (in review-driven mode) **waits for approval** before executing; you can
  comment on or edit the plan before it runs.
- **Verify:** agents don't just write code — they **run** it (terminal) and
  **verify** it (browser: navigate, screenshot, record) end-to-end, then surface
  the evidence as Artifacts (§11).

## 8. Agents / parallel subagents

- **Parallel agents (orch):** the Agent Manager runs **multiple agents
  concurrently** across different workspaces/tasks — the headline "manage a team
  of agents" model.
- **Cross-surface autonomy:** a single agent can write code in the editor, use
  the terminal to launch the app, and use the browser to test the result —
  without synchronous human intervention.
- **Definitions / personas:** specialized personas configured via `AGENTS.md` +
  Rules (§5); monitored/approved via `/agents` (§4). ⚠ verify — whether isolated
  per-agent workspaces map to git worktrees or to separate checkouts.

## 9. Models & providers

- **Default:** **Gemini 3 Pro** (generous preview rate limits).
- **Also selectable:** **Anthropic Claude Sonnet 4.5** and **OpenAI GPT-OSS**;
  **Gemini 3.5 Flash** added post-launch. Model optionality inside one platform
  is a stated feature.
- **Auth:** Google account (preview). ⚠ verify — BYO-API-key path for the
  third-party models, and whether local/OpenAI-compatible endpoints are
  configurable.

## 10. Tools

- **Editor tools:** file read/write/edit, tab-completion, inline commands.
- **Terminal tool:** shell command execution (gated by the autonomy policy, §6).
- **Browser tool:** a **Chrome extension** the agent drives to navigate, click,
  take **screenshots**, and produce **browser recordings** for verification.
- **MCP tools:** external tools/data via MCP servers (§13).
- ⚠ verify — canonical built-in tool names and whether first-class LSP /
  formatter hooks exist.

## 11. Artifacts & knowledge base (distinctive)

- **Artifacts:** as it works, the agent emits **tangible deliverables** — task
  lists, implementation plans, architecture diagrams, screenshots, **browser
  recordings**, code diffs, and walkthroughs — as rich markdown/media. They let
  you **verify the agent's logic at a glance**, and you can **comment on any
  Artifact** (Google Docs-style) to redirect the agent. This "verifiable
  work-product" surface is Antigravity's signature idea.
- **Knowledge base / learning:** Antigravity treats **learning as a core
  primitive** — agents **save useful context and code snippets to a knowledge
  base** to improve future tasks (cross-session memory). ⚠ verify — storage
  scope (per-workspace vs global) and whether it's user-editable.

## 12. Skills & rules

- **Rules** (§5) are the always-on constitution; **skills / `skills.md`** package
  reusable procedures for "autonomous developer pipelines" (per Codelabs).
- ⚠ verify — whether there is a hosted **marketplace** for skills/rules or only
  local files + community rule packs.

## 13. MCP / browser extension / integrations

- **MCP client:** configure MCP **servers** and choose which **MCP tools** are
  allowed **per project** (so global servers aren't blanket-exposed to every
  workspace's agent) — real-time context to local tools, databases, and external
  services.
- **Browser extension:** the Chrome extension is exposed to the agent as a tool
  (screenshots, navigation, recordings) — the browser half of the verify loop.
- **Headless / CI:** ⚠ verify — no confirmed first-party headless/CI or GitHub
  Action surface for Antigravity off the canonical docs (the CLI, §3, is the
  likeliest path); treat as unconfirmed.

---

## Notable / distinctive vs caliban

1. **Agent-first IDE + Agent Manager "mission control"** — a full GUI editor with
   a dedicated dashboard to run and observe **many parallel agents** across
   workspaces. This is a *platform*, broader than caliban; the multi-agent
   orchestration half is **Prospero's** category **(orch)**, not caliban's.
2. **Artifacts as verifiable work-product** — task lists, plans, diagrams,
   screenshots, and **browser recordings** you can **comment on like a Google
   Doc**. caliban surfaces plans and diffs but has no rich, commentable
   Artifact/recording stream.
3. **Built-in browser agent (Chrome extension)** — the agent navigates,
   screenshots, and records a real browser to **verify** its own changes
   end-to-end. caliban has no browser-driving/verification surface.
4. **Knowledge base / learning as a core primitive** — cross-session memory of
   useful context and snippets. caliban has session context + CLAUDE.md memory,
   not an accumulating learned-knowledge store.
5. **Multi-model optionality in one platform** — Gemini 3 Pro default plus Claude
   Sonnet 4.5, GPT-OSS, and Gemini 3.5 Flash selectable.
6. **Autonomy-level presets** — Secure / Review-driven / Agent-driven / Custom
   with a first-run "Terminal Command Auto Execution" choice.
7. **VS Code-lineage GUI + tab-completion / inline commands** — a synchronous
   editor experience (**n/a** for a terminal agent).

## Explicit uncertainties to re-verify before the next parity pass

- **(a)** Canonical `antigravity.google/docs/*`, the Google Developers blog
  launch post, and `codelabs.developers.google.com` all **403 automated fetch** —
  the whole inventory leans on secondary sources; re-read directly next pass.
- **(b)** Antigravity **CLI** specifics — binary name, subcommands, headless
  `-p`-style flag, structured-output format (§3).
- **(c)** Whether per-agent workspaces use **git worktrees** or separate
  checkouts (§8).
- **(d)** Permission granularity beyond the coarse autonomy presets (§6).
- **(e)** Config compatibility — whether `CLAUDE.md` is read, and exact
  `skills.md` layout/discovery (§5, §12).
- **(f)** Pricing / tier structure after the preview (credits, subscription,
  reported increases) (§2).
- **(g)** Knowledge-base storage scope and editability (§11); MCP transport
  support (stdio vs HTTP) (§13); any first-party headless/CI surface (§13).

---

## Source pages (referenced 2026-07-20)

Canonical docs at `https://antigravity.google/docs/<slug>` and the Google
Developers blog (⚠ **403 to automated fetch** — cross-checked from secondary
sources). Launch: 2026-11-18 alongside Gemini 3 Pro.

| Page | URL | Notes |
|---|---|---|
| Product / download | `antigravity.google` | surfaces, download, platforms |
| Docs home | `antigravity.google/docs/home` | overview |
| Agent docs | `antigravity.google/docs/agent` | agent loop, personas |
| Agent modes & settings | `antigravity.google/docs/agent-modes-settings` | autonomy, permissions |
| Launch post | `developers.googleblog.com/build-with-google-antigravity-our-new-agentic-development-platform` | surfaces, Artifacts, models |
| Getting Started codelab | `codelabs.developers.google.com/getting-started-google-antigravity` | Editor/Agent Manager, MCP, browser |
| Pipelines codelab | `codelabs.developers.google.com/autonomous-ai-developer-pipelines-antigravity` | `AGENTS.md` + `skills.md` |
| Browser extension | `chromewebstore.google.com/detail/antigravity-browser-exten/eeijfnjmjelapkebgockoeaadonbchdd` | browser agent tool |
