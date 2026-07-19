# OpenCode documented-capability inventory

> **Static snapshot — captured 2026-07-18.**
>
> Structured snapshot of **OpenCode**'s documented surface, captured from the
> canonical docs at `https://opencode.ai/docs/*` and the project repo. This is
> the *source* feeding [`parity-gap-matrix.md`](parity-gap-matrix.md). It is
> intentionally a point-in-time capture, not a live mirror.
>
> **Scope note:** OpenCode is a genuine **terminal coding agent** in the same
> category as caliban / Claude Code / Codex — this is a head-to-head parity
> target. It ships as a TUI, a headless server, a web UI, a desktop app, and an
> IDE extension over a **client/server** core.
>
> **⚠ Repo lineage:** the canonical docs (`opencode.ai/docs`) currently point
> to `github.com/anomalyco/opencode` and describe a **Node.js/TypeScript**
> build. This is a change from the *original* Go/Bubble-Tea `opencode-ai/opencode`.
> Pin `opencode.ai/docs` as the durable source; re-verify the exact repo +
> maintainer on the next re-baseline. License was not stated on the pages read
> (commonly MIT — verify).
>
> **Re-baseline cadence:** refresh manually before each parity-prioritization
> review. When refreshing, re-fetch the upstream docs, update the sections
> below, bump the snapshot date in this header, and propagate any new rows into
> `parity-gap-matrix.md` in the same commit.
>
> Conventions: *surfaces* = user-visible primitives; "Config = X" lines name
> the canonical configuration mechanism. Items still carrying upstream
> uncertainty are marked **⚠ verify** (see §14).

## 1. Overview / surfaces

- **What it is:** An open-source AI coding agent for the terminal, built on a **client/server** architecture: a backend server holds sessions; a TUI, web UI, IDE extension, or the CLI attach to it.
- **Key surfaces:** TUI (`opencode` / `opencode tui`), headless HTTP server (`opencode serve`), web interface (`opencode web`), IDE extension, desktop app, `attach` (connect a TUI to a running backend), ACP server (`opencode acp`), SDK/Server/Plugins for embedding. Hosted model gateway: **OpenCode Zen**.
- **Runtime:** Node.js / TypeScript. Install via npm / Bun / pnpm / Yarn, Homebrew, Arch, Chocolatey, Scoop, Mise, Docker.
- **Repo / docs:** `opencode.ai/docs` (canonical); repo `github.com/anomalyco/opencode` (⚠ lineage, see header). Marketing: `opencode.ai`.

## 2. CLI reference (exhaustive by subcommand)

- **`tui`** (default) — start the TUI. Flags: `--continue`/`-c`, `--session`/`-s`, `--fork`, `--prompt`, `--model`/`-m`, `--agent`, `--auto`, `--port`, `--hostname`, `--mdns`, `--mdns-domain`, `--cors`.
- **`run`** — non-interactive execution. Flags: `--command`, `--continue`/`-c`, `--session`/`-s`, `--fork`, `--share`, `--model`/`-m`, `--agent`, `--file`/`-f`, `--format` (`default`|`json`), `--title`, `--attach`, `--password`/`-p`, `--username`/`-u`, `--dir`, `--port`, `--variant`, `--thinking`, `--auto`.
- **`serve`** — headless HTTP server for API access. Flags: `--port`, `--hostname`, `--mdns`, `--mdns-domain`, `--cors`.
- **`web`** — headless server + web interface (same flags as `serve`).
- **`attach`** — connect a TUI to a running backend. Flags: `--dir`, `--continue`/`-c`, `--session`/`-s`, `--fork`, `--password`/`-p`, `--username`/`-u`.
- **`auth`** — `login` (`--provider`/`-p`, `--method`/`-m`), `list`/`ls`, `logout`.
- **`agent`** — `create` (`--path`, `--description`, `--mode`, `--permissions`, `--model`/`-m`), `list`.
- **`models`** — list available models (`--refresh`, `--verbose`).
- **`mcp`** — `add`, `list`/`ls`, `auth`, `logout`, `debug`.
- **`github`** — GitHub agent automation: `install`, `run` (`--event`, `--token`).
- **`pr`** — fetch + checkout a GitHub PR branch.
- **`session`** — `list` (`--max-count`/`-n`, `--format`), `delete`.
- **`stats`** — usage/cost (`--days`, `--tools`, `--models`, `--project`).
- **`export`** — session → JSON (`--sanitize`); **`import`** — from JSON file or share URL.
- **`acp`** — start an Agent Client Protocol server (`--cwd`, `--port`, `--hostname`, `--mdns`, `--mdns-domain`, `--cors`).
- **`plugin`/`plug`** — install plugins (`--global`/`-g`, `--force`/`-f`).
- **`db`** — `path` (`--format` `json`|`tsv`); **`debug`**; **`uninstall`** (`--keep-config`/`-c`, `--keep-data`/`-d`, `--dry-run`, `--force`/`-f`); **`upgrade`** (`--method`/`-m`).
- **Global flags:** `--help`/`-h`, `--version`/`-v`, `--print-logs`, `--log-level`, `--pure`.

