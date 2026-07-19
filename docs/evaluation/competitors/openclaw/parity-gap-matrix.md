# Caliban ↔ OpenClaw parity gap matrix

> **What this is:** a living comparison between caliban (this project) and
> **OpenClaw**. Refresh it whenever a major feature lands or OpenClaw ships a
> new capability.
>
> **⚠ Read the framing first — this is not a like-for-like parity target.**
> OpenClaw is a **multi-channel personal-assistant gateway**, not a terminal
> coding engine. Its coding path **delegates to background workers — Codex,
> Claude Code, or OpenCode** — in isolated git worktrees (see
> [`capability-inventory.md`](capability-inventory.md) §6). So the two products
> overlap only partially:
> - **Section A — coding engine:** where caliban competes directly. Caliban
>   *is* the kind of engine OpenClaw delegates to; this is the honest
>   head-to-head.
> - **Section B — shared infrastructure:** model providers, MCP, skills,
>   plugins, sandbox/approvals, hooks, memory, headless, observability — real
>   parity comparisons.
> - **Section C — OpenClaw-distinctive surface:** channels, gateway/node mesh,
>   media generation, companion apps. Almost all **n/a** — caliban is a
>   local-first terminal agent and is *not trying to be* a chat gateway. Do
>   **not** file these as gaps.
>
> **Strategic note:** because OpenClaw *consumes* coding agents, the highest-
> leverage move here may be to make **caliban a supported OpenClaw worker
> backend** (alongside Codex/Claude Code/OpenCode) — a distribution channel,
> not a parity gap. See the closing section.
>
> **How to use it:** when shipping a feature that closes a Section A/B row, tick
> it 🔴 → 🟡 or 🟡 → ✅ in the same PR.
>
> **Companion document:** [`capability-inventory.md`](capability-inventory.md)
> — the dated snapshot of OpenClaw's documented surface this matrix derives
> from; refresh both together.

**Legend:** ✅ caliban has an equivalent · 🟡 partial · 🔴 gap · **n/a** =
OpenClaw-surface concept with no intended caliban analogue (out of scope by
design). A ✅ means "caliban does the equivalent thing," not byte-identical.

