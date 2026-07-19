# Caliban ↔ OpenAI Codex CLI parity gap matrix

> **What this is:** a living checklist of feature parity between caliban
> (this project) and the **OpenAI Codex CLI**. Refresh it whenever a major
> feature lands or Codex ships a new capability. Use it — alongside the
> [Claude Code matrix](../claude-code/parity-gap-matrix.md) — to prioritize
> the next sprint.
>
> **How to use it:** Codex is a *second* reference agent. Most core surfaces
> caliban already tracks against Claude Code (permissions, hooks, MCP,
> sub-agents, model router, sandbox, headless, OTel); this matrix's job is to
> surface where **Codex differs** — the capabilities Codex has that caliban
> does not, and the places caliban's design diverges. When planning what to
> build, look here for Codex-distinctive gaps; when shipping, tick the row(s)
> in the same PR that ships the code.
>
> **Companion document:** [`capability-inventory.md`](capability-inventory.md)
> — a structured, dated snapshot of Codex's documented surface, captured from
> the canonical docs (`developers.openai.com/codex/*` → `learn.chatgpt.com`).
> That file is the *source* this matrix is derived from; refresh both together.

**Legend:** ✅ caliban has an equivalent · 🟡 partial · 🔴 gap · **n/a** =
Codex-surface concept with no intended caliban analogue (e.g. hosted cloud). A
✅ here means "caliban does the equivalent thing," not that the two are
byte-for-byte identical.

