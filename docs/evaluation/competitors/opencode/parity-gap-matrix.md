# Caliban ↔ OpenCode parity gap matrix

> **What this is:** a living checklist of feature parity between caliban
> (this project) and **OpenCode** (`opencode.ai`) — a genuine head-to-head
> terminal-coding-agent competitor. Refresh it whenever a major feature lands
> or OpenCode ships a new capability. Use it — alongside the
> [Claude Code](../claude-code/parity-gap-matrix.md),
> [Codex](../codex/parity-gap-matrix.md), and
> [Grok Build](../grok-build/parity-gap-matrix.md) matrices — to prioritize the next sprint.
>
> **How to use it:** unlike OpenClaw (a gateway that *orchestrates* coding
> agents), OpenCode is the same *kind* of thing as caliban, so most rows are
> real apples-to-apples comparisons. When shipping a feature that closes a row,
> tick it 🔴 → 🟡 or 🟡 → ✅ in the same PR.
>
> **Companion document:** [`capability-inventory.md`](capability-inventory.md)
> — a structured, dated snapshot of OpenCode's documented surface, captured from
> `opencode.ai/docs`. That file is the *source* this matrix is derived from;
> refresh both together.

**Legend:** ✅ caliban has an equivalent · 🟡 partial · 🔴 gap · **n/a** =
OpenCode-surface concept with no intended caliban analogue (e.g. hosted plane).
A ✅ means "caliban does the equivalent thing," not byte-identical.

