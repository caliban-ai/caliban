# Grok Build documented-capability inventory

> **Static snapshot — captured 2026-07-27 (primary-source re-baseline).**
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
> mouse-interactive Rust TUI that also runs headless (`grok -p`) and as an ACP
> agent over JSON-RPC (`grok agent stdio`).
>
> **✅ Fetch status (2026-07-27):** the earlier caveat that `docs.x.ai` 403'd
> automated fetches **no longer holds** — the canonical pages (Overview,
> Headless & Scripting, Modes & Commands, Skills/Plugins/Marketplaces, Settings
> Reference, Enterprise) were read **directly** off `docs.x.ai/build/*` this
> pass. This re-baseline therefore *replaces* the secondary-source detail from
> the 2026-07-19 capture with primary-source facts and corrects several rows the
> secondary sources got wrong (see the "Corrections applied" note below).
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
> the canonical configuration mechanism. The few items still carrying upstream
> uncertainty are marked **⚠ verify** (see §14).

## 1. Overview / surfaces

- **What it is:** xAI's agentic coding CLI (`grok`). It can read files, write code, run shell commands, spawn parallel subagents, and drive git workflows against a local codebase.
- **Key surfaces:** interactive fullscreen TUI (`grok`), headless / non-interactive (`grok -p`), and an **ACP agent over JSON-RPC** (`grok agent stdio`, for editor/automation integration). Fullscreen, mouse-interactive terminal UI with a native subagent view.
- **Runtime:** Rust (repo listed ~99.6% Rust). Distributed as the `grok` binary via an install script.
- **Repo / docs:** canonical docs `docs.x.ai/build/*`; launch note `x.ai/news/grok-build-cli`; marketing `x.ai/cli`; open-source harness + TUI `github.com/xai-org/grok-build` (Apache-2.0).

## 2. Install & access

- **Install:** `curl -fsSL https://x.ai/cli/install.sh | bash` (PowerShell variant `x.ai/cli/install.ps1`).
- **Access / pricing:** launched in beta 2026-05-14 for SuperGrok Heavy, expanded 2026-05-25 to all SuperGrok and X Premium+ subscribers. Deeper agentic features gated to the heavy/"SuperHeavy" tier. ⚠ verify — exact tier names/prices drifted across sources.
- **Open source:** agent harness, TUI, CLI shell, and tool layer open-sourced 2026-07-15 (Apache-2.0) at `github.com/xai-org/grok-build`; the hosted **model** remains proprietary.

## 3. CLI reference

- **`grok`** (default) — start the interactive TUI.
- **`grok -p, --single "<prompt>"`** — headless / non-interactive run for scripts, CI, and bots (§13).
- **`grok agent stdio`** — run Grok as an **ACP agent** over JSON-RPC on stdin/stdout (§13).
- **`grok skill`** — marketplace skills: `search`, `install`, `list`, `remove` (e.g. `grok skill install @xai/postgres-migrations`).
- **`grok mcp`** — MCP servers: `add` (`--command …`), `list`, `remove`.
- **`grok inspect`** — show discovered config sources, instructions, skills, plugins, hooks, and MCP servers for the current directory.
- **Headless flags (confirmed off `docs.x.ai/build/cli/headless-scripting`):**
  - `-p, --single <PROMPT>` — send one prompt.
  - `-m, --model <MODEL>` — pick the model for the run.
  - `--output-format <plain|json|streaming-json>` — `plain` text, one final `json` object, or newline-delimited `streaming-json` events.
  - `-s, --session-id <ID>` — create or resume a **named** headless session.
  - `-r, --resume <ID>` — resume an existing session.
  - `-c, --continue` — continue the most recent session in the current directory.
  - `--cwd <PATH>` — set the working directory.
  - `--always-approve` — auto-approve tool executions for the run.
  - `--no-alt-screen` — run inline (no fullscreen TUI takeover).
  - `--no-auto-update` — disable background update checks (recommended in CI).
  - `--sandbox <PROFILE>` — select a sandbox profile for the run (§6b).
- **Sessions** are stored under `~/.grok/sessions`.
- **Help:** `grok --help`, `grok <subcommand> --help`.

## 4. Interactive TUI