**Last refreshed:** 2026-07-18 (initial capture — derived from
[`capability-inventory.md`](capability-inventory.md) snapshot 2026-07-18;
caliban state cross-referenced from the [Claude Code parity
matrix](../claude-code/parity-gap-matrix.md) as of its 2026-06-17 refresh; #485).

> **Caveat:** rows tagged **⚠** depend on a Codex fact still flagged uncertain
> in the inventory (§14 there) or on a caliban detail inferred from the Claude
> Code matrix rather than re-verified against `main`. Re-verify before quoting.

---

## A. Install & distribution

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| npm (`@openai/codex`) / Homebrew cask / shell + PowerShell installers | 🔴 | caliban builds from source via `cargo`; no published npm/brew/installer channel yet (GHCR image tracked separately, not yet shipped) |
| Prebuilt binaries macOS (arm64+x86_64) / Linux (x86_64+arm64) | 🔴 | release-binary distribution not yet stood up |
| Windows support | 🟡 | caliban runs on Windows/WSL for most paths; OS sandbox on Windows is deferred (see E) |

## B. CLI subcommands

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Non-interactive run (`codex exec`) | ✅ | caliban `-p`/`--print` headless mode (ADR-0025) |
| `codex resume` (continue a session) | ✅ | session persistence + `/resume`; headless `--resume` |
| `codex fork` (branch a session) | 🟡 | `/rewind` + Esc-Esc fork-from-checkpoint partial; full session fork tracked under the sub-agent fleet spec |
| `codex apply` (apply a diff to the tree) | 🔴 | no standalone diff-apply subcommand; caliban's agent edits in place |
| `codex review` (non-interactive review) | 🔴 | `/code-review` is skill-level, deferred to the Skills polish sub-project |
| `codex mcp` (manage MCP servers) | ✅ | `caliban mcp` + `/mcp` (ADR-0023) |
| `codex mcp-server` (Codex **as** an MCP server) | 🔴 ⚠ | caliban is an MCP client only; exposing caliban itself as an MCP server is unbuilt |
| `codex plugin` + `marketplace` | ✅ | `caliban plugin {install,list,enable,disable,remove,info,update}` + marketplace (ADR-0030) |
| `codex sandbox` (run a command under a policy) | 🟡 | caliban has OS sandbox but no standalone "sandbox an arbitrary command" subcommand (ADR-0032) |
| `codex execpolicy` (evaluate rule files) | 🟡 | `caliban perms lint/test/explain` cover rule evaluation; not a 1:1 exec-policy evaluator |
| `codex doctor` | ✅ | `caliban doctor` headless + `/doctor` |
| `codex cloud` / `cloud-tasks` | n/a | no hosted cloud plane (see M) |

## C. Interactive TUI

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| `/` command menu + `@` file mention/search | ✅ | slash-menu typeahead (#15) + `@file` autocomplete (ADR-0027) |
| `/permissions` presets (Auto / Read Only / Full Access) | ✅ | permission modes + Shift+Tab cycle + status chip (ADR-0029) |
| `/reasoning` (adjust effort) | ✅ | `/effort` + `/think` runtime controls (ADR-0038, #100) |
| `/compact`, `/status` (context + rate limits) | ✅ | `/compact`, `/context`, `/usage` (ADR-0033) |
| `/review` (code-review mode) | 🔴 | skill-level, deferred |
| Raw-output copy (`Ctrl+O` / `Alt+R`, `tui.raw_output_mode`) | 🟡 | `Ctrl+O` transcript viewer + `[` dump-to-scrollback exists; single-response raw-copy chord not a direct match |
| Image input (`--image` / paste) | ✅ | `caliban-images` ingest (clipboard, `@path`, DnD) (ADR-0039) |

## D. Config system

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| TOML config (`~/.codex/config.toml` + project `.codex/`) | ✅ | `caliban.toml` + layered settings (managed/user/project/local); TOML primary (ADR-0026/0045) |
| Named profiles (`--profile`) | 🟡 | model-router routes/effort-maps cover some of this; no first-class named-profile switch |
| Enterprise policy layer (`requirements.toml`, managed-hooks gate) | ✅ | managed settings scope + `allow_managed_hooks_only` / `permissions.enforce` lockdown |
| Inline overrides (`-c KEY=VALUE`) | 🟡 | `--settings` (file/inline JSON) + `--setting-sources`; per-key `-c` dotted override not a direct match |
| Published schema for editor autocomplete | ✅ | embedded settings schema (`caliban-settings/src/schema.json`, Draft-7) |

## E. Approval modes & sandboxing

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Orthogonal sandbox × approval axes | ✅ | permission rules/modes (approval axis) + OS sandbox (boundary axis) compose independently (ADR-0029/0032/0045) |
| Sandbox modes (`read-only` / `workspace-write` / `danger-full-access`) | ✅ | permission modes + sandbox filesystem allow/deny map cover the equivalent spectrum |
| Approval policies (`untrusted`/`on-request`/`on-failure`/`never`) | 🟡 | caliban modes (`default`/`acceptEdits`/`plan`/`auto`/`dontAsk`/`bypassPermissions`) overlap but don't map 1:1; no `on-failure`-style escalate-on-error policy |
| macOS Seatbelt enforcement | ✅ | ADR-0032 |
| Linux `bubblewrap` / user-namespace enforcement | ✅ | ADR-0032 (Linux/WSL) |
| Windows native sandbox | 🔴 | Windows sandbox deferred |
| Network-access gating in workspace-write | ✅ | sandbox `network.allow/denyDomains` + proxy knobs |

## F. MCP

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| MCP client, stdio + streamable-HTTP transports | ✅ | rmcp client; stdio + HTTP/SSE (ADR-0023) |
| Per-server enable / tool allow-deny / approval mode | ✅ | per-server permission scoping + `enabled_tools` equivalents |
| OAuth / bearer auth for HTTP servers | ✅ | PKCE + loopback OAuth, keyring store (ADR-0023 Phase C) |
| Startup / tool timeouts | ✅ | `CALIBAN_MCP_TIMEOUT` / `CALIBAN_MCP_TOOL_TIMEOUT` |
| Codex **as** MCP server (`mcp-server`) | 🔴 ⚠ | see B — unbuilt |

## G. Models & providers

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Multiple providers + local models (`--oss`, ollama/lmstudio) | ✅ | Anthropic/OpenAI/Ollama/Google/Bedrock/Vertex (broader set); ollama + LMStudio probed |
| Reasoning-effort tiers | ✅ | `low`/`medium`/`high` + effort map (ADR-0038); `/effort` runtime |
| `ultra` tier that auto-delegates to subagents | 🔴 | no effort tier that automatically fans out to a subagent fleet |
| Live web search (`--search`) | ✅ | `WebSearch` (Brave/Tavily/Exa) |
| Provider wire-API selection (Responses vs Chat) | 🟡 | caliban targets Anthropic + OpenAI wire shapes; no Responses-vs-Chat toggle abstraction |

## H. Memory / project instructions

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Nested instruction file, closer-dir-wins precedence | ✅ | CLAUDE.md ancestor walk + nested-on-demand (ADR-0036) |
| `AGENTS.md` as the primary instruction file | 🟡 | caliban's primary is CLAUDE.md; `/init` ingests `AGENTS.md`/`.cursorrules`/`.windsurfrules` but doesn't read `AGENTS.md` as the live source |
| Model-written per-project memory | ✅ | auto-memory (ADR-0035) |
| Cross-session "Memories" / "Chronicle" | 🔴 ⚠ | partly a Codex app/cloud feature; no caliban equivalent to cross-session learned memory |

## I. Hooks / skills / plugins / notifications

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Lifecycle hooks (`PreToolUse`/`PostToolUse`/`SessionStart`/`SessionStop`) | ✅ | caliban hook taxonomy is a superset (ADR-0024) |
| Regex `matcher` on hooks | ✅ | matcher-group filtering |
| Enterprise `allow_managed_hooks_only` | ✅ | same key honored (ADR-0045) |
| Skills (`SKILL.md`, `.agents/skills`, open standard) | 🟡 | caliban ships skills, but under `.caliban/`/`.claude/` layout, not the `.agents/skills` open-standard path |
| Plugin marketplace (skills + MCP + hooks + connectors) | ✅ | `caliban plugin` marketplace bundles skills/hooks/agents/MCP/output-styles (ADR-0030) |
| Plugins bundling browser extensions / scheduled-task templates | 🔴 | no browser-extension or scheduled-task packaging |
| `notify` external-script notifications | 🟡 | status-line runner + hook surface can approximate; no dedicated `notify` script contract |

## J. Sub-agents / parallelism

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Custom subagent definitions with per-agent model/sandbox/MCP overrides | ✅ | subagent frontmatter (`model`, `tools`, `permissionMode`, `mcpServers`, `isolation: worktree`) (ADR-0037) |
| Subagent file format | 🟡 ⚠ | caliban uses Markdown+frontmatter; Codex canonical is TOML (inventory §11 flags version drift) |
| Auto-parallelized delegation, orchestration auto-managed | 🟡 | `AgentTool` + background fleet exist, but fan-out is agent-driven, not an automatic orchestrator |
| Worktree isolation | ✅ | `caliban-worktrees`, `isolation: worktree` (ADR-0037) |
| Background fleet + supervisor daemon | ✅ | `caliban-supervisor` + `caliband` (ADR-0037) |

## K. Headless / CI

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Non-interactive exec + stderr progress / stdout result | ✅ | `-p` + `--output-format text/json/stream-json` (ADR-0025) |
| NDJSON event stream (`--json`, typed `thread.*`/`turn.*`/`item.*`) | ✅ | `stream-json` NDJSON frames (`system/init`, `message`, `tool_use`, `tool_result`, `result`) |
| JSON-Schema-constrained output (`--output-schema`) | 🟡 | `--json-schema` is best-effort local validation; native constrained decoding lands with ADR-0032 |
| Stdin piping (`codex exec -`) | ✅ | `--input-format` stdin (10 MiB cap) |
| Env-key auth for CI | ✅ | `ANTHROPIC_API_KEY` / provider env + `--bare` |
| Official GitHub Action (`openai/codex-action`) | 🔴 | GitHub Actions workflow deferred (separate sub-project) |

## L. Observability / cost

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| OpenTelemetry export (`[otel]`) | ✅ | OTLP metrics/logs/traces, `CALIBAN_ENABLE_TELEMETRY=1` (ADR-0033) |
| Session history log (`history.jsonl`) | ✅ | session persistence + transcript export (`/export`) |
| In-session context + rate-limit status (`/status`) | ✅ | `/context` + `/usage` |
| Diagnostics (`codex doctor`) | ✅ | `caliban doctor` / `/doctor` |

## M. Cloud / IDE / long-tail

All 🔴 or **n/a** — large investments, parked until terminal/CLI parity, and
mostly outside caliban's local-first scope. Tracked only so we remember they
exist:

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Codex Cloud (isolated parallel cloud tasks, delegate-from-CLI) | n/a | no hosted plane; out of scope for the local agent |
| GitHub `@codex` PR review + cloud delegation | 🔴 | no GitHub-app review path |
| IDE extension (VS Code / Cursor / Windsurf / JetBrains) | 🔴 | shared with the Claude Code long-tail (matrix §N) |
| Delegate-to-cloud continuum (local ↔ cloud handoff) | n/a | no cloud plane to hand off to |

---

## Codex-distinctive gaps worth a ticket

Capabilities Codex has that caliban does **not**, and that aren't already
tracked by the Claude Code matrix — the highest-signal candidates if we decide
to chase Codex parity specifically:

1. **`mcp-server` mode** (B/F) — expose caliban itself as an MCP server so other
   agents can drive it. Small, high-leverage, no caliban analogue.
2. **`--output-schema` constrained decoding** (K) — move from best-effort JSON
   validation to provider-native structured output (already gated on ADR-0032).
3. **`ultra`-style auto-delegating effort tier** (G/J) — an effort level that
   automatically fans work out to the background subagent fleet.
4. **Standalone `sandbox` / `execpolicy` subcommands** (B/E) — run/evaluate an
   arbitrary command under a sandbox policy outside a full session.
5. **`on-failure` approval policy** (E) — run sandboxed, escalate only on error.
6. **AGENTS.md as a live, first-class instruction source** (H) — read it
   directly (with nested precedence), not just ingest it at `/init`.

Cloud plane, IDE extension, and GitHub-app review are **deliberately out of
scope** (n/a) — caliban is a local-first terminal agent; do not file these as
parity gaps.

---

## Refresh process

1. When a caliban feature lands: edit the relevant row(s) in this matrix in the
   same PR, ticking 🔴 → 🟡 or 🟡 → ✅.
2. When Codex ships something new: refresh
   [`capability-inventory.md`](capability-inventory.md) first (re-fetch the
   upstream docs + bump the currency marker), then propagate any new rows here.
3. Resolve any **⚠** rows against Codex's live docs / caliban `main` when you
   touch them.
4. Bump the **Last refreshed** date at the top.
