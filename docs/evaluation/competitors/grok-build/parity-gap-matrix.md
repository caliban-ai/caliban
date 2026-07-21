# Caliban ↔ Grok Build parity gap matrix

> **What this is:** a living checklist of feature parity between caliban
> (this project) and **Grok Build** (`x.ai/cli`) — xAI's terminal coding agent,
> a genuine head-to-head competitor. Refresh it whenever a major feature lands
> or Grok Build ships a new capability. Use it — alongside the
> [Claude Code](../claude-code/parity-gap-matrix.md),
> [Codex](../codex/parity-gap-matrix.md),
> [OpenCode](../opencode/parity-gap-matrix.md), and
> [Google Antigravity](../antigravity/parity-gap-matrix.md) matrices — to
> prioritize the next sprint.
>
> **How to use it:** Grok Build is the same *kind* of thing as caliban (a
> terminal coding agent), so most rows are real apples-to-apples comparisons.
> One twist: Grok Build reads the **Claude Code / AGENTS.md** ecosystem
> natively, and caliban is itself a Claude Code-lineage agent, so many config
> rows land ✅ almost by construction. When shipping a feature that closes a
> row, tick it 🔴 → 🟡 or 🟡 → ✅ in the same PR.
>
> **Companion document:** [`capability-inventory.md`](capability-inventory.md)
> — a structured, dated snapshot of Grok Build's documented surface. That file
> is the *source* this matrix is derived from; refresh both together.

**Legend:** ✅ caliban has an equivalent · 🟡 partial · 🔴 gap · **n/a** =
Grok-Build-surface concept with no intended caliban analogue (e.g. hosted
model plane). A ✅ means "caliban does the equivalent thing," not
byte-identical.

**Last refreshed:** 2026-07-19 (initial capture — derived from
[`capability-inventory.md`](capability-inventory.md) snapshot 2026-07-19;
caliban state cross-referenced from the [Claude Code parity
matrix](../claude-code/parity-gap-matrix.md) and the
[OpenCode matrix](../opencode/parity-gap-matrix.md)).

> **Caveat:** rows tagged **⚠** depend on a Grok Build fact still flagged
> uncertain in the inventory (§14 there — canonical `docs.x.ai` 403'd automated
> fetch, so detail is cross-checked from secondary sources) or on a caliban
> detail inferred from the sibling matrices rather than re-verified against
> `main`.

---

## A. Install & distribution

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| One-line install script (`curl … x.ai/cli/install.sh`) | 🔴 | caliban builds from source via `cargo`; no install-script channel yet |
| Background self-update (`--no-auto-update` to disable) | 🔴 | no built-in updater |
| Open-source harness/TUI (Apache-2.0) | ✅ | caliban is open source (harness + TUI in-repo) |

## B. Surfaces & architecture

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Interactive fullscreen TUI (mouse, subagent view) | ✅ | caliban ships a TUI; mouse/subagent-view parity ⚠ verify against `main` |
| Headless / non-interactive (`grok -p`) | ✅ | `-p` + `--output-format json/stream-json` (ADR-0025) |
| ACP agent over JSON-RPC (being driven by editors) | 🔴 | no editor-driving protocol server (shared gap with the OpenCode ACP row) |

## C. CLI subcommands

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Headless run w/ structured output (`-p`, `--output-format streaming-json`) | ✅ | `-p` + `--output-format json/stream-json` + `--bare` |
| Auto-approve flag (`--always-approve`) | ✅ | `--dangerously-skip-permissions` / bypass mode (ADR-0029) |
| MCP management (`grok mcp add/list/remove`) | ✅ | `caliban mcp` + `/mcp` (ADR-0023) |
| Marketplace skills CLI (`grok skill search/install/list/remove`) | 🟡 | `caliban plugin` marketplace (ADR-0030) covers install/list; skill *search* over a hosted marketplace 🔴 |
| Discovery/inspect (`grok inspect`) | ✅ | `caliban doctor` / `/doctor` surfaces discovered config/MCP/sources |
| Provider auth / login subcommand | 🟡 | `/login`/`/logout`/`/status` are stubs; auth via env + `apiKeyHelper` |
| Session list/manage | 🟡 | `/resume` picker; no explicit delete/list-and-manage command |

## D. Config system

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Layered global → project config | ✅ | layered settings (managed/user/project/local) with per-key merge (ADR-0026) |
| Reads `CLAUDE.md` + ancestry | ✅ | CLAUDE.md ancestry + `@`-imports (ADR-0036) |
| Reads `.claude/` tree (skills/agents/MCPs/hooks/rules) | ✅ | caliban's native config tree is the same lineage |
| Reads AGENTS.md family | 🟡 | CLAUDE.md is first-class; AGENTS.md ingestion ⚠ verify against `main` |
| Reads Claude Code marketplaces/plugins natively | 🟡 | caliban plugin marketplace exists; cross-reading *Claude Code* plugin packs 🔴 |
| TOML config w/ local-inference pointer | ✅ | settings support provider/base-URL config incl. local runners |

## E. Permissions

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Per-tool-call approval gating | ✅ | rule grammar + modes (ADR-0029/0045) |
| Coarse mode switch (`ask` / `always-approve`) | ✅ | permission modes incl. ask + bypass; **finer-grained than Grok's** (allow/ask/deny rules, Bash patterns) |
| User vs project-scoped permission override | ✅ | four config scopes with merge (ADR-0026) |