- **What it does:** fullscreen, mouse-interactive TUI with a native **subagent view**, diff review, Plan Mode, and a Marketplace tab.
- **Autonomy modes (cycled with `Shift+Tab`):** **Plan**, **Auto**, and **Always-Approve** (see §6).
- **Slash commands:** 50+ documented. Session: `/new`, `/resume`, `/sessions`, `/fork` (branch the session into a peer agent). Context: `/context`, `/compact`, `/rewind` (checkpoint/undo). Model: `/model`, `/effort`. Extensions: `/hooks`, `/plugins`, `/skills`, `/mcps` (one unified extensions modal). Planning/permission: `/plan`, `/view-plan`, `/auto`, `/always-approve`. Background/other: `/loop` (recurring tasks), `/tasks` (background tasks / subagents / scheduled work), `/queue` (pending prompts), `/dashboard` (monitoring), `/imagine` (image generation), `/feedback`. Any user-invocable skill also appears as `/<skill-name>` (qualified forms like `/local:commit` disambiguate).
- **Diff review:** once a plan is approved, every change surfaces as a clean diff before it lands.
- **Plan Mode toggle:** **Shift+Tab** cycles until the status bar reads `plan` (see §7).

## 5. Config system

- **What it does:** TOML config, layered **system → user → project**.
- **Files:**
  - `~/.grok/config.toml` (or `$GROK_HOME/config.toml`) — **user** defaults; carries all sections.
  - `.grok/config.toml` — **project** overrides; only `[mcp_servers]`, `[plugins]`, and `[permission]` are honored here.
  - `/etc/grok/requirements.toml` — **system-level** enterprise policy (MDM / golden image); see §13b.
- **Custom models:** configured in `config.toml` (model id, `base_url`, `api_key`); `grok inspect` reveals discovered config sources. Can point the agent at a local / OpenAI-compatible endpoint via `base_url`.
- **Claude Code compatibility (distinctive):** Grok auto-reads `CLAUDE.md`, the `.claude/` tree (skills, agents, MCPs, hooks, rules), the **AGENTS.md** family, and Claude Code **marketplaces/plugins** alongside `.grok/` with no extra setup — existing Claude Code / AGENTS.md projects "just work." Compat scanners are gated by `GROK_CLAUDE_HOOKS_ENABLED` / `GROK_CURSOR_HOOKS_ENABLED` (both default on).

## 6a. Permissions

> **Corrected 2026-07-27** — the prior "coarse mode switch only" reading was a
> secondary-source artifact. The Settings Reference documents a full
> Claude-Code-class permission rule grammar.

- **Autonomy modes:** `ask` (prompt per tool call), **`auto`** (a classifier auto-approves *safe* tool calls and prompts only for risky ones), and `always-approve` (skip prompts; deny rules + hooks still apply). Cycled with `Shift+Tab` alongside Plan Mode. Internal mode identifiers seen in enterprise policy: `dontAsk`, `acceptEdits`, `bypassPermissions`.
- **Rule grammar (`[permission]` in `config.toml`):**
  - **Compact form:** `allow` / `deny` / `ask` arrays of pattern rules, e.g. `"Bash(git *)"`, `"Read(src**)"`, `"Edit(**/*.rs)"`, `"MCPTool(server__*)"`.
  - **Verbose form:** array of objects with `action` (`allow`|`deny`|`ask`), `tool` (`any`|`bash`|`edit`|`read`|`grep`|`mcp`|`webfetch`), and optional `pattern`.
  - **Evaluation order:** `deny > ask > allow`.
- **Scope:** the `[permission]` section is one of the three sections honored in project-scoped `.grok/config.toml`, so per-project allow/deny/ask rules are first-class.
- **CLI:** `grok --always-approve` sets always-approve for a run; per-tool policy lives in config rules rather than `--allow`/`--deny` flags.

## 6b. Sandboxing

- **Built-in sandbox profiles:** `off`, `workspace`, `read-only`, `strict`, `devbox`.
- **Custom profiles:** defined in `~/.grok/sandbox.toml` (user) or `.grok/sandbox.toml` (project); each specifies `extends` (a built-in parent), `restrict_network` (bool), `read_only` / `read_write` path lists, and `deny` (path/glob list, **kernel-enforced**).
- **Activation:** `[sandbox] profile = "…"` in `config.toml`, the `--sandbox` flag, or the `GROK_SANDBOX` env var.

