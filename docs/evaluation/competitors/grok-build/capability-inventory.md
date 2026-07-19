# Grok Build documented-capability inventory

> **Static snapshot — captured 2026-07-19.**
>
> Structured snapshot of **Grok Build**'s documented surface, captured from the
> canonical docs at `https://docs.x.ai/build/*`, the launch note at
> `https://x.ai/news/grok-build-cli`, and the open-sourced repo
> `github.com/xai-org/grok-build`. This is the *source* feeding
> [`parity-gap-matrix.md`](parity-gap-matrix.md). It is intentionally a
> point-in-time capture, not a live mirror.
>
> **Scope note:** Grok Build is a genuine **terminal coding agent** in the same
> category as caliban / Claude Code / Codex / OpenCode — a head-to-head parity
> target. It is xAI's first agentic coding CLI (`grok` binary), a fullscreen,
> mouse-interactive Rust TUI that also runs headless (`-p`) and as an ACP agent
> over JSON-RPC.
>
> **⚠ Fetch caveat:** the canonical `x.ai` / `docs.x.ai` pages 403 automated
> fetches (bot protection), so the detail below was cross-checked from
> secondary write-ups and the open-sourced repo rather than read directly off
> the canonical docs. Rows carrying residual uncertainty are marked **⚠ verify**
> (see §14); re-confirm them against the live docs / repo on the next
> re-baseline.
>
> **Currency markers:** beta opened 2026-05-14 (SuperGrok Heavy), expanded
> 2026-05-25 (SuperGrok + X Premium+); harness + TUI open-sourced 2026-07-15 at
> `github.com/xai-org/grok-build` (**Apache-2.0**, ~99.6% Rust). Default coding
> model at capture: **grok-build-0.1** (256K context); larger **Grok-4.x** (2M
> context) for heavy reasoning. Use these to gauge drift on the next
> re-baseline.
>
> **Re-baseline cadence:** refresh manually before each parity-prioritization
> review. When refreshing, re-fetch the upstream docs, update the sections
> below, bump the snapshot date + currency markers in this header, and propagate
> any new rows into `parity-gap-matrix.md` in the same commit.
>
> Conventions: *surfaces* = user-visible primitives; "Config = X" lines name
> the canonical configuration mechanism. Items still carrying upstream
> uncertainty are marked **⚠ verify** (see §14).

## 1. Overview / surfaces

- **What it is:** xAI's agentic coding CLI (`grok`). It can read files, write code, run shell commands, spawn parallel subagents, and drive git workflows against a local codebase.
- **Key surfaces:** interactive fullscreen TUI (`grok`), headless / non-interactive (`grok -p`), and an **ACP agent over JSON-RPC** (editor/automation integration). Fullscreen, mouse-interactive terminal UI with a native subagent view.
- **Runtime:** Rust (repo listed ~99.6% Rust). Distributed as the `grok` binary via an install script.
- **Repo / docs:** canonical docs `docs.x.ai/build/*`; launch note `x.ai/news/grok-build-cli`; marketing `x.ai/cli`; open-source harness + TUI `github.com/xai-org/grok-build` (Apache-2.0).

## 2. Install & access

- **Install:** `curl -fsSL https://x.ai/cli/install.sh | bash`.
- **Access / pricing:** launched in beta 2026-05-14 for SuperGrok Heavy, expanded 2026-05-25 to all SuperGrok and X Premium+ subscribers. Deeper agentic features gated to the heavy/"SuperHeavy" tier. ⚠ verify — exact tier names/prices drifted across sources.
- **Open source:** agent harness, TUI, CLI shell, and tool layer open-sourced 2026-07-15 (Apache-2.0) at `github.com/xai-org/grok-build`; the hosted **model** remains proprietary.

## 3. CLI reference

- **`grok`** (default) — start the interactive TUI.
- **`grok -p "<prompt>"`** — headless / non-interactive run for scripts, CI, and bots.
- **`grok skill`** — marketplace skills: `search`, `install`, `list`, `remove` (e.g. `grok skill install @xai/postgres-migrations`).
- **`grok mcp`** — MCP servers: `add` (`--command …`), `list`, `remove`.
- **`grok inspect`** — show discovered config sources, instructions, skills, plugins, hooks, and MCP servers for the current directory.
- **Key flags:** `-p` (headless), `--output-format` (`plain` | `json` | `streaming-json`), `--always-approve` (skip interactive approvals), `--no-auto-update` (skip background update checks — recommended in automation).
- **Help:** `grok --help`, `grok <subcommand> --help`.
- ⚠ verify — full subcommand list (auth/login, sessions, config, update) not fully confirmed off canonical docs; treat §3 as the confirmed core, not exhaustive.

