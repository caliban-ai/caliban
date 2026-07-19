# OpenAI Codex CLI documented-capability inventory

> **Static snapshot — captured 2026-07-18.**
>
> Structured snapshot of the **OpenAI Codex CLI**'s documented surface,
> captured from the canonical docs at `https://developers.openai.com/codex/*`
> (which 308-redirect to the content host `https://learn.chatgpt.com/docs/*`)
> and the `openai/codex` GitHub repo. This is the *source* feeding
> [`parity-gap-matrix.md`](parity-gap-matrix.md). It is intentionally a
> point-in-time capture, not a live mirror.
>
> **Scope note:** this inventory covers the **terminal CLI** (`codex` /
> `@openai/codex`, the Rust binary) — *not* the deprecated 2021 Codex model
> API. Codex's docs are unified under "ChatGPT Learn" and describe one product
> across four surfaces (CLI, IDE extension, ChatGPT desktop app, Codex cloud).
> Where a capability is app/cloud-only rather than in the CLI, it is flagged.
>
> **Currency marker:** latest stable release at capture was `rust-v0.144.6`
> (2026-07-18); current model family is **GPT-5.6**. Use these to gauge drift
> on the next re-baseline.
>
> **Re-baseline cadence:** refresh manually before each parity-prioritization
> review. When refreshing, re-fetch the upstream docs, update the sections
> below, bump the snapshot date + currency marker in this header, and propagate
> any new rows into `parity-gap-matrix.md` in the same commit.
>
> Conventions: *surfaces* = user-visible primitives; "Config = X" lines name
> the canonical configuration mechanism. Items still carrying upstream
> uncertainty are marked **⚠ verify** (see §14).

## 1. Overview / surfaces

- **What it does:** A Rust-based agentic coding engine accessible from a terminal CLI, an IDE extension, the ChatGPT desktop app, and Codex cloud. The four surfaces share config, AGENTS.md instructions, MCP servers, and can hand work off to each other (local ↔ cloud continuum).
- **Key surfaces:** Terminal CLI (`codex`), IDE extension (VS Code / Cursor / Windsurf; JetBrains referenced), ChatGPT desktop app, Codex cloud (`chatgpt.com/codex`), GitHub integration (`@codex` PR review + `openai/codex-action`).
- **Repo:** `github.com/openai/codex` (Rust). In-repo `docs/*.md` are now thin stubs that link out to `developers.openai.com`; do not rely on them for detail.
- **Config:** `~/.codex/config.toml` (`$CODEX_HOME/config.toml`); project `.codex/config.toml`; enterprise `requirements.toml`.

## 2. Install methods

- **Shell (macOS/Linux):** `curl -fsSL https://chatgpt.com/codex/install.sh | sh`
- **PowerShell (Windows):** `irm https://chatgpt.com/codex/install.ps1 | iex`
- **npm:** `npm install -g @openai/codex`
- **Homebrew:** `brew install --cask codex`
- **Direct binary** from GitHub Releases.
- **Platforms:** prebuilt binaries for macOS (arm64 + x86_64) and Linux (x86_64 + arm64). Windows supported, with sandboxing via native Windows Sandbox or WSL2 (see §6).

## 3. CLI reference

