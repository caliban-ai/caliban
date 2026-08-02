# OpenCode documented-capability inventory

> **Static snapshot — captured 2026-07-27.**
>
> Structured snapshot of **OpenCode**'s documented surface, captured from the
> canonical docs at `https://opencode.ai/docs/*` and the project repo. This is
> the *source* feeding [`parity-gap-matrix.md`](parity-gap-matrix.md). It is
> intentionally a point-in-time capture, not a live mirror.
>
> **Live-docs status:** the canonical `opencode.ai/docs/*` pages are directly
> readable this pass (all HTTP 200), so the earlier "verify against live docs
> later" posture is discharged — the facts below were confirmed against primary
> sources, not left pending. (One slug moved: `opencode.ai/docs/mcp/` now 404s;
> the canonical MCP page lives at a different slug — see §10.)
>
> **Scope note:** OpenCode is a genuine **terminal coding agent** in the same
> category as caliban / Claude Code / Codex — this is a head-to-head parity
> target. It ships as a TUI, a headless server, a web UI, a desktop app, and an
> IDE extension over a **client/server** core.
>
> **Repo lineage (confirmed):** the canonical docs (`opencode.ai/docs`) point
> to `github.com/anomalyco/opencode` (HTTP 200) — **anomalyco = Anomaly
> Innovations, the rebranded SST team** (Dax Raad / Adam Doty). `sst/opencode`
> → `anomalyco/opencode` is the **same actively-maintained project** (a
> **TypeScript/Bun** rewrite), not a lineage fork to re-verify. The earlier
> Go/Bubble-Tea `opencode-ai/opencode` line was the separate, earlier
> Charm-maintained codebase. License is **CONFIRMED MIT**.
>
> **Corrections applied this pass (2026-07-27):** confirmed lineage
> (anomalyco = SST/Anomaly, same project as `sst/opencode`, TypeScript/Bun) and
> **MIT** license — dropped the prior "verify" hedges; added `lsp` + `question`
> to the §6 permission-type list; noted default `opencode` runs a TUI-client +
> server together (§3); added the OpenAPI 3.1 `/doc` spec, basic-auth env vars,
> and typed JS/TS + Go SDKs (§3/§13); added LSP default-off + 30+ built-in
> servers (§9); pinned the `doom_loop` threshold at 3 identical calls (§6);
> resolved uncertainties (a) and (b) as complementary/confirmed. (c) MCP-server
> mode stays *leaning-refuted* (canonical MCP slug moved/404) and (d) has no
> single documented `small_model` default.
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
- **Runtime:** TypeScript / Bun. Install via npm / Bun / pnpm / Yarn, Homebrew, Arch, Chocolatey, Scoop, Mise, Docker.
- **Repo / docs:** `opencode.ai/docs` (canonical); repo `github.com/anomalyco/opencode` (**MIT**; anomalyco = rebranded SST/Anomaly team — same project as `sst/opencode`, see header). Marketing: `opencode.ai`.

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
- **Default `opencode` is *already* client/server.** Per `/docs/server/`: "When you run `opencode` it starts a TUI **and** a server, where the TUI is the client that talks to the server." So the client/server split is not an opt-in `serve` mode — it is the default runtime shape.
- **Surfaces:** `opencode serve` (HTTP API), `opencode web` (browser UI on the server), `opencode attach` (TUI → running backend), `opencode acp` (Agent Client Protocol server for editor integration), plus an **SDK** and documented **Server** API + **Plugins** for embedding.
- **HTTP API:** an **OpenAPI 3.1** spec is served at `/doc` (e.g. `http://localhost:4096/doc`); HTTP **basic auth** is available via `OPENCODE_SERVER_PASSWORD` / `OPENCODE_SERVER_USERNAME` env vars.
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
- **Permission types (13):** `read`, `edit`, `glob`, `grep`, `bash`, `task`, `skill`, `lsp` (running LSP queries), `question` (asking the user), `webfetch`, `websearch`, `external_directory`, `doom_loop`.
- **Defaults:** most `allow`; `doom_loop` + `external_directory` default `ask`; `.env` denied for `read` (OpenCode ships `*.env` / `*.env.*` deny rules). **`doom_loop`** fires specifically "when the same tool call repeats **3 times** with identical input."
- **Agent-level overrides:** agent config (JSON or markdown frontmatter) overrides global permissions; agent rules take precedence.

## 7. Agents / subagents