## 7. Plan Mode

- **What it does:** a read-only planning mode. Toggle with **Shift+Tab** until the status bar reads `plan`; every write tool is blocked **except** a single session plan-file scratchpad — the model can read, search, and edit that one file but cannot touch source. The file-edit gate operates independently of the permission setting.
- **Workflow:** `/plan [description]` to enter, `/view-plan` to reopen; approve the plan, comment on individual steps, or rewrite it entirely before execution begins.

## 8. Agents / subagents

- **Parallel subagents:** larger tasks are delegated to specialized subagents that run **in parallel** (up to **8**), e.g. research / implementation / review concurrently. `/fork` branches the current session into a peer agent; `/tasks` lists background tasks, subagents, and scheduled work.
- **Worktree isolation (distinctive depth):** deep git-worktree integration — subagents can launch in their own isolated worktrees so parallel edits don't stomp the main branch; supports parallel issue-fixing across worktrees.
- **Arena Mode:** competing agent outputs generated in parallel for comparison. ⚠ verify — exact behavior/UX (not surfaced on the canonical pages read this pass).
- **Definitions:** custom agents via the `.claude/` `agents/` tree and `.grok/` (Claude Code-compatible); `/agents` in the TUI.

## 9. Model & provider support

- **Default coding model:** **grok-build-0.1**, a purpose-built agentic-coding model with a **256K** context window.
- **Heavy reasoning:** larger **Grok-4.x** (e.g. Grok-4.3/4.5) with a **2M** context window for complex tasks; `/model` swaps at runtime, `/effort` tunes reasoning effort.
- **Benchmark:** ~**70.8% SWE-bench Verified** is the most-cited figure for the underlying model. ⚠ verify — attributed variously to `grok-build-0.1` vs `grok-code-fast-1`; confirm which model + which SWE-bench split on re-baseline (this is a benchmark claim, not a doc fact).
- **Local / custom inference:** `config.toml` can point the agent at a local or OpenAI-compatible endpoint via `base_url` + `api_key`.
- **Auth:** four documented paths — Enterprise **OIDC** (Entra ID / Okta / Auth0), **API key** (`XAI_API_KEY`, for scripts/CI), **Device Code** (RFC 8628, for headless/SSH), and an **External Auth Provider** token-broker executable. (Resolves the earlier "API-key vs OAuth" uncertainty: both, plus device-code and broker.)

## 10. Tools

- **Built-in tools:** file read/write/edit, shell command execution (`bash`), code search (`grep`), web fetch (`webfetch`), git workflow operations, MCP tool calls, and subagent spawning. (Tool identifiers confirmed off the permission `tool` enum: `bash`, `edit`, `read`, `grep`, `mcp`, `webfetch`.)
- **Diff-gated edits:** edits surface as reviewable diffs before applying (see §4).
- **LSP integration:** **present** — plugins can add **LSP servers** (see §11). *(Correction: the prior "no first-class LSP … treat as absent" note was wrong.)*

## 11. Skills, plugins & marketplaces

- **Skills:** structured folders (markdown instructions + scripts + resources) discovered from `./.grok/skills/`, home paths, plugin dirs, and configured custom paths; user-invocable skills become slash commands. Marketplace-installable and self-hosted: `grok skill {search,install,list,remove}`, namespaced (`@xai/<skill>`); `/skills` in the TUI. Claude Code `.claude/` skills are read directly.
- **Plugins:** extend Grok by adding **skills, agents, hooks, MCP servers, and LSP servers**. Loaded from `./.grok/plugins/`, user home paths, marketplace installs, and CLI-specified locations. A unified extensions modal (`/plugins`, `/hooks`, `/skills`, `/mcps`) manages them.
- **Marketplaces:** a dedicated **Marketplace tab** in the TUI; sources come from the main config plus a known-marketplaces JSON. Bundles (skills+agents+hooks+MCP+LSP) install behind one entry, self-hostable from **any git repo**.
- **Claude Code backward-compat:** Grok auto-recognizes Claude Code marketplaces, plugins, skills, MCPs, and instruction files alongside native `.grok/` resources with no configuration.