## 3. Client/server architecture

- **What it does:** The backend server owns sessions and model calls; front-ends (TUI, web, IDE, CLI) are clients. Multiple clients can attach to one backend; sessions survive client disconnects.
- **Surfaces:** `opencode serve` (HTTP API), `opencode web` (browser UI on the server), `opencode attach` (TUI → running backend), `opencode acp` (Agent Client Protocol server for editor integration), plus an **SDK** and documented **Server** API + **Plugins** for embedding.
- **Config:** `server` key (`port`, `hostname`, `mdns`, `cors`); mDNS discovery flags on the serving commands.

## 4. Interactive TUI

- **What it does:** A rich TUI with Plan mode, undo/redo, sharing, and image input.
- **Plan mode:** toggle via **Tab** (switch between the `build` and `plan` primary agents) to review a strategy before edits run.
- **Undo/redo:** `/undo` reverts changes (backed by the `snapshot` file-tracking system, default on); retry with refined prompts.
- **Sharing:** `/share` generates a shareable conversation link.
- **Image input:** drag-and-drop images into the terminal for context.
- **Config:** theme + keybinds live in a separate `tui.json`/`tui.jsonc` (`theme`, `keybinds`, `switch_agent`, `session_child_first`, `session_parent`).

## 5. Config system

- **What it does:** JSON/JSONC config, **merged** across many sources (not replaced).
- **Files:** `opencode.json` / `opencode.jsonc` (main); `tui.json` / `tui.jsonc` (theme/keybinds).
- **Precedence (later wins):** remote (`.well-known/opencode`) → global (`~/.config/opencode/opencode.json`) → `OPENCODE_CONFIG` → project (`opencode.json`) → `.opencode/` dirs → `OPENCODE_CONFIG_CONTENT` → managed (system dirs) → macOS MDM (`.mobileconfig`).
- **Plural subdirs** under `.opencode/` and the config dir: `agents/`, `commands/`, `plugins/`, `tools/`, `themes/`, `skills/`, `modes/`.
- **Major keys:** `model`, `small_model`, `provider` (+ `disabled_providers`/`enabled_providers`), `agent`, `default_agent`, `subagent_depth` (default 1), `command`, `tools`, `permission`, `instructions` (array of paths/globs), `server`, `shell`, `mcp`, `formatter`, `lsp`, `share` (`manual`|`auto`|`disabled`), `snapshot` (default true), `autoupdate`, `attachment.image`, `compaction`, `watcher.ignore`, `plugin`, `experimental.policies`.
- **Substitution:** `{env:VAR}`, `{file:path}`. **Env vars:** `OPENCODE_CONFIG`, `OPENCODE_CONFIG_DIR`, `OPENCODE_TUI_CONFIG`.