**Last refreshed:** 2026-07-18 (initial capture — derived from
[`capability-inventory.md`](capability-inventory.md) snapshot 2026-07-18;
caliban state cross-referenced from the [Claude Code parity
matrix](../claude-code/parity-gap-matrix.md) as of its 2026-06-17 refresh; #486).

> **Caveat:** rows tagged **⚠** depend on an OpenClaw fact still flagged
> uncertain in the inventory or on a caliban detail inferred from the Claude
> Code matrix rather than re-verified against `main`.

---

## Section A — Coding engine (direct competition)

Where OpenClaw *delegates*, caliban *is* the engine. These rows compare
caliban's native coding surface to what OpenClaw's `coding-agent` skill orchestrates.

| Capability | Caliban | Notes |
|---|---|---|
| Native code-editing engine (not delegated) | ✅ | caliban is a first-party engine; OpenClaw shells out to Codex/Claude Code/OpenCode workers |
| File tools (`read`/`write`/`edit`/`apply_patch`) | ✅ | Read/Write/Edit/MultiEdit; `apply_patch`-style atomic multi-replace covered |
| Shell / command execution | ✅ | Bash tool (+ background bash) |
| Isolated git worktrees for work | ✅ | `caliban-worktrees`, `isolation: worktree` (ADR-0037) — same isolation model OpenClaw uses for workers |
| Branch/ancestry checks before push | 🟡 | worktree base-ref handling exists; explicit pre-push ancestry "proof" cycle is OpenClaw-skill-specific |
| Review-until-clean loop | 🔴 | `/code-review` is skill-level, deferred; OpenClaw runs review cycles "until no accepted findings" |
| Issue→PR / PR-review coding loops | 🔴 | no GitHub-app / PR loop (shared with the Claude Code long-tail) |
| Report results back over a chat channel | n/a | OpenClaw messages a channel; caliban returns to the terminal/headless caller |

## Section B — Shared infrastructure (real parity)

| Capability | Caliban | Notes |
|---|---|---|
| Model providers | 🟡 | caliban: Anthropic/OpenAI/Ollama/Google/Bedrock/Vertex. OpenClaw advertises 60+ (incl. routing gateways + the Claude CLI as a backend) — broader breadth, not depth |
| Local model runners (Ollama / LM Studio) | ✅ | both; caliban probed against both |
| Provider routing / fallback | ✅ | `caliban-model-router` v2 (fallback/hedging/breakers) (ADR-0038) ≈ OpenClaw `models fallbacks` |
| Per-agent model selection | ✅ | subagent frontmatter `model` |
| Reasoning-effort / thinking controls | ✅ ⚠ | caliban `/effort` + `/think` (ADR-0038/#100); OpenClaw's effort controls not documented at capture |
| MCP client | ✅ | rmcp client, stdio + HTTP (ADR-0023) |
| MCP server mode (`mcp serve`) | 🔴 ⚠ | caliban is client-only; OpenClaw can run as an MCP server (same gap flagged for Codex) |
| Skills (`SKILL.md`) | ✅ | caliban skills loader |
| Plugins (tools/skills/hooks/providers) | ✅ | `caliban plugin` + marketplace (ADR-0030) |
| Hosted public registry (ClawHub + publish CLI) | 🟡 | caliban has a marketplace concept but no hosted public registry with a moderated publish pipeline |
| Sandbox environments | ✅ | OS sandbox (Seatbelt / bubblewrap) (ADR-0032); `openclaw sandbox` ≈ managed sandbox |
| Approvals / exec-policy / allowlist | ✅ | permission rules + modes + `caliban perms` (ADR-0029/0045) |
| Container isolation (`--container`) | 🟡 | caliban ships via docker-compose but has no `--container` run-in-container flag |
| Lifecycle hooks | ✅ | caliban hook taxonomy is a superset (ADR-0024); OpenClaw hooks are installable/updatable like plugins |
| Scheduled / cron runs | 🟡 | `/loop` short-interval only; no persistent `cron`-style scheduler (remote/scheduled agents deferred) |
| Indexed memory | ✅ | auto-memory (ADR-0035) |
| Knowledge base / wiki (ingest, Obsidian/ChatGPT import) | 🔴 | no `wiki`-style ingest/synthesis surface |
| Headless + JSON output | ✅ | `-p` + `--output-format json/stream-json` (ADR-0025); OpenClaw `--json` global flag |
| Lazy tool discovery | ✅ | `ToolSearch` / `tools.lazy_mcp` (ADR-0046) ≈ OpenClaw `tool_search`/`tool_describe` |
| Code-aware tool search (`tool_search_code`) | 🔴 | no code-indexed tool discovery variant |
| Browser automation tool | 🔴 | caliban has `WebFetch`/`WebSearch`, not a full `browser` control tool |
| Config system + profiles | 🟡 | layered settings ✅ (ADR-0026); named `--profile` isolation not a direct match |
| Observability / diagnostics (OTel, doctor, audit) | ✅ | OTel export + `caliban doctor` (ADR-0033) |
| Output compacting (`Tokenjuice`) | ✅ | MicroCompact + tool-result cap (Plan B) cover exec-output compaction |

## Section C — OpenClaw-distinctive surface (out of scope for caliban)

All **n/a** or 🔴 by design — OpenClaw is a personal-assistant *gateway*;
caliban is a local-first terminal coding agent and is **not** trying to be a
chat gateway or device mesh. Listed only so we recognize the boundary.

| Capability | Caliban | Notes |
|---|---|---|
| Multi-channel messaging (Discord/Slack/WhatsApp/Signal/iMessage/Telegram/…) | n/a | not a messaging product |
| Gateway daemon + typed WebSocket control plane + device pairing | n/a | `caliband` supervises subagents, not channels/devices |
| Node mesh (mobile/desktop nodes: canvas, camera, screen-record, voice) | n/a | no device-node model |
| Web Control UI / macOS menu-bar app / Windows Hub | n/a | terminal-first |
| Media generation (`image`/`music`/`video`/`tts`) | n/a | not a media tool |
| Voice wake / talk modes + transcription providers | n/a | no voice surface |
| Peer/mesh directory (`directory`, peers/groups) | n/a | no peer topology |

---

## Strategic note — caliban as an OpenClaw worker backend

OpenClaw's `coding-agent` skill already delegates to **Codex, Claude Code, and
OpenCode** workers (inventory §6). The highest-leverage engagement with OpenClaw
is therefore *integration, not imitation*: getting caliban recognized as a
supported worker backend. Prerequisites that map to real caliban work:

1. **Non-interactive worker contract** — caliban's `-p` headless mode + NDJSON
   stream (ADR-0025) already fits the "run in a worktree, stream progress,
   report a final status" shape OpenClaw expects. ✅ mostly there.
2. **Permission-bypass / non-PTY run mode** — OpenClaw drives Claude Code
   "without PTY, in permission-bypass mode." Caliban has
   `--allow-dangerously-skip-permissions` + `--bare`; verify it runs cleanly
   without a PTY. 🟡 verify.
3. **MCP server mode** (Section B) — would let OpenClaw drive caliban over MCP
   directly, the cleanest integration path. 🔴 — the one genuinely new build.

## Distinctive gaps worth a ticket (if we chase them)

Only these OpenClaw capabilities are plausibly worth caliban building; the rest
is out of scope:

1. **MCP server mode** (B) — also flagged for Codex; unlocks the OpenClaw-worker
   path above.
2. **Browser automation tool** (B) — a first-class `browser` control tool beyond
   WebFetch/WebSearch.
3. **`cron`-style persistent scheduler** (B) — scheduled/recurring agent runs
   beyond `/loop`.
4. **Code-aware tool discovery** (B) — a `tool_search_code` analogue.

---

## Refresh process

1. When a caliban feature lands: tick the relevant Section A/B row(s) in the same PR.
2. When OpenClaw ships something new: refresh
   [`capability-inventory.md`](capability-inventory.md) first, then propagate here.
3. Resolve **⚠** rows against OpenClaw's live docs / caliban `main` when you touch them.
4. Keep Section C as a boundary marker — don't let out-of-scope rows creep into the sprint backlog.
5. Bump the **Last refreshed** date at the top.