## 12. Hooks

- **Via plugins:** plugins can contribute **hooks** (see §11).
- **Via Claude/Cursor compat:** hooks are also scanned from external harnesses — `GROK_CLAUDE_HOOKS_ENABLED` and `GROK_CURSOR_HOOKS_ENABLED` (both default on) gate ingestion of Claude Code / Cursor hooks.
- ⚠ verify — the previously-listed native `.grok/hooks.json` file with a fixed event set (`pre/post-edit`, `pre/post-commit`, `on-error`, `on-complete`) was a **secondary-source claim not found** in the Settings Reference this pass. Treat native-hook config + that specific event list as unconfirmed until re-checked against the docs/repo.

## 13a. MCP / ACP / headless / CI

- **MCP client:** `grok mcp {add,list,remove}` + `/mcps`; local + remote servers; Claude Code MCP configs read natively. Config `[mcp_servers.<name>]`: stdio (`command`, `args`, `env`, `cwd`) or HTTP/remote (`url`, `headers`, `bearer_token_env_var`); common keys `enabled`, `startup_timeout_sec` (default 30), `tool_timeout_sec` (default 6000), per-tool `tool_timeouts`. String fields expand `${VAR}`; headers also expand `{{session_id}}`.
- **ACP (being *driven*):** `grok agent stdio` runs Grok as an **ACP agent over JSON-RPC on stdin/stdout** — the primary integration mode for editors (Zed, Neovim, Emacs) and custom clients. Documented JSON-RPC methods: `initialize`, `authenticate`, `session/new` (declares `cwd` and the `mcpServers` to connect — ACP + MCP wire up in one handshake), `session/prompt` (returns completion metadata), and `session/update` (streams `agent_message_chunk` text). The docs ship a full Node.js client example.
- **Headless / CI:** `grok -p "…"` with `--output-format {plain,json,streaming-json}`; `streaming-json` emits newline-delimited structured events for scripts, GitHub Actions, and custom tooling. Named/resumable via `-s/--session-id`, `-r/--resume`, `-c/--continue`; `--no-auto-update` recommended in automation.
- **GitHub:** parallel issue-fixing via worktrees; CI integration through headless streaming-json. ⚠ verify — whether a *first-party* GitHub Action / PR-review bot exists (vs roll-your-own via headless).

## 13b. Enterprise & deployment

- **Hosting:** **cloud-hosted only** — no self-hosting; inference via `cli-chat-proxy.grok.com`, auth via `auth.x.ai`.
- **System policy (`/etc/grok/requirements.toml`):** locked-down enterprise policy pushed via MDM / golden image — e.g. `disable_api_key_auth` (force IdP login), `force_login_team_uuid` (restrict to a team), `disable_bypass_permissions_mode` (lock always-approve off).
- **Proxy:** honors `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY`; TLS 1.2+ via `rustls`; TLS-inspecting proxies need the CA in the OS trust store.
- **Data handling:** Zero Data Retention (ZDR) enforced at the team level; local history in `~/.grok/`.

---

## Notable / distinctive vs caliban

