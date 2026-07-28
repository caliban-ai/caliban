# Google Antigravity documented-capability inventory

> **Static snapshot — captured 2026-07-27.**
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
> **Fetch status:** the canonical `antigravity.google/docs/*` pages, the Google
> Developers blog launch post, and the `codelabs.developers.google.com`
> walkthroughs are all **directly readable now (HTTP 200)** — the earlier "403 to
> automated fetch / secondary sources only" caveat is resolved. The detail below
> is read straight off the canonical pages, which are now the source of record.
> Rows carrying residual uncertainty are marked **⚠ verify** (see the
> uncertainties list below).
>
> **Currency markers:** launched **2026-11-18** in **free public preview**
> alongside Gemini 3 Pro; cross-platform (macOS / Windows / Linux). Launch-era
> default model was **Gemini 3 Pro**; the current `/docs/models` roster is
> **Gemini 3.6 Flash**, **Gemini 3.5 Flash**, **Gemini 3.1 Pro**, **Claude
> Sonnet 4.6 (thinking)**, **Claude Opus 4.6 (thinking)**, **GPT-OSS-120b**, plus
> **Nano Banana 2** (images). Pricing drifted post-launch — a credit /
> subscription structure and reported price increases landed by **2026-03**
> (user protest reported), and a `/docs/pricing` page now gates models "by plan."
> Use these to gauge drift on the next re-baseline.
>
> **Re-baseline cadence:** refresh manually before each parity-prioritization
> review. When refreshing, re-fetch the upstream docs, update the sections
> below, bump the snapshot date + currency markers in this header, and propagate
> any new rows into `parity-gap-matrix.md` in the same commit.
>
> Conventions: *surfaces* = user-visible primitives; "Config = X" lines name the
> canonical configuration mechanism. **(orch)** marks orchestration-layer surface
> that is Prospero's category, not caliban's. Items still carrying upstream
> uncertainty are marked **⚠ verify** (see the uncertainties list below).

> **Corrections applied this pass (2026-07-27):** now reading the canonical docs
> directly (200, no more 403 caveat); permissions reframed from coarse presets to
> the fine-grained Deny > Ask > Allow rule engine (launch-era preset names appear
> superseded); CLI binary pinned to `agy` (+ install script); model roster
> refreshed (Gemini 3.6/3.5 Flash, 3.1 Pro, Claude Sonnet/Opus 4.6, GPT-OSS-120b,
> Nano Banana 2); added the **Antigravity SDK** (§14, the biggest prior miss),
> Antigravity 2.0 standalone Agent Manager + `/schedule`, Hooks/Sidecars/Plugins,
> the fuller slash-command set, and full MCP transport/config detail;
> uncertainties (a)/(b)/(d) resolved, (c)/(e)/(f)/(g-knowledge-base) still open.

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
- **Binary / install:** the CLI binary is **`agy`**; install via
  `curl -fsSL https://antigravity.google/cli/install.sh | bash` → `~/.local/bin/agy`
  (per `/docs/cli/install`). The CLI overview names "headless" workflows,
  Gemini CLI migration, and conversation export shared between the CLI and
  Antigravity 2.0.
- **Slash commands (documented):** `/agents`, `/codesearch`, `/credits`,
  `/diff`, `/permissions`, `/resume`, `/statusline`, `/title`, `/usage`,
  `/browser`, `/schedule`.
- ⚠ verify — only the exact CLI non-interactive flag (`-p` / `--print`) is still
  unshown (it points to `/docs/cli/reference`, not yet fetched). Headless /
  structured-output operation itself is confirmed via the SDK (§14).

## 4. Editor view & Agent Manager

- **Editor view:** VS Code-style editor with AI tab-completion, inline commands,
  and a synchronous chat/composer — the "be hands-on" surface. Largely **n/a**
  for caliban parity (GUI editor chrome).
- **Agent Manager (orch):** as of **Antigravity 2.0** this is a **standalone
  desktop "command center"** (no longer just a pane inside the IDE) — project
  grouping, multi-workspace, async task management, and **scheduled tasks**. Spin
  up **parallel agents** on different tasks/workspaces, watch each one's plan and
  progress, approve steps, and **leave comments/feedback on any Artifact** (Google
  Docs-style commenting) to steer an agent mid-task. Conversation export is
  shared between the CLI and Antigravity 2.0.
