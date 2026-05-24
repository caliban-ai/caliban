# ADR 0037 · Sub-agent worktree isolation + background fleet

- **Status:** proposed
- **Date:** 2026-05-24
- **Spec:** `docs/superpowers/specs/2026-05-24-subagent-worktree-and-fleet-design.md`
- **Builds on:** ADR 0021 (sub-agent primitive), ADR 0024 (hook taxonomy)
- **Author:** john.ford2002@gmail.com

## Context

ADR 0021 shipped `AgentTool` as an in-process, foreground, recursion-
guarded primitive. That covers the simple "spawn a read-only Grep/Read
subagent and inline its summary" use case Claude Code uses for parallel
research. It does **not** cover:

- **Filesystem isolation** — a sub-agent that *writes* files shares the
  parent's working tree, so Edit/Write side-effects mix into the parent's
  diff and there is no clean way to discard them.
- **Long-running detached work** — the parent's turn budget is the
  sub-agent's wall-clock budget; nothing survives the parent run ending.
  Claude Code's `--bg`, `claude agents list / attach / respawn / rm`
  surface a fleet of detachable sub-agents we have no equivalent for.
- **Hook inheritance** — deferred from PR #9 / ADR 0024. Child sub-agents
  currently get a brand-new hook stack; flow-scoped hooks the parent set
  up are silently dropped.

These three concerns share state (the spawn site, the lifecycle ownership,
the working-directory model) and want to be solved together. This ADR
records the architectural commitments; mechanics live in the design spec.

## Decision

### Isolation is opt-in per sub-agent, via frontmatter or call-site

Two modes only — `none` (today's behavior, default) and `worktree`. A
`worktree` sub-agent runs in a dedicated `git worktree` materialized under
`.caliban/worktrees/<name>` with a configurable `base_ref` (`fresh` /
`head` / named ref), optional `sparse_paths`, and optional
`symlink_directories` (so heavy build outputs like `target/` and
`node_modules/` are shared by symlink instead of duplicated).

We pick git-worktree over copy-on-write filesystems or chroots because it
works everywhere git works, it is a primitive the user already
understands, and it composes with the rest of git (a sub-agent's diff is
a real branch tip the user can inspect). Containers and OS sandboxes are
orthogonal layers that can wrap a worktree later.

### Background sub-agents are owned by a new `caliban-supervisor` daemon

`bg = true` (frontmatter or runtime override) detaches the sub-agent from
its caller. The detached agent's lifecycle is managed by a per-repo
daemon (`caliband`) auto-spawned on first need. The daemon owns a control
Unix socket (`list/attach/kill/respawn/rm/spawn/status`) and exposes a
per-agent socket each sub-agent writes its `TurnEvent` stream to.

We pick a separate daemon process — not a tokio task inside the main
CLI — because (a) the parent CLI process should be free to exit and let
background sub-agents keep running, and (b) it cleanly separates
short-lived foreground concerns from long-lived fleet concerns. We pick a
Unix domain socket over TCP because the fleet is local-only by design;
TCP exposure waits for a remote-orchestration ADR.

### Per-agent on-disk store is `caliban-sessions`-compatible

A background sub-agent's `<base>/agents/<id>/session.json` is a regular
caliban session file. `caliban agents attach <id>` is sugar for
`caliban resume <id>` over the agent's socket. Reusing the format means
session tooling (compaction, replay, audit) works on background sub-
agents for free.

### `Ctrl+B` is a runtime transition, not a new spawn

A foreground sub-agent can be backgrounded mid-run by snapshotting its
state and transferring ownership to the supervisor. The parent's
in-flight `AgentTool::invoke` future is cancelled with a
`ToolError::Backgrounded(id)` and the assistant transcript records the
handoff. The sub-agent itself sees no state change — it continues from
the next event. This is the operator's escape hatch for "this is taking
longer than I thought; let me get my main loop back."

### Hook inheritance defaults to `true`, with an explicit opt-out

Closes the deferred follow-up from ADR 0024 PR #9. Children inherit
the parent's `Hooks` chain by default; `inherit_hooks: false` in
frontmatter resets to the binary's default chain. For background sub-
agents, only the *serializable* portion of the parent chain
(`HookRouter` config + identified in-process hooks) crosses the process
boundary; opaque closures are stripped with a loud warning. This trades
some correctness for a tractable contract — operators who want full
inheritance keep their background sub-agents foreground until their
hooks are config-expressible.

### Worktree cleanup defaults to `true`

Foreground worktrees are removed when the sub-agent's `WorktreeHandle`
drops. `CALIBAN_KEEP_WORKTREES=1` (and per-call `keep_on_exit: true`)
disable removal for debugging. Background worktrees are owned by the
supervisor and removed on `caliban agents rm <id>` (and on daemon
startup, for orphans, when configured). This is deliberately aggressive:
worktrees are cheap to recreate and expensive to leak.

## Consequences

- **Positive.** Closes four 🔴 rows under matrix G — worktree isolation,
  background sub-agents, subagent-local memory dir, hook inheritance —
  and adds the supervisor daemon row as a new ✅. Unblocks the
  "long-running code-review subagent" and "parallel exploratory
  refactor" workflows that Claude Code uses heavily. Establishes the
  daemon substrate other features can borrow (notably a future
  `caliban serve` HTTP shim for headless use).
- **Negative.** Two new crates and a new binary (`caliband`). The
  per-repo daemon model means cross-repo agent management requires
  multiple daemons; we accept this for v1. Hook inheritance for
  background sub-agents is partial by design (closure hooks dropped).
  Disk usage grows with sparse + symlink-shared worktrees, but the
  default fresh-empty base_ref keeps the floor low. Windows symlink
  requirements (elevation / dev mode) make worktree isolation a
  best-effort feature there.
- **Revisit if:** Disk pressure from worktrees becomes a recurring
  operator complaint — promote a "shared object store" layout
  (`git worktree --no-checkout` + targeted materialization). If
  background-agent IPC outgrows length-prefixed bincode, swap to gRPC
  over the same socket. If the no-closure-hook-inheritance compromise
  for background mode bites real users, sketch a serializable-hook IR.