1. **Native Claude Code / AGENTS.md compatibility** — reads `CLAUDE.md`, `.claude/` (skills, agents, MCPs, hooks, rules), the AGENTS.md family, and Claude Code marketplaces/plugins with zero conversion.
2. **Claude-Code-class permission rule grammar** — `allow`/`deny`/`ask` arrays with `Bash(...)`/`Read(...)`/`Edit(...)`/`MCPTool(...)` patterns (and a verbose object form), `deny > ask > allow` — *plus* an `auto` classifier mode. (This was previously mis-scored as coarser than caliban's; it is at parity.)
3. **Kernel-enforced sandbox profiles** — `off`/`workspace`/`read-only`/`strict`/`devbox` + custom `sandbox.toml` (`extends`, `restrict_network`, `read_only`/`read_write`, `deny`).
4. **8 parallel subagents with per-subagent worktree isolation** + **Arena Mode** competing outputs.
5. **ACP agent over JSON-RPC** (`grok agent stdio`) — a concrete protocol surface for being driven by editors/automation, with MCP servers wired in at `session/new`.
6. **LSP servers via plugins** — language-server integration is a supported plugin extension type.
7. **Marketplace skills + one-install bundles** (`grok skill install @xai/…`) packaging skills/agents/hooks/MCP/LSP, self-hostable from any git repo.
8. **Enterprise auth + policy** — OIDC/API-key/device-code/broker auth and `/etc/grok/requirements.toml` MDM-pushed lockdown.
9. **Install script + background self-update** (`--no-auto-update` to disable).
10. **Two-tier model split** — a fast 256K coding model (grok-build-0.1) plus a 2M-context heavy model, swappable via `/model` (+ `/effort`).

## Corrections applied this pass (2026-07-19 → 2026-07-27)

- **Fetch caveat removed** — `docs.x.ai/build/*` is directly readable; the inventory is now primary-sourced.
- **LSP** — was "absent"; **is present** via plugins (§10/§11).
- **Sandboxing** — was unclear/absent; **five built-in profiles + custom kernel-enforced profiles** (§6b).
- **Permissions** — was "coarse mode switch, finer-grained in caliban"; **full allow/deny/ask rule grammar + `auto` classifier mode** (§6a), at parity with caliban's model.
- **Session management** — added `-s/--session-id`, `-r/--resume`, `-c/--continue`, `--cwd`, `--no-alt-screen`, `~/.grok/sessions` (§3).
- **ACP** — vague → concrete command (`grok agent stdio`) + JSON-RPC method list + `session/new` MCP wiring (§13a).
- **Auth** — resolved: OIDC + API key + device code + external broker (§9).
- **Hooks** — native `.grok/hooks.json` + fixed event list *not confirmed*; hooks come via plugins + Claude/Cursor compat (§12).

## Explicit uncertainties to re-verify before the next parity pass

- **(a)** Which model earns 70.8% SWE-bench and on which split (`grok-build-0.1` vs `grok-code-fast-1`) (§9) — a benchmark claim, not a doc fact.
- **(b)** Arena Mode exact semantics (§8) — not on the canonical pages read this pass.
- **(c)** Whether a *first-party* GitHub Action / PR-review bot exists vs roll-your-own headless (§13a).
- **(d)** Native hook config (`.grok/hooks.json`) + its event set (§12) — unconfirmed; hooks otherwise arrive via plugins / Claude-Cursor compat.
- **(e)** Access-tier names/pricing drifted across sources (§2).
- **(f)** ⚠ *Reported context:* multiple secondary sources tie the 2026-07-15 open-sourcing to a prior report that the CLI had uploaded full git repositories to an xAI-controlled bucket. Not independently verified here; flagged only so a re-baseline checks the current data-handling/telemetry posture, not as a settled fact.

---

## Source pages (read directly 2026-07-27 — all HTTP 200)

Canonical docs at `https://docs.x.ai/build/<slug>`. Repo: `github.com/xai-org/grok-build` (Apache-2.0). Launch note: `https://x.ai/news/grok-build-cli`. Marketing: `https://x.ai/cli`.

| Page | URL | Notes |
|---|---|---|
| Overview | `docs.x.ai/build/overview` | surfaces, run modes, install, custom models |
| Headless & Scripting | `docs.x.ai/build/cli/headless-scripting` | `-p`/`--single`, output formats, session flags, ACP `grok agent stdio` + JSON-RPC example |
| Modes & Commands | `docs.x.ai/build/modes-and-commands` | Plan/Auto/Always-Approve, 50+ slash commands |
| Skills, plugins & marketplaces | `docs.x.ai/build/features/skills-plugins-marketplaces` | skills/plugins (incl. LSP servers)/marketplace, Claude Code compat |
| Settings Reference | `docs.x.ai/build/settings/reference` | `[permission]` rule grammar, `[sandbox]` profiles, `[mcp_servers]`, config scope |
| Enterprise | `docs.x.ai/build/enterprise` | hosting, auth (OIDC/API-key/device-code/broker), `/etc/grok/requirements.toml`, ZDR, proxy |
| Launch note | `x.ai/news/grok-build-cli` | plan mode, subagents, worktree, models |
| Open-source repo | `github.com/xai-org/grok-build` | Rust harness/TUI, Apache-2.0 |
