# OpenClaw — tracked in the Prospero repo (worker-backend note only)

> **The full OpenClaw comparison moved to Prospero.** OpenClaw is a
> multi-channel **assistant gateway** whose core is an agent *control plane* —
> it launches, routes to, and observes agent workers, and for coding it
> delegates to background workers (Codex / Claude Code / OpenCode) in isolated
> git worktrees. That is **Prospero's** category (the orchestration layer over
> caliban), not caliban's. Compared against a single terminal agent, most of
> OpenClaw's surface was necessarily out of scope; compared against Prospero,
> the launch / fleet / observe / persist / dashboard rows are real parity.
>
> **→ Full inventory + Prospero ↔ OpenClaw parity matrix:**
> [`caliban-ai/prospero` › `docs/evaluation/competitors/openclaw/`](https://github.com/caliban-ai/prospero/tree/main/docs/evaluation/competitors/openclaw)

This directory keeps only the one **caliban-relevant** angle: OpenClaw is a
potential *consumer* of caliban — a coding-agent **worker backend** it could
delegate to, alongside Codex / Claude Code / OpenCode.

## caliban as an OpenClaw worker backend

OpenClaw's `coding-agent` skill drives external agents "run in a worktree, stream
progress, report a final status." Making caliban a supported backend is
integration work, not parity work:

| Prerequisite | caliban | Notes |
|---|---|---|
| Non-interactive worker contract (run in a worktree, stream progress, final status) | ✅ | `-p` headless + NDJSON stream (ADR-0025) already fits the shape |
| Permission-bypass / non-PTY run mode | 🟡 | `--allow-dangerously-skip-permissions` + `--bare` exist; verify a clean run with no PTY (OpenClaw drives Claude Code non-PTY, permission-bypass) |
| Server / ACP / MCP-server surface to be *driven* | 🔴 | the cleanest integration path — and the single highest-leverage gap across the sibling matrices (Codex `mcp-server`, [OpenCode](../opencode/parity-gap-matrix.md) `serve`/ACP, [Grok Build](../grok-build/parity-gap-matrix.md) ACP); a server surface lets OpenClaw (and Prospero, and editors) drive caliban directly |

The third row is the same gap Codex (`mcp-server`), OpenCode (`serve`/`attach`/ACP),
and Grok Build (ACP) already close — one build serves all of them plus this
worker-backend path.