- **Primary agents:** the assistants you talk to directly; cycle with Tab / `switch_agent`. Built-ins: **Build** (all tools) and **Plan** (edits + bash default to `ask`).
- **Subagents:** invoked by primary agents (via the **Task** tool) or manually via **`@`-mention**. Built-ins: **General** (full access, parallel work), **Explore** (read-only codebase), **Scout** (read-only external docs/deps).
- **Custom agents:** markdown in `~/.config/opencode/agents/` (global) or `.opencode/agents/` (project), filename = agent id; or JSON in `opencode.json`. Frontmatter/keys: `description` (required), `model`, `mode` (`primary`|`subagent`|`all`), `permission`, `temperature` (0.0–1.0), `prompt` (system-prompt file), `steps` (max agentic iterations).
- **Depth:** `subagent_depth` (default 1); parent/child navigation via `session_child_first` / `session_parent`.

## 8. Model & provider support

- **75+ providers** via the AI SDK + **Models.dev**: Anthropic, OpenAI, Google Vertex, Amazon Bedrock, Azure OpenAI, Groq, xAI (Grok), DeepSeek, Cerebras, Together, OpenRouter, Fireworks, NVIDIA, Moonshot (Kimi), + 50 more. **Local:** Ollama, LM Studio, llama.cpp, any OpenAI-compatible endpoint (`@ai-sdk/openai-compatible`).
- **Auth:** `opencode auth login` (`--provider`, `--method`); env vars (`OPENAI_API_KEY`, `AWS_PROFILE`, …); browser OAuth (OpenAI, GitHub Copilot, GitLab Duo, xAI, DigitalOcean, Snowflake); config `{env:VAR}` injection. **`/connect` and `opencode auth login` are complementary, not either/or:** `/connect` is the **in-app / TUI manual API-key entry** path (`/docs/providers/`), while `opencode auth login` is the **CLI** path (`/docs/cli/`). Both are real and documented.
- **Model config:** `model` (e.g. `"anthropic/claude-sonnet-4-5"`), `small_model` (session titles / light tasks), per-provider `baseURL` + `whitelist`/blacklist, per-model context/output token limits. **Routing:** OpenRouter / Vercel Gateway provider-priority ordering.
- **Thinking:** `run --thinking` flag; per-model reasoning otherwise provider-driven.

## 9. Tools

- **Built-in:** `read`, `write`, `edit`, `bash`, `glob`, `grep`, `webfetch`, `websearch`, `task` (subagent), `skill`. Enable/disable via `tools`; gate via `permission`.
- **LSP integration:** `lsp` config wires Language Server Protocol servers so the agent gets diagnostics/symbols (a first-class, distinctive feature). **Disabled by default** — enabled via `"lsp": true` — and ships **30+ built-in servers** (Python / TypeScript / Rust / Go / PHP …) with auto-install.
- **Formatters:** `formatter` config (prettier / custom) auto-formats edited files.
- **Custom tools:** user-defined tools under `.opencode/tools/`.
- **Skills / commands:** Agent Skills (`skills/`) and custom `command`s (`commands/`, markdown templates).

## 10. MCP

- **Config:** `mcp` key (local + remote servers). CLI: `opencode mcp {add,list,auth,logout,debug}`.
- **Being driven:** OpenCode exposes itself for automation via the **HTTP server** (`serve`, OpenAPI 3.1), **ACP** (`acp`), and the **SDK** — distinct from an MCP-server mode. ⚠ **leaning-refuted, not fully confirmable:** the canonical `opencode.ai/docs/mcp/` slug **404'd this pass (the MCP page has moved)**, so the canonical MCP doc could not be read directly. From what *is* readable: `/docs/cli/` shows `mcp` is **client** management (`add`/`list`/`auth`/`logout`/`debug`), and `/docs/server/` lists MCP only as an *API endpoint category*. No evidence of an OpenCode-as-MCP-*server* mode; being-driven is via `serve` (HTTP/OpenAPI), `acp`, and the SDK. Recheck once the correct MCP slug is located.

## 11. Sharing / sessions / stats

- **Sharing:** `/share` + `share` config (`manual`/`auto`/`disabled`) → hosted conversation links; `export` (`--sanitize`) / `import` (JSON or share URL).
- **Sessions:** `session list`/`delete`; `--continue`/`--session`/`--fork` across `tui`/`run`/`attach`; SQLite-backed persistence (`db path`).
- **Stats:** `opencode stats` (cost/usage by `--days`/`--tools`/`--models`/`--project`).