## F. Agents / subagents

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Parallel subagents (up to 8; research/impl/review) | ✅ | parallel subagents supervised by `caliband` (parallel-subagent probe) |
| Per-subagent git-worktree isolation | ✅ | `isolation: worktree` (ADR-0037) |
| Parallel issue-fixing across worktrees | 🟡 | worktree isolation exists; a packaged multi-issue fan-out workflow 🔴 |
| Arena Mode (competing outputs) | 🔴 | no built-in competing-output/tournament mode |
| Markdown agent definitions + `/agents` | ✅ | `.caliban/agents/*.md` frontmatter; `/agents` editor is a stub (🟡 UI) |
| Recursion/depth control | ✅ | recursion guard (ADR-0021) + `maxTurns` |

## G. Models & providers

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Purpose-built coding model (grok-build-0.1, 256K) | n/a | caliban is model-agnostic; no first-party model |
| xAI / Grok provider | 🔴 | providers: Anthropic/OpenAI/Ollama/Google/Bedrock/Vertex — no xAI/Grok backend wired |
| Runtime model swap (`/model`) | ✅ | `/model` runtime swap |
| Fast/heavy model split | ✅ | purpose-keyed routing + `FastClassifier` (ADR-0022); router v2 (ADR-0038) |
| Local / OpenAI-compatible inference | ✅ | Ollama + LM Studio probed |
| Reasoning-effort control | ✅ | `/effort` + `/think` (ADR-0038/#100) |

## H. Tools

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| read/write/edit/shell/search/git/subagent | ✅ | full built-in tool set present |
| Diff-gated edits before apply | ✅ | edit review + auto-checkpoint + `/rewind` (ADR-0028) |
| Image input | ✅ | `caliban-images` (ADR-0039) |

## I. Plan mode

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Plan mode (Shift+Tab; writes blocked except plan scratchpad) | ✅ | `/plan` + plan permission mode + Shift+Tab cycle |
| Approve / comment-per-step / rewrite the plan | 🟡 | approve + edit before execution; per-step commenting UX ⚠ verify |

## J. Skills / plugins / marketplace

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Skills read from `.grok/` + `.claude/` | ✅ | Agent Skills supported (Claude Code lineage) |
| Marketplace install (`@xai/…`, self-host from git) | 🟡 | `caliban plugin` marketplace (ADR-0030); namespaced hosted-skill search 🔴 |
| One-install bundles (skills+agents+hooks+MCP) | 🟡 | plugins can bundle; parity of a single skills+agents+hooks+MCP pack ⚠ verify |

## K. Hooks

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| Lifecycle hooks (`.grok/hooks.json`) | ✅ | hooks system present (Claude Code-style event hooks) |
| Events: pre/post-edit, pre/post-commit, on-error, on-complete | 🟡 | edit/tool-lifecycle hooks exist; exact commit/error/complete event coverage ⚠ verify |

## L. MCP / ACP / CI

| Capability (Grok Build) | Caliban | Notes |
|---|---|---|
| MCP client (local + remote) | ✅ | rmcp client, stdio + HTTP (ADR-0023) |
| `mcp add/list/remove` + `/mcps` | ✅ | `caliban mcp` + `/mcp` |
| ACP agent (JSON-RPC, being driven) | 🔴 | no ACP/editor-driving surface (shared with OpenCode B) |
| Headless streaming-json for CI/GitHub Actions | ✅ | `--output-format stream-json`; GitHub Action itself deferred |
| First-party GitHub Action / PR bot | 🔴 | GitHub Actions deferred sub-project ⚠ verify Grok's first-party offering |

---

## Grok-Build-distinctive gaps worth a ticket

Capabilities Grok Build has that caliban lacks and that aren't already tracked
by the Claude Code / OpenCode matrices — the highest-signal candidates if we
chase Grok Build parity specifically:

1. **ACP agent over JSON-RPC** (B/L) — a protocol surface for editors/automation
   to drive caliban. **Overlaps with the OpenCode `serve`/`attach`/ACP row** and
   the "caliban as a worker backend" strategic note — one surface would serve
   several matrices.
2. **Arena Mode** (F) — parallel competing agent outputs for comparison; no
   caliban analogue.
3. **Hosted marketplace skill *search*** (`grok skill search`) (C/J) — caliban's
   `plugin` marketplace has install/list but not hosted namespaced search.
4. **One-line install script + self-update** (A) — `caliban upgrade` + a
   packaged binary channel (shared with the OpenCode/Codex long-tail).
5. **Native Claude Code marketplace/plugin cross-reading** (D/J) — Grok ingests
   *Claude Code* plugin packs directly; caliban reads its own tree but not
   Claude Code plugin marketplaces.
6. **xAI / Grok provider backend** (G) — wire xAI as a first-class provider if a
   Grok backend is in scope.

The grok-build-0.1 / Grok-4.x hosted models are **out of scope** (n/a) — caliban
is model-agnostic and ships no first-party model.

---

## Refresh process

1. When a caliban feature lands: edit the relevant row(s) in the same PR,
   ticking 🔴 → 🟡 or 🟡 → ✅.
2. When Grok Build ships something new: refresh
   [`capability-inventory.md`](capability-inventory.md) first (re-fetch the
   upstream docs), then propagate any new rows here.
3. Resolve any **⚠** rows against Grok Build's live docs / repo and caliban
   `main` when you touch them — several rows here lean on secondary sources
   because `docs.x.ai` 403'd the automated capture.
4. Bump the **Last refreshed** date at the top.