## 6. Permissions

- **What it does:** A `permission` key resolving each action to `allow` / `ask` / `deny`.
- **Structure:** global wildcard (`"*": "ask"`) + per-tool overrides; per-command object syntax with **last-matching-rule-wins** pattern matching (`*` = zero-or-more, `?` = exactly-one), e.g. `bash: { "*": "ask", "git *": "allow", "rm *": "deny" }`.
- **Permission types:** `read`, `edit`, `glob`, `grep`, `bash`, `task`, `skill`, `webfetch`, `websearch`, `external_directory`, `doom_loop`.
- **Defaults:** most `allow`; `doom_loop` + `external_directory` default `ask`; `.env` denied for `read`.
- **Agent-level overrides:** agent config (JSON or markdown frontmatter) overrides global permissions; agent rules take precedence.

## 7. Agents / subagents

- **Primary agents:** the assistants you talk to directly; cycle with Tab / `switch_agent`. Built-ins: **Build** (all tools) and **Plan** (edits + bash default to `ask`).
- **Subagents:** invoked by primary agents (via the **Task** tool) or manually via **`@`-mention**. Built-ins: **General** (full access, parallel work), **Explore** (read-only codebase), **Scout** (read-only external docs/deps).
- **Custom agents:** markdown in `~/.config/opencode/agents/` (global) or `.opencode/agents/` (project), filename = agent id; or JSON in `opencode.json`. Frontmatter/keys: `description` (required), `model`, `mode` (`primary`|`subagent`|`all`), `permission`, `temperature` (0.0–1.0), `prompt` (system-prompt file), `steps` (max agentic iterations).
- **Depth:** `subagent_depth` (default 1); parent/child navigation via `session_child_first` / `session_parent`.

## 8. Model & provider support

- **75+ providers** via the AI SDK + **Models.dev**: Anthropic, OpenAI, Google Vertex, Amazon Bedrock, Azure OpenAI, Groq, xAI (Grok), DeepSeek, Cerebras, Together, OpenRouter, Fireworks, NVIDIA, Moonshot (Kimi), + 50 more. **Local:** Ollama, LM Studio, llama.cpp, any OpenAI-compatible endpoint (`@ai-sdk/openai-compatible`).
- **Auth:** `opencode auth login` (`--provider`, `--method`); env vars (`OPENAI_API_KEY`, `AWS_PROFILE`, …); browser OAuth (OpenAI, GitHub Copilot, GitLab Duo, xAI, DigitalOcean, Snowflake); config `{env:VAR}` injection. ⚠ verify — one docs page referenced a `/connect` flow; the CLI reference is authoritative (`auth login`).
- **Model config:** `model` (e.g. `"anthropic/claude-sonnet-4-5"`), `small_model` (session titles / light tasks), per-provider `baseURL` + `whitelist`/blacklist, per-model context/output token limits. **Routing:** OpenRouter / Vercel Gateway provider-priority ordering.
- **Thinking:** `run --thinking` flag; per-model reasoning otherwise provider-driven.

## 9. Tools

- **Built-in:** `read`, `write`, `edit`, `bash`, `glob`, `grep`, `webfetch`, `websearch`, `task` (subagent), `skill`. Enable/disable via `tools`; gate via `permission`.
- **LSP integration:** `lsp` config wires Language Server Protocol servers so the agent gets diagnostics/symbols (a first-class, distinctive feature).
- **Formatters:** `formatter` config (prettier / custom) auto-formats edited files.
- **Custom tools:** user-defined tools under `.opencode/tools/`.
- **Skills / commands:** Agent Skills (`skills/`) and custom `command`s (`commands/`, markdown templates).

## 10. MCP

- **Config:** `mcp` key (local + remote servers). CLI: `opencode mcp {add,list,auth,logout,debug}`.
- **Being driven:** OpenCode exposes itself for automation via the **HTTP server** (`serve`) and **ACP** (`acp`) + SDK — distinct from an MCP-server mode. ⚠ verify — whether a dedicated MCP-*server* mode exists (vs client-only + ACP/HTTP).