- **Scheduled tasks:** `/schedule` drives recurring / scheduled task automation
  from the Agent Manager. (Full documented slash-command set is in §3.)

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

- **Fine-grained permission engine (not coarse presets):** per `/docs/permissions`,
  Antigravity runs a **unified permission engine** with **Deny > Ask > Allow**
  precedence over **six action categories** — `read_file`, `write_file`,
  `read_url`, `execute_url`, `command` (matched by **prefix or regex**),
  `unsandboxed`, and `mcp` — with wildcard / path / domain targets and
  in-approval scope expansion. This is a genuine per-command / per-tool
  allow-ask-deny rule grammar, not a small set of coarse modes.
- **Terminal Command Auto Execution policy** chosen at first setup — governs how
  much the agent does without asking, expressed through the same rule engine.
- **`/permissions`** adjusts the policy at runtime.
- The launch-era named autonomy presets (**Secure / Review-driven / Agent-driven
  / Custom**) appear **superseded** — the current docs describe the rule engine
  rather than those preset names.

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

- **Current roster (`/docs/models`):** **Gemini 3.6 Flash**, **Gemini 3.5
  Flash**, **Gemini 3.1 Pro**, **Claude Sonnet 4.6 (thinking)**, **Claude Opus
  4.6 (thinking)**, **GPT-OSS-120b**, plus **Nano Banana 2** for image
  generation. Model optionality inside one platform is a stated feature; models
  are gated "by plan."
- **Launch-era note:** the header "Default: Gemini 3 Pro" reflects the launch
  configuration; the current roster above has drifted from the launch trio.
- **Auth:** Google account (preview). ⚠ verify — BYO-API-key path for the
  third-party models; the models page shows **no** documented local /
  OpenAI-compatible / BYO-endpoint path.

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
- **Customization surfaces:** the docs "Customization" section lists **Hooks**,
  **Sidecars**, and **Plugins** alongside MCP / Skills / Rules-Workflows — the
  extension story is broader than just skills + rules.
- ⚠ verify — whether there is a hosted **marketplace** for skills/rules or only
  local files + community rule packs.

## 13. MCP / browser extension / integrations