## 4. Interactive TUI

- **What it does:** fullscreen, mouse-interactive TUI with a native **subagent view**, diff review, and Plan Mode.
- **Slash commands (observed):** `/mode`, `/model`, `/agents`, `/skills`, `/mcps`, `/tokens`, `/feedback`.
- **Diff review:** once a plan is approved, every change surfaces as a clean diff before it lands.
- **Plan Mode toggle:** **Shift+Tab** cycles until the status bar reads `plan` (see §7).

## 5. Config system

- **What it does:** TOML config, layered global → project.
- **Files:** `~/.grok/config.toml` (global defaults) and `.grok/config.toml` (project-scoped overrides).
- **Keys (observed):** `[ui]` section with `permission_mode = "always-approve" | "ask"`; can point the agent at local inference from config.
- **Claude Code compatibility (distinctive):** Grok auto-reads `CLAUDE.md`, the `.claude/` tree (skills, agents, MCPs, hooks, rules), the **AGENTS.md** family, and Claude Code **marketplaces/plugins** alongside `.grok/` with no extra setup — existing Claude Code / AGENTS.md projects "just work."

## 6. Permissions

- **What it does:** per-tool-call approval gating driven by a permission mode.
- **Modes:** `permission_mode` = `ask` (default — prompt per tool call) or `always-approve` (auto-approve); `grok --always-approve` sets it for a run.
- **Scope:** user-level mode in `~/.grok/config.toml`; project-scoped overrides in `.grok/config.toml`.
- ⚠ verify — whether a finer-grained allow/ask/deny per-tool or per-command rule grammar exists beyond the coarse mode switch.

## 7. Plan Mode

- **What it does:** a read-only planning mode. Toggle with **Shift+Tab** until the status bar reads `plan`; every write tool is blocked **except** a single session plan-file scratchpad — the model can read, search, and edit that one file but cannot touch source.
- **Workflow:** approve the plan, comment on individual steps, or rewrite it entirely before execution begins.

## 8. Agents / subagents

- **Parallel subagents:** larger tasks are delegated to specialized subagents that run **in parallel** (up to **8**), e.g. research / implementation / review concurrently.
- **Worktree isolation (distinctive depth):** deep git-worktree integration — subagents can launch in their own isolated worktrees so parallel edits don't stomp the main branch; supports parallel issue-fixing across worktrees.
- **Arena Mode:** competing agent outputs generated in parallel for comparison. ⚠ verify — exact behavior/UX.
- **Definitions:** custom agents via the `.claude/` `agents/` tree and `.grok/` (Claude Code-compatible); `/agents` in the TUI.

## 9. Model & provider support

- **Default coding model:** **grok-build-0.1**, a purpose-built agentic-coding model with a **256K** context window.
- **Heavy reasoning:** larger **Grok-4.x** (e.g. Grok-4.3/4.5) with a **2M** context window for complex tasks; `/model` swaps at runtime.
- **Benchmark:** ~**70.8% SWE-bench Verified** is the most-cited figure for the underlying model. ⚠ verify — attributed variously to `grok-build-0.1` vs `grok-code-fast-1`; confirm which model + which SWE-bench split on re-baseline.
- **Local inference:** config can point the agent at a local/OpenAI-compatible endpoint. ⚠ verify — exact provider knobs.
- **Auth:** xAI account (SuperGrok / X Premium+). ⚠ verify — API-key vs OAuth login command.

## 10. Tools

- **Built-in:** file read/write/edit, shell command execution, code search, git workflow operations, and subagent spawning.
- **Diff-gated edits:** edits surface as reviewable diffs before applying (see §4).
- ⚠ verify — canonical built-in tool names; no first-class LSP/formatter integration was surfaced (treat as absent until confirmed).

## 11. Skills, plugins & marketplaces

- **Skills:** marketplace-installable and self-hosted; `grok skill {search,install,list,remove}`, namespaced (`@xai/<skill>`); `/skills` in the TUI. Reusable `.grok/skills` (and Claude Code `.claude/` skills) are read directly.
- **Bundles:** skills, agents, hooks, and MCP servers can be **bundled behind one install** — via the marketplace or self-hosted from **any git repo**.
- **Claude Code marketplaces/plugins** are read natively (see §5).