## 11. Sharing / sessions / stats

- **Sharing:** `/share` + `share` config (`manual`/`auto`/`disabled`) → hosted conversation links; `export` (`--sanitize`) / `import` (JSON or share URL).
- **Sessions:** `session list`/`delete`; `--continue`/`--session`/`--fork` across `tui`/`run`/`attach`; SQLite-backed persistence (`db path`).
- **Stats:** `opencode stats` (cost/usage by `--days`/`--tools`/`--models`/`--project`).

## 12. GitHub / GitLab / CI

- **GitHub:** `opencode github install` + `opencode github run --event --token` (GitHub Actions automation); `opencode pr` checks out a PR branch.
- **GitLab:** GitLab Duo integration (OAuth provider).
- **Headless/CI:** `opencode run -f json` / `--format json` for scripting; `--pure` global flag; `serve` for API-driven CI.

## 13. Developer surface / enterprise

- **SDK / Server / Plugins / Ecosystem:** documented developer APIs; plugins load from npm (`plugin`/`plug`, `--global`).
- **Enterprise:** managed config (system dirs + macOS MDM `.mobileconfig`), `experimental.policies` resource-access policies, remote `.well-known/opencode` config.
- **OpenCode Zen:** hosted model gateway/marketplace (default provider for `small_model` / session titles).

---

## Notable / distinctive vs caliban

1. **Client/server core with `attach`** — one backend, many front-ends (TUI/web/IDE/CLI), sessions survive disconnects. Not a single-process REPL.
2. **First-class LSP integration** — the agent consumes Language Server diagnostics/symbols, plus auto-`formatter`s on edits.
3. **ACP server + web UI + SDK** — multiple documented ways to embed/drive OpenCode beyond a terminal.
4. **Models.dev-driven 75+ providers** with OAuth for many, `small_model` split, and provider-priority routing.
5. **Hosted sharing** (`/share` links) + `export`/`import` round-tripping.
6. **`doom_loop` + `external_directory` permission guards** and last-match-wins bash-pattern permissions.
7. **OpenCode Zen** hosted gateway as a default light-model provider.

## Explicit uncertainties to re-verify before the next parity pass

- **(a)** repo/maintainer lineage + license — docs point to `anomalyco/opencode` (Node.js), a change from the Go `opencode-ai/opencode`; license not stated on pages read.
- **(b)** auth entry point — `/connect` (one page) vs `opencode auth login` (CLI reference).
- **(c)** whether a dedicated MCP-*server* mode exists vs client-only + ACP/HTTP (§10).
- **(d)** `small_model` default (a page cited "gpt-4-nano via Zen") — confirm current default.

---

## Source pages (fetched 2026-07-18)

Canonical docs at `https://opencode.ai/docs/<slug>`. Repo: `github.com/anomalyco/opencode` (⚠ lineage). Marketing: `https://opencode.ai/`.

| Page | Slug | Notes |
|---|---|---|
| Intro / overview | `/docs/` | nav + surfaces |
| CLI | `/docs/cli/` | full subcommand + flag list |
| Config | `/docs/config/` | merge model + keys |
| Providers | `/docs/providers/` | 75+ via Models.dev |
| Agents | `/docs/agents/` | primary/subagent model |
| Permissions | `/docs/permissions/` | allow/ask/deny + patterns |
| Rules / instructions | `/docs/rules/` (nav) | `instructions` files |
| Tools / LSP / MCP / Skills | `/docs/{tools,lsp,mcp,skills}/` (nav) | tool surface |
| Share / GitHub / GitLab | `/docs/{share,github,gitlab}/` (nav) | collaboration |
| SDK / Server / Plugins | `/docs/{sdk,server,plugins}/` (nav) | developer surface |
| Enterprise | `/docs/enterprise/` | managed config, policies |