- **What it does:** `codex` opens the interactive TUI; a family of subcommands cover non-interactive runs, session management, MCP, cloud, plugins, and diagnostics.
- **Subcommands:** `codex` (TUI), `codex exec`/`e` (non-interactive), `codex resume` (continue a session), `codex fork` (branch a session), `codex apply`/`a` (apply diffs to the working tree), `codex review` (non-interactive code review), `codex mcp` (manage MCP servers), `codex mcp-server` (run Codex *as* an MCP server), `codex login` / `codex logout`, `codex cloud` / `codex cloud-tasks`, `codex archive` / `codex unarchive` / `codex delete` (session mgmt), `codex plugin` + `codex plugin marketplace`, `codex sandbox` (run a command under a sandbox policy), `codex execpolicy` (evaluate rule files), `codex features` (feature flags), `codex completion` (shell completions), `codex doctor` (diagnostics), `codex update`, `codex remote-control`, `codex app` (launch desktop app), `codex app-server` (experimental local server).
- **Global flags:** `--model`/`-m <str>`, `--config`/`-c KEY=VALUE` (repeatable inline override), `--profile`/`-p <name>`, `--cd`/`-C <path>`, `--sandbox`/`-s <read-only|workspace-write|danger-full-access>`, `--ask-for-approval <untrusted|on-request|never>`, `--full-auto` (deprecated → `--sandbox workspace-write`), `--dangerously-bypass-approvals-and-sandbox` (alias `--yolo`), `--image`/`-i <path[,path...]>`, `--add-dir <path>`, `--search` (live web search), `--oss` + `--local-provider <lmstudio|ollama>`, `--strict-config`, `--enable <feature>` / `--disable <feature>`, `--no-alt-screen`, `--remote` + `--remote-auth-token-env <VAR>`.
- **`exec` flags:** `--json` (aka `--experimental-json`; NDJSON event stream), `--output-schema <path>` (JSON-Schema-constrained final output), `--output-last-message`/`-o <path>`, `--color <always|never|auto>`, `--ephemeral` (don't persist the session), `--ignore-rules`, `--ignore-user-config`, `--skip-git-repo-check`.
- **`resume`/`fork` flags:** `--last` (most recent, skip picker), `--all` (include sessions outside cwd), `--include-non-interactive`.

## 4. Interactive TUI

- **What it does:** A composer-driven TUI; `/` opens the command list, `@` does file mention/search.
- **Slash commands (CLI-relevant):** `/init` (scaffold AGENTS.md), `/model`, `/permissions` (formerly `/approvals`; presets **Auto (default) / Read Only / Full Access**), `/compact`, `/review`, `/mcp`, `/status` (session id, context usage, rate limits), `/fork`, `/reasoning` (adjust effort), `/feedback`, `/quit`. ⚠ verify — the consolidated commands page also lists app/cloud-surface commands (`/pet`, `/goal`, `/plan`, `/worktree`, `/cloud`, `/local`, `/project`, `/task`, `/side`, `/ide-context`, `/personality`, `/fast`, `/memories`) that may not all exist in the pure CLI.
- **Keybindings:** `Ctrl+C` / `/quit` exit; `Esc` interrupt; `Ctrl+J` newline; `Ctrl+O` (or `Alt+R`) copy latest completed response as raw output (persist via `tui.raw_output_mode = true`); `@` file mentions; images via `--image` or paste.
- **Config:** TUI behavior under `[tui]` (`notifications`, `notification_method` = `auto|osc9|bel`, `notification_condition`, `animations`, `alternate_screen`, `show_tooltips`).

## 5. Config system

- **What it does:** Single TOML configuration system with named profiles and an enterprise policy layer.
- **Format/path:** TOML at `~/.codex/config.toml` (`$CODEX_HOME/config.toml`); project-scoped `.codex/config.toml`; managed/enterprise `requirements.toml` (e.g. `allow_managed_hooks_only`). A published JSON schema drives editor autocomplete (Even Better TOML).
- **Profiles:** named profiles applied via `--profile`; profiles **cannot** override provider/auth/telemetry keys.
- **Major keys:** `model`, `model_provider` (default `openai`), `model_reasoning_effort` (`minimal|low|medium|high|xhigh`; Responses-API only), `model_reasoning_summary`, `model_verbosity`, `show_raw_agent_reasoning`/`hide_agent_reasoning`, `approval_policy`, `sandbox_mode`, `[sandbox_workspace_write]` (`writable_roots`, `network_access`, `exclude_slash_tmp`, `exclude_tmpdir_env_var`), `[model_providers.<id>]`, `[mcp_servers.<id>]`, `notify` (array), `[hooks]`, `[otel]`, `[history]` (`persistence`, `max_bytes`; default `~/.codex/history.jsonl`), `[tui]`.
- **Env vars:** `CODEX_HOME` (config dir), `CODEX_API_KEY` / `OPENAI_API_KEY` (auth), `-c key=value` inline overrides.

## 6. Approval modes & sandboxing

- **What it does:** Two **orthogonal** axes — sandbox = the technical boundary; approval policy = when the agent must stop and ask. OS-native enforcement.
- **Sandbox modes:** `read-only` (inspect only), `workspace-write` (default; read anywhere, write within workspace + run local commands), `danger-full-access` (no FS/network boundary).
- **Approval policies:** `untrusted` (approve anything outside a trusted command set), `on-request` (autonomous in sandbox, ask to exceed), `on-failure` (run sandboxed, escalate on failure), `never`. A `granular` table form exists (`sandbox_approval`, `rules`, `mcp_elicitations`, `request_permissions`, `skill_approval`) plus `approvals_reviewer` = `user|auto_review`.
- **OS enforcement:** macOS uses the built-in **Seatbelt** framework (automatic); Linux/WSL2 uses **unprivileged user namespaces via `bubblewrap`** (must be installed); Windows uses native **Windows Sandbox** (PowerShell) or the Linux sandbox under WSL2. Network in `workspace-write` gated by `sandbox_workspace_write.network_access`.
- **Standalone:** `codex sandbox` runs an arbitrary command under a sandbox policy; `codex execpolicy` evaluates rule files.

## 7. MCP (client + server)

- **As MCP client** — `[mcp_servers.<id>]` tables, two transports:
  - **stdio:** `command` (req), `args`, `env`, `env_vars` (forwarded), `cwd`, `experimental_environment` (`remote`).
  - **Streamable HTTP:** `url` (req), `auth` (`oauth`|`chatgpt`), `bearer_token_env_var`, `http_headers`, `env_http_headers`.
  - **Common:** `enabled`, `startup_timeout_sec` (default 10), `tool_timeout_sec` (default 60), `enabled_tools`/`disabled_tools`, `default_tools_approval_mode` (`auto|prompt|writes|approve`).
  - **CLI:** `codex mcp add <name> -- <command>`, `codex mcp list`, `codex mcp login <server>`, `codex mcp --help`; `/mcp` for status.
- **As MCP server:** `codex mcp-server` runs Codex itself as an MCP server so other tools/agents can drive it.

## 8. Model & provider support

- **OpenAI models (default, ChatGPT-auth):** current **GPT-5.6** family — marketed as **Sol** (flagship), **Terra** (balanced), **Luna** (fast/cheap); also 5.5, 5.4, 5.4-mini. Config-example IDs: `gpt-5.6`, `gpt-5.6-terra`, `gpt-5.5`, `gpt-5.4-mini`.
- **Reasoning effort:** `minimal|low|medium (default)|high|xhigh`, plus `max` and an **`ultra`** tier that triggers parallel subagent delegation (see §11). `xhigh` is model-dependent.
- **Third-party / local:** any provider speaking the **Responses API** (current standard) or **Chat Completions API** (legacy for Codex), under `[model_providers.<id>]` with `wire_api` selecting responses vs chat. Built-in provider IDs include `openai`, `ollama`, `lmstudio`; `--oss` / `--local-provider` for local models.

## 9. Memory / project instructions

- **`AGENTS.md`** is Codex's CLAUDE.md analogue and follows the open **AGENTS.md** standard. Global `~/.codex/AGENTS.md`; project repo-root `AGENTS.md` + nested `AGENTS.md` in subdirectories; **files closer to the working directory take precedence** (merged). `/init` scaffolds one.
- **Higher tiers (partly app/cloud):** **Memories** and a **Chronicle** (cross-session learning); a community **Honcho** lifecycle-hook integration for persistent cross-session memory.

## 10. Hooks / skills / plugins / notifications

- **Hooks:** commands at lifecycle events — `PreToolUse`, `PostToolUse`, `SessionStart`, `SessionStop` — in `hooks.json` or inline `[[hooks.PreToolUse]]` tables with a `matcher` regex (e.g. `matcher = "^Bash$"`). Enterprise `allow_managed_hooks_only` gate.
- **Skills:** open agent-skills standard; a skill dir has `SKILL.md` (required) + optional `scripts/`, `references/`, `assets/`. Locations: global `$HOME/.agents/skills`, project `.agents/skills`. `codex build-skills` / `/skills`. Skills can declare MCP deps in `agents/openai.yaml` for auto-wiring.
- **Plugins:** `codex plugin` + `codex plugin marketplace` (`/plugins` in TUI). Plugins bundle skills, connectors, MCP servers, browser extensions, hooks, and scheduled-task templates; distributed via marketplace sources.
- **Notifications:** `notify = ["python3", "/path/notify.py"]` — script receives one JSON arg (`type`, `thread-id`, `turn-id`, `cwd`, `input-messages`, `last-assistant-message`); TUI-native notifications via `[tui]`.

## 11. Sub-agents / parallelism

- **What it does:** Custom subagents as **standalone TOML files** in `~/.codex/agents/` (personal) or `.codex/agents/` (project). ⚠ verify — some third-party guides show Markdown+YAML frontmatter; the canonical page specifies TOML.
- **Fields:** `name`, `description`, `developer_instructions` (required); optional overrides `model`, `model_reasoning_effort`, `sandbox_mode`, `mcp_servers`, `skills.config`, `nickname_candidates`.
- **Execution:** spawned in **parallel** (one agent per point, codebase exploration, multi-step plans), orchestration auto-managed, results consolidated to the main thread. **Enabled by default**; the `ultra` reasoning tier auto-delegates to subagents. Activity surfaces in CLI, desktop app, and IDE.

## 12. Headless / CI / automation

- **`codex exec`** — streams progress to stderr, final message to stdout. Flags: `--json` (JSONL events: `thread.started`, `turn.started/completed`, `item.*`), `--output-schema <path>` (JSON-Schema-constrained output), `--output-last-message`/`-o`, `--ephemeral`, `--skip-git-repo-check`, `--ignore-user-config`, `--ignore-rules`.
- **Piping:** `cat prompt.txt | codex exec -` (stdin sentinel `-`); `npm test 2>&1 | codex exec "summarize failures"`.
- **Resume in scripts:** `codex exec resume --last "<task>"` or `codex exec resume <SESSION_ID>`.
- **Auth for CI:** `CODEX_API_KEY=<key> codex exec ...` (env-key auth is exec-only).
- **GitHub:** official **`openai/codex-action`** is the recommended CI path.

## 13. Observability / cost / telemetry

- **OpenTelemetry:** `[otel]` config — `environment`, `exporter` (`none|otlp-http|otlp-grpc`), `log_user_prompt`. Emits events for conversation starts, API requests, SSE events, WebSocket activity, user prompts, tool decisions, tool results.
- **Session history:** `~/.codex/history.jsonl`, controlled via `[history]` (`persistence`, `max_bytes`).
- **Cost/usage:** `/status` shows context usage + rate limits in-session; billing tied to ChatGPT plan or API-key usage. `codex doctor` produces diagnostic reports.

## 14. Cloud / IDE / long-tail surfaces

- **Codex Cloud** (`chatgpt.com/codex`): parallel tasks in isolated cloud environments with reproducible env config (deps, tools, vars, setup steps, secrets, internet-access toggle). Delegated from CLI (`codex cloud` / `codex cloud-tasks`), IDE, GitHub, Linear, or Slack. GitHub integration does PR code review; `@codex` in PR comments delegates. Review flow: inspect summary + diff, request follow-up, open PR, apply diffs locally.
- **IDE extension** (VS Code Marketplace, publisher `openai`; VS Code / Cursor / Windsurf; JetBrains referenced): local agent mode side-by-side, plus delegate-to-cloud (start local, offload, monitor, preview, apply diffs locally).
- **Auth:** ChatGPT plan sign-in (browser OAuth) for Plus/Pro/Team/Enterprise; device-code flow for headless; API key (`CODEX_API_KEY`/`OPENAI_API_KEY`, `codex login`). Three billing/capability models.

---

## Notable / distinctive vs a Claude-Code-like agent

1. **Orthogonal sandbox × approval model** with OS-native enforcement (macOS Seatbelt, Linux bubblewrap/user-namespaces, Windows Sandbox), plus a standalone `codex sandbox` subcommand.
2. **First-class subagents as TOML files** with per-agent model/sandbox/MCP overrides, auto-parallelized, and an **`ultra` reasoning tier that auto-delegates** to them.
3. **Tight local ↔ cloud continuum**: the same CLI/IDE delegates long jobs to Codex Cloud and pulls diffs back; GitHub `@codex` review + `openai/codex-action`.
4. **`codex mcp-server`** — Codex acts as an MCP *server*, not just a client.
5. **Structured exec output**: `--output-schema` (JSON-Schema-constrained answer) + `--json` NDJSON with a typed event vocabulary (`thread.*`, `turn.*`, `item.*`).
6. **Plugin marketplace** (`codex plugin marketplace`) bundling skills + connectors + MCP + hooks + browser extensions + scheduled tasks.
7. **Open-standards adoption**: AGENTS.md + the open agent-skills format (`SKILL.md`, `.agents/skills`).
8. **Built-in OpenTelemetry export** (`[otel]`) + enterprise `requirements.toml` / managed-hooks policy layer.

## Explicit uncertainties to re-verify before the next parity pass

- **(a)** exact CLI-only slash-command set vs desktop-app commands (§4).
- **(b)** subagent file format — TOML (canonical page) vs MD+YAML (some third-party guides) across versions (§11).
- **(c)** the dedicated authentication page 404'd during capture; auth details (§14) are corroborated from adjacent pages and should be spot-checked.
- **(d)** whether `on-failure` approval is exposed as a CLI flag value or config-only (§6).

---

## Source pages (fetched 2026-07-18)

Canonical entry point `https://developers.openai.com/codex/<slug>` (308-redirects
to the content host `https://learn.chatgpt.com/docs/<slug>`). Repo: `github.com/openai/codex`.

| Page | Canonical slug | Notes |
|---|---|---|
| CLI command reference | `codex/cli/reference` → `developer-commands?surface=cli` | subcommands + flags |
| Config reference | `codex/config-reference` → `config-file/config-reference` | TOML keys |
| Advanced config | — → `config-file/config-advanced` | profiles, providers |
| Sandboxing / approvals | `codex/concepts/sandboxing` → `sandboxing` | Seatbelt / bubblewrap |
| Non-interactive / exec | `codex/noninteractive` → `non-interactive-mode` | `--json`, `--output-schema` |
| MCP | `codex/mcp` → `extend/mcp` | client + `mcp-server` |
| Models | — → `models` | GPT-5.6 family |
| Customization overview | — → `customization/overview` | AGENTS.md / hooks / skills |
| Subagents | `codex/subagents` → `agent-configuration/subagents` | TOML agent files |
| Plugins | — → `plugins` | marketplace |
| Skills | `codex/build-skills` | `SKILL.md` |
| Cloud | `codex/cloud` | cloud tasks |
| IDE | `codex/ide` | VS Code / forks |
| Repo + releases | `github.com/openai/codex` | Rust; `rust-v0.144.6` at capture |

> Repo `docs/*.md` files (config.md, exec.md, sandbox.md, …) are now thin
> stubs that link out to `developers.openai.com`; the canonical detail lives
> at the URLs above, not in-repo.