## 12. Hooks

- **Config:** `.grok/hooks.json`.
- **Events (observed):** `pre-edit`, `post-edit`, `pre-commit`, `post-commit`, `on-error`, `on-complete` — run custom logic at lifecycle points (e.g. lint after every edit, trigger tests), useful for enforcing project standards automatically.

## 13. MCP / ACP / headless / CI

- **MCP client:** `grok mcp {add,list,remove}` + `/mcps`; local + remote servers; Claude Code MCP configs read natively.
- **ACP:** runs as an **ACP agent over JSON-RPC** for editor/automation integration (being *driven*).
- **Headless / CI:** `grok -p "…"` with `--output-format {plain,json,streaming-json}`; `streaming-json` emits structured records of files modified, commands run, and results — for scripts, GitHub Actions, and custom tooling. `--no-auto-update` recommended in automation.
- **GitHub:** parallel issue-fixing via worktrees; CI integration through headless streaming-json. ⚠ verify — whether a first-party GitHub Action / PR-review bot exists (vs roll-your-own via headless).

---

## Notable / distinctive vs caliban

1. **Native Claude Code / AGENTS.md compatibility** — reads `CLAUDE.md`, `.claude/` (skills, agents, MCPs, hooks, rules), the AGENTS.md family, and Claude Code marketplaces/plugins with zero conversion. Grok Build treats the Claude Code ecosystem as its own config surface.
2. **8 parallel subagents with per-subagent worktree isolation** + **Arena Mode** competing outputs — parallelism and isolation as headline features, not an afterthought.
3. **ACP agent over JSON-RPC** — a documented protocol surface for being driven by editors/automation.
4. **Marketplace skills + one-install bundles** (`grok skill install @xai/…`) that package skills/agents/hooks/MCP together, self-hostable from any git repo.
5. **Lifecycle hooks via `.grok/hooks.json`** (`pre/post-edit`, `pre/post-commit`, `on-error`, `on-complete`).
6. **Install script + background self-update** (`--no-auto-update` to disable) — a packaged, self-updating binary.
7. **Two-tier model split** — a fast 256K coding model (grok-build-0.1) plus a 2M-context heavy model for reasoning, swappable via `/model`.

## Explicit uncertainties to re-verify before the next parity pass

- **(a)** Canonical docs (`docs.x.ai/build/*`) 403 automated fetch — the whole inventory leans on secondary sources; re-read directly next pass.
- **(b)** Which model earns 70.8% SWE-bench and on which split (`grok-build-0.1` vs `grok-code-fast-1`) (§9).
- **(c)** Permission granularity beyond the coarse `permission_mode` switch (§6).
- **(d)** Full CLI subcommand set — auth/login, session management, `config`, `update` not confirmed off canonical docs (§3).
- **(e)** Arena Mode exact semantics (§8) and whether a first-party GitHub Action exists (§13).
- **(f)** Access-tier names/pricing drifted across sources (§2).
- **(g)** ⚠ *Reported context:* multiple secondary sources tie the 2026-07-15 open-sourcing to a prior report that the CLI had uploaded full git repositories to an xAI-controlled bucket. Not independently verified here; flagged only so a re-baseline checks the current data-handling/telemetry posture, not as a settled fact.

---

## Source pages (referenced 2026-07-19)

Canonical docs at `https://docs.x.ai/build/<slug>` (⚠ 403 to automated fetch — cross-checked from secondary sources). Repo: `github.com/xai-org/grok-build` (Apache-2.0). Launch note: `https://x.ai/news/grok-build-cli`. Marketing: `https://x.ai/cli`.

| Page | URL | Notes |
|---|---|---|
| Overview | `docs.x.ai/build/overview` | surfaces, what-it-is |
| CLI reference | `docs.x.ai/build/cli/reference` | subcommands + flags |
| Skills, plugins & marketplaces | `docs.x.ai/build/features/skills-plugins-marketplaces` | skills/bundles/marketplace |
| Changelog | `x.ai/build/changelog` | release drift |
| Launch note | `x.ai/news/grok-build-cli` | plan mode, subagents, worktree, models |
| Open-source repo | `github.com/xai-org/grok-build` | Rust harness/TUI, Apache-2.0 |
| Marketing | `x.ai/cli` | install, positioning |