- **MCP client:** configure MCP **servers** and choose which **MCP tools** are
  allowed **per project** (so global servers aren't blanket-exposed to every
  workspace's agent) — real-time context to local tools, databases, and external
  services. Per `/docs/mcp`, transports are **stdio + Streamable HTTP + SSE +
  websocket**; config lives at **`~/.gemini/config/mcp_config.json`** (global) and
  **`.agents/mcp_config.json`** (workspace), an `mcpServers` object supporting
  `disabled` / `disabledTools`, plus one-click Google Cloud server install.
- **Browser extension:** the Chrome extension is exposed to the agent as a tool
  (screenshots, navigation, recordings) — the browser half of the verify loop.
- **Headless / CI:** Antigravity **does** ship a first-party programmatic /
  headless surface — the **Antigravity SDK** (§14) plus **`/schedule`** scheduled
  tasks. The earlier "no confirmed first-party headless/CI" note is resolved.

## 14. Antigravity SDK (headless Python agent framework)

- **What it is:** `pip install google-antigravity` — a **programmatic Python
  agent framework** described in `/docs/sdk/overview` as a **"headless API that
  decouples agent logic from execution environments."** This is the surface that
  materially changes the "can Antigravity be driven programmatically?" story: it
  **can** be driven headlessly, contradicting the earlier "no confirmed
  first-party headless/CI" reading.
- **Capabilities:** built-in tools (file I/O, code editing, shell); **custom
  Python tools**; MCP servers; reusable **skills**; a **deny-by-default
  declarative permission policy**; **lifecycle hooks** (Inspect / Decide /
  Transform across **9 lifecycle points**); **streaming**; **multimodal** input;
  **sub-agents**; **structured output via Pydantic schemas**; **human-in-the-loop**;
  and token / thinking-trace **observability**.
- **Relationship to the CLI:** the SDK is the headless engine; the `agy` CLI (§3)
  is the interactive terminal front-end. Together they resolve the earlier CLI /
  headless / structured-output uncertainty (only the exact CLI non-interactive
  flag remains unshown).

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
5. **Multi-model optionality in one platform** — Gemini 3.1 Pro, Gemini 3.6/3.5
   Flash, Claude Sonnet 4.6, Claude Opus 4.6, GPT-OSS-120b, and Nano Banana 2
   (images) all selectable.
6. **Fine-grained permission engine** — a Deny > Ask > Allow rule engine over six
   action categories (read_file / write_file / read_url / execute_url / command /
   unsandboxed / mcp) with a first-run "Terminal Command Auto Execution" choice —
   not a small set of coarse presets.
7. **VS Code-lineage GUI + tab-completion / inline commands** — a synchronous
   editor experience (**n/a** for a terminal agent).

## Explicit uncertainties to re-verify before the next parity pass

- **(a)** ~~403 automated fetch~~ **RESOLVED** — all canonical
  `antigravity.google/docs/*` pages, the launch blog, and the Codelabs
  walkthroughs return HTTP 200 and are read directly; the caveat is refuted.
- **(b)** Antigravity **CLI** specifics **RESOLVED** — binary is `agy`, install
  script confirmed, slash-command set documented, and headless / structured
  output confirmed via the SDK (Pydantic). Only the exact CLI non-interactive
  flag (`-p` / `--print`) is still unshown (points to `/docs/cli/reference`) (§3).
- **(c)** Whether per-agent workspaces use **git worktrees** or separate
  checkouts — **STILL OPEN** (docs describe "Projects" combining folders but not
  the git isolation mechanism) (§8).
- **(d)** Permission granularity **RESOLVED/refuted** — a fine-grained
  Deny > Ask > Allow rule engine over six action categories exists (§6).
- **(e)** Config compatibility — **PARTIAL**: global `~/.gemini`, workspace
  `.agents/`, and a Skills docs page exist, but `CLAUDE.md` compat is unconfirmed
  (Antigravity uses `GEMINI.md` + `AGENTS.md`) and the exact `skills.md` layout
  is not pinned (§5, §12).
- **(f)** Pricing / tier structure after the preview — **PARTIAL**: a
  `/docs/pricing` page exists and models are gated "by plan," but exact tier
  names / prices are unpinned (§2).
- **(g)** Knowledge-base storage scope and editability — **STILL OPEN** (§11).
  (MCP transport support **RESOLVED**: stdio + Streamable HTTP + SSE + websocket;
  first-party headless/CI surface **RESOLVED**: SDK + `/schedule` — §13, §14.)

---

## Source pages (referenced 2026-07-27)

Canonical docs at `https://antigravity.google/docs/<slug>` and the Google
Developers blog — all **directly readable (HTTP 200)** this pass and now the
source of record. Launch: 2026-11-18 alongside Gemini 3 Pro.

| Page | URL | Notes |
|---|---|---|
| Product / download | `antigravity.google` | surfaces, download, Antigravity 2.0 |
| Docs home | `antigravity.google/docs/home` | overview, Customization (Hooks/Sidecars/Plugins) |
| Getting Started | `antigravity.google/docs/getting-started` | Editor/Agent Manager, `/schedule` |
| Models | `antigravity.google/docs/models` | model roster, Nano Banana 2 |
| Permissions | `antigravity.google/docs/permissions` | Deny>Ask>Allow rule engine |
| MCP | `antigravity.google/docs/mcp` | transports, config paths |
| CLI overview | `antigravity.google/docs/cli/overview` | headless workflows, Gemini CLI migration |
| CLI install | `antigravity.google/docs/cli/install` | `agy` binary, install script |
| SDK overview | `antigravity.google/docs/sdk/overview` | `google-antigravity` headless Python framework |
| Launch post | `developers.googleblog.com/build-with-google-antigravity-our-new-agentic-development-platform` | surfaces, Artifacts, launch-era models |
| Getting Started codelab | `codelabs.developers.google.com/getting-started-google-antigravity` | Editor/Agent Manager, MCP, browser |
| Browser extension | `chromewebstore.google.com/detail/antigravity-browser-exten/eeijfnjmjelapkebgockoeaadonbchdd` | browser agent tool |