**Last refreshed:** 2026-07-18 (initial capture — derived from
[`capability-inventory.md`](capability-inventory.md) snapshot 2026-07-18;
caliban state cross-referenced from the [Claude Code parity
matrix](../claude-code/parity-gap-matrix.md) as of its 2026-06-17 refresh; #487).

> **Caveat:** rows tagged **⚠** depend on an OpenCode fact still flagged
> uncertain in the inventory (§14 there) or on a caliban detail inferred from
> the Claude Code matrix rather than re-verified against `main`.

---

## A. Install & distribution

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| npm / Bun / pnpm / Yarn / Homebrew / Arch / Choco / Scoop / Mise / Docker | 🔴 | caliban builds from source via `cargo`; no package-manager channels yet |
| Self-update (`opencode upgrade`) | 🔴 | no built-in updater |

## B. Architecture (client/server)

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Client/server core; sessions survive client disconnects | 🔴 | caliban is a single-process TUI + headless `-p`; `caliband` supervises subagents, not a session server clients attach to |
| Headless HTTP server (`opencode serve`) | 🔴 | no API server surface |
| Attach a client to a running backend (`opencode attach`) | 🔴 | no attach model |
| Web UI (`opencode web`) | 🔴 | terminal-first (shared with the Claude Code long-tail) |
| ACP (Agent Client Protocol) server (`opencode acp`) | 🔴 | no editor-driving protocol server |

## C. CLI subcommands

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Non-interactive run (`run`, `--format json`) | ✅ | `-p` + `--output-format json/stream-json` (ADR-0025) |
| Continue / session / fork flags | 🟡 | `/resume` + `--resume`; checkpoint fork partial (see Claude Code matrix) |
| Provider auth (`auth login`) | 🟡 | `/login`/`/logout`/`/status` are stubs; auth via env + `apiKeyHelper` |
| Manage agents (`agent create/list`) | 🟡 | subagent files exist; `/agents` editor is a stub |
| List models (`models`) | 🟡 | `/model` runtime swap; no `models` catalog command |
| MCP management (`mcp add/list/...`) | ✅ | `caliban mcp` + `/mcp` (ADR-0023) |
| GitHub automation (`github install/run`) | 🔴 | GitHub Actions deferred |
| Checkout a PR (`pr`) | 🔴 | no PR-checkout command |
| Session list/delete | 🟡 | `/resume` picker; no explicit delete command |
| Usage/cost stats (`stats`) | ✅ | `/usage` + `/cost` (ADR-0033) |
| Export / import session | 🟡 | `/export` ✅; import from JSON/share-URL 🔴 |
| Install plugins (`plugin`) | ✅ | `caliban plugin` (ADR-0030) |
| Diagnostics (`debug`, `db path`) | ✅ | `caliban doctor` / `/doctor` |

## D. Config system

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Merged multi-source config (not replaced) | ✅ | layered settings (managed/user/project/local) with per-key merge (ADR-0026) |
| Project + global + managed + MDM sources | ✅ | four scopes + managed delivery |
| Remote config (`.well-known/opencode`) | 🔴 | no remote-config fetch |
| `{env:VAR}` / `{file:path}` substitution | 🟡 | `${VAR}` expansion in MCP/config; no `{file:...}` inclusion |
| `instructions` glob array | ✅ | CLAUDE.md ancestry + `@`-imports (ADR-0036) |
| Separate TUI theme/keybind config | 🟡 | statusline + settings exist; `/theme` deferred (🔴), keybinds partial |

## E. Permissions

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| `allow` / `ask` / `deny` resolution | ✅ | rule grammar + modes (ADR-0029/0045) |
| Per-tool + wildcard defaults | ✅ | ordered `[[permissions.rules]]` with globstar |
| Per-command bash patterns, last-match-wins | ✅ | `Bash(...)` patterns; deny→ask→allow precedence |
| Agent-level permission overrides | ✅ | subagent `permissionMode` + tool scoping (ADR-0037) |
| `external_directory` gate | ✅ | `additionalDirectories` + `--add-dir` |
| `doom_loop` (repeated-identical-call) guard | 🔴 | turn-loop resilience exists, but no dedicated repeated-call guard |
| `.env` read denied by default | 🟡 | achievable via rules; not a shipped default |

## F. Agents / subagents

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Subagents with per-agent model/tools/permissions | ✅ | subagent frontmatter (ADR-0037) |
| Built-in Explore / Plan / general roles | ✅ | Explore + Plan + general-purpose analogues |
| Markdown agent definitions + frontmatter | ✅ | `.caliban/agents/*.md` frontmatter |
| `steps` / max-iteration cap | ✅ | `maxTurns` per subagent |
| `subagent_depth` recursion control | ✅ | recursion guard (ADR-0021) |
| `@`-mention manual subagent invocation | 🟡 | invoked via `AgentTool`/Task; `@agent` mention not a direct match |
| Primary-agent switching (Build/Plan via Tab) | 🟡 | plan mode + Shift+Tab cycle overlap, but not "swap the primary agent" |
| Plan mode | ✅ | `/plan` + plan permission mode |
| Worktree isolation | ✅ | `isolation: worktree` (ADR-0037) — OpenCode has no first-class worktree isolation |

## G. Models & providers

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Provider breadth (75+ via Models.dev) | 🟡 | caliban: Anthropic/OpenAI/Ollama/Google/Bedrock/Vertex — fewer, hardcoded |
| Local runners (Ollama / LM Studio / OpenAI-compatible) | ✅ | ollama + LMStudio probed |
| Provider-priority routing / fallback | ✅ | router v2 fallback/hedging/breakers (ADR-0038) |
| `small_model` split for light tasks | ✅ | purpose-keyed routing (`FastClassifier`) (ADR-0022) |
| Browser OAuth for providers | 🟡 | MCP OAuth shipped; provider-login OAuth not |
| `--thinking` / reasoning controls | ✅ | `/effort` + `/think` (ADR-0038/#100) |

## H. Tools

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| `read`/`write`/`edit`/`bash`/`glob`/`grep`/`webfetch`/`websearch`/`task`/`skill` | ✅ | full built-in tool set present |
| Snapshot file-tracking + `/undo`/`/redo` | ✅ | auto-checkpoint + `/rewind` (ADR-0028) |
| LSP integration (diagnostics/symbols to the agent) | 🔴 | no Language-Server integration — an OpenCode-distinctive gap |
| Auto-formatters on edit (`formatter`) | 🔴 | no post-edit formatter hook |
| User-defined custom tools (`.opencode/tools/`) | 🟡 | extend via MCP/skills/plugins; no first-class custom-tool dir |
| Image input | ✅ | `caliban-images` (ADR-0039) |

## I. MCP

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| MCP client (local + remote servers) | ✅ | rmcp client, stdio + HTTP (ADR-0023) |
| `mcp add/list/auth/logout` CLI | ✅ | `caliban mcp` |
| Driven via HTTP server / ACP / SDK | 🔴 | see B — no server/ACP surface for being driven |

## J. Sharing / sessions / persistence

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Hosted share links (`/share`, `share` config) | 🔴 | no hosted share plane (n/a-adjacent — local-first) |
| Export / import (`--sanitize`, share-URL import) | 🟡 | `/export` ✅; sanitized/URL import 🔴 |
| Persistent session store (SQLite) | ✅ | session persistence + transcripts |

## K. GitHub / GitLab / CI

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| GitHub Action automation (`github install/run`) | 🔴 | deferred sub-project |
| PR checkout (`pr`) | 🔴 | no PR-checkout helper |
| GitLab Duo integration | 🔴 | no GitLab integration |
| Headless JSON for scripting (`run -f json`, `--pure`) | ✅ | `--output-format json` + `--bare` |

## L. Developer surface / enterprise

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Plugins (npm-loaded) | ✅ | `caliban plugin` marketplace (ADR-0030) |
| SDK / documented Server API | 🔴 | no embedding SDK / HTTP API |
| Managed config + MDM | ✅ | managed settings scope (ADR-0026/0045) |
| Resource-access policies (`experimental.policies`) | 🟡 | permissions + sandbox cover much of this; no separate policy engine |
| Hosted model gateway (OpenCode Zen) | n/a | no first-party hosted gateway |

## M. TUI ergonomics

| Capability (OpenCode) | Caliban | Notes |
|---|---|---|
| Plan mode toggle | ✅ | `/plan` + Shift+Tab |
| Undo/redo | ✅ | `/rewind` + checkpoints |
| Image drag-and-drop | ✅ | ADR-0039 |
| Theme + keybind customization | 🟡 | `/theme` deferred (🔴); keybinds partial |

---

## OpenCode-distinctive gaps worth a ticket

Capabilities OpenCode has that caliban lacks and that aren't already tracked by
the Claude Code matrix — the highest-signal candidates if we chase OpenCode
parity specifically:

1. **Client/server core + `serve`/`attach` + ACP** (B/I) — a backend other
   front-ends (web, IDE, another agent) attach to. This is OpenCode's biggest
   architectural difference and **overlaps with the "caliban as a worker
   backend" note under [OpenClaw](../openclaw/README.md)** (the full OpenClaw
   comparison lives in the Prospero repo) — a server/ACP surface would serve both.
2. **LSP integration** (H) — feed Language-Server diagnostics/symbols to the
   agent. No caliban analogue; high coding-quality leverage.
3. **Auto-formatters on edit** (H) — run prettier/gofmt/etc. after file edits.
4. **`doom_loop` guard** (E) — a dedicated repeated-identical-tool-call circuit
   breaker.
5. **Self-update** (A) — `caliban upgrade`.
6. **GitHub Action + PR checkout** (K) — shared with the Codex/Claude Code
   long-tail; already a known deferred sub-project.

Hosted share plane, web UI, and OpenCode Zen are **deliberately out of scope**
(n/a) — caliban is a local-first terminal agent.

---

## Refresh process

1. When a caliban feature lands: edit the relevant row(s) in the same PR,
   ticking 🔴 → 🟡 or 🟡 → ✅.
2. When OpenCode ships something new: refresh
   [`capability-inventory.md`](capability-inventory.md) first (re-fetch the
   upstream docs), then propagate any new rows here.
3. Resolve any **⚠** rows against OpenCode's live docs / caliban `main` when you
   touch them.
4. Bump the **Last refreshed** date at the top.