## 12. GitHub / GitLab / CI

- **GitHub:** `opencode github install` + `opencode github run --event --token` (GitHub Actions automation); `opencode pr` checks out a PR branch.
- **GitLab:** GitLab Duo integration (OAuth provider).
- **Headless/CI:** `opencode run -f json` / `--format json` for scripting; `--pure` global flag; `serve` for API-driven CI.

## 13. Developer surface / enterprise

- **SDK / Server / Plugins / Ecosystem:** documented developer APIs. **Server:** OpenAPI 3.1 spec at `/doc`, basic auth via `OPENCODE_SERVER_PASSWORD` / `OPENCODE_SERVER_USERNAME`. **SDK:** a **generated, type-safe JS/TS SDK** `@opencode-ai/sdk` (`/docs/sdk/`) exposing `createOpencode()` (spawns server + client) and `createOpencodeClient()` (attach to a running server); a **Go SDK** at `/docs/go/`. Plugins load from npm (`plugin`/`plug`, `--global`).
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

- **(a) — RESOLVED (2026-07-27).** Repo/maintainer lineage + license: `anomalyco` = Anomaly Innovations, the **rebranded SST team**; `anomalyco/opencode` is the **same actively-maintained project** as `sst/opencode` (a **TypeScript/Bun** rewrite), *not* a lineage fork of the earlier Go `opencode-ai/opencode`. License is **CONFIRMED MIT**. Closed.
- **(b) — RESOLVED (2026-07-27).** Auth entry points are **complementary**: `/connect` = in-app / TUI manual API-key entry (`/docs/providers/`); `opencode auth login` = CLI path (`/docs/cli/`). Both real; the earlier "CLI authoritative, `/connect` stray" framing was wrong. Closed.
- **(c) — LEANING REFUTED, not fully confirmable (2026-07-27).** Whether a dedicated MCP-*server* mode exists vs client-only + ACP/HTTP (§10). The canonical `opencode.ai/docs/mcp/` slug **404'd (page moved)**, so this could not be fully confirmed; all readable evidence points to client-only + `serve`/`acp`/SDK being-driven. Recheck once the correct MCP slug is found.
- **(d) — STALE/UNCONFIRMED (2026-07-27).** `small_model` default: `/docs/zen/` names **no single hardcoded default**; it describes a low-cost tier (GPT 5.4 Nano, Claude Haiku 4.5, Gemini 3.5 Flash Lite). The earlier "gpt-4-nano default" is unconfirmed — treat as **"no single documented default named."**

---

## Source pages (fetched 2026-07-27)

Canonical docs at `https://opencode.ai/docs/<slug>` — all pages below **HTTP 200**
this pass except where noted. Repo: `github.com/anomalyco/opencode` (200; **MIT**;
anomalyco = rebranded SST/Anomaly, same project as `sst/opencode`). Marketing:
`https://opencode.ai/`.

| Page | Slug | Notes |
|---|---|---|
| Intro / overview | `/docs/` | nav + surfaces (200) |
| CLI | `/docs/cli/` | full subcommand + flag list (200) |
| Config | `/docs/config/` | merge model + keys (200) |
| Providers | `/docs/providers/` | 75+ via Models.dev; `/connect` key entry (200) |
| Agents | `/docs/agents/` | primary/subagent model (200) |
| Permissions | `/docs/permissions/` | allow/ask/deny + patterns; 13 types; doom_loop = 3 identical (200) |
| LSP | `/docs/lsp/` | default-off; 30+ built-in servers, auto-install (200) |
| Server | `/docs/server/` | default TUI+server; OpenAPI 3.1 `/doc`; basic-auth env vars (200) |
| SDK | `/docs/sdk/` | typed `@opencode-ai/sdk`; `createOpencode()` / `createOpencodeClient()` (200) |
| Go SDK | `/docs/go/` | Go SDK (200) |
| Zen | `/docs/zen/` | low-cost tier; no single `small_model` default named (200) |
| Rules / instructions | `/docs/rules/` (nav) | `instructions` files |
| MCP | `/docs/mcp/` | **404 — slug moved**; MCP client mgmt only, no server mode found |
| Share / GitHub / GitLab | `/docs/{share,github,gitlab}/` (nav) | collaboration |
| Plugins | `/docs/plugins/` (nav) | developer surface |
| Enterprise | `/docs/enterprise/` | managed config, policies |
