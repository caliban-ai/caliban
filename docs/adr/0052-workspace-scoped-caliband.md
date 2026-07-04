# ADR 0052 · Workspace-scoped caliband (multi-source workspace + wired per-source worktree isolation)

- **Status:** accepted
- **Date:** 2026-07-04
- **Amends:** ADR 0037 (per-repo daemon identity → workspace identity; wires the worktree isolation 0037 specified but left unrealized)
- **Source:** [`docs/superpowers/plans/2026-07-04-p1-caliband-workspace-scoping.md`](../superpowers/plans/2026-07-04-p1-caliband-workspace-scoping.md) · caliban [#281](https://github.com/caliban-ai/caliban/issues/281) · epic [#274](https://github.com/caliban-ai/caliban/issues/274). Originating design is the cross-repo k8s system-design spec in the caliban-ai docs hub (§"CalibanTask CR + workspace model", §"caliban changes"), not in this repo.

## Context

ADR 0037 made caliband a **per-repo** daemon: its identity is `hash(repo_root)`
(`crates/caliban-supervisor/src/runtime.rs`), one control socket per repo, and it
explicitly accepted that "cross-repo agent management requires multiple daemons …
for v1." A k8s pod, however, hosts a **workspace** — a directory holding N repo
checkouts ("sources"), e.g. `/work/{caliban,gonzalo,prospero}` — so a single task
can span repos (cross-system integration / e2e). The k8s design spec makes the
schedulable unit a workspace of 1..N sources (`{name, repo, ref, path}`), rooted by
one caliband, with **worktree isolation per source**, and the source set
**runtime-extensible** (a running agent can `git clone` a new source into the live
volume). This ticket (#281) generalizes caliband's identity/discovery from per-repo
to per-workspace.

Mapping the current code surfaced two facts that shape the decision:

1. **Single-repo is baked into identity + store + discovery, not the protocol.**
   `repo_hash(repo_root)` → one `.sock`; `AgentStore::default_for(repo_root)` keys
   the store by one path; `caliband --repo-root` is single + required; the CLI's
   `discover_repo_root` walks up to one `.git`. None of the wire types
   (`CtlRequest`/`CtlReply`, `SpawnSpec`) carry a repo/source/working-dir field, and
   the launcher (`ExecWorkerLauncher::launch`) sets **no** `current_dir` — so an
   agent's working directory is silently inherited from the daemon's process cwd.
   There is no way today to say *which checkout* an agent runs against.

2. **Per-source worktree isolation is specified but unrealized.** ADR 0037
   described worktree-isolated sub-agents, and `SpawnSpec.isolation_worktree: bool`
   is persisted, but `crates/caliban-worktrees::WorktreeManager` has **no runtime
   consumer** — the flag is never read to create a worktree, in any mode. So
   "worktree isolation remains per-source" cannot mean "keep the existing wiring";
   there is none to keep.

No prospero coupling exists on the caliban side: prospero's own path-hash
(`discovery.rs::hash16`) only matters for *local* discovery of a caliband socket,
which the k8s / `FleetProvider` model sidesteps (endpoints come from `CalibanTask`
CRs, not a re-derived hash). So #281 is a **caliban-only** change; keeping the two
repos' hash rules in sync is deferred to whenever prospero's local discovery needs
workspace awareness.

## Decision

We will make caliband **workspace-scoped**, and — because #281's acceptance
("per-source worktrees still isolate writes") cannot rest on dead code — we will
**wire the per-source worktree materialization** that 0037 specified.

1. **Workspace identity.** Rename the daemon's rooting concept from a repo to a
   **workspace root**: `caliband --workspace-root <dir>` becomes canonical, with
   `--repo-root` accepted as a back-compat alias. `repo_hash` generalizes to
   `workspace_hash` (same implementation — it hashes a directory path), and the
   socket + `AgentStore` are keyed by the workspace root. A local single-repo
   workspace's root equals today's repo_root, so its hash — and therefore its
   socket path and store dir — are **unchanged**: existing local daemons and the
   CLI keep working with no migration.

2. **Sources = auto-discovered child checkouts.** A **source** is a git checkout
   under the workspace root. caliband resolves its sources by scanning the
   workspace root for children containing `.git` (plus the root itself when it is a
   checkout, for single-source back-compat). Discovery is on-demand, not a fixed
   provisioned list, so a source `git clone`d into the live volume at runtime is
   visible on the next resolution — matching the spec's dynamic-extension
   requirement without a daemon restart or a `--source` registry.

3. **Per-source addressing.** `SpawnSpec` gains `source: Option<String>` (a source
   name / workspace-relative path). At spawn the daemon resolves it to the source's
   absolute directory; `None` means the workspace root (single-source back-compat).
   The resolved working directory is recorded on `AgentRecord` so the launcher and
   the exit-cleanup path can see it.

4. **Wired per-source worktree isolation.** When `spec.isolation_worktree` is set,
   the daemon creates a git worktree via a `WorktreeManager` rooted at the resolved
   **source** (`<source>/.caliban/worktrees/<agent>`), records that worktree as the
   agent's working directory, and removes it when the worker exits — mirroring the
   per-agent socket lifecycle already in `launch_and_monitor`. When the flag is
   unset, the working directory is the source dir itself. The launcher
   (`ExecWorkerLauncher::launch`) sets the worker's `current_dir` to that recorded
   working directory — closing the "cwd inherited from the daemon" gap. A source
   that is not a git checkout is a hard error when isolation is requested.

The NDJSON protocol is otherwise unchanged; this is an identity/discovery + spawn
generalization plus the worktree wiring, not a wire-format change (`SpawnSpec` gains
one optional field, back-compatible via `#[serde(default)]`).

## Consequences

- **Positive:** one caliband supervises agents across ≥2 sources in a single
  workspace (the #281 acceptance), each agent runs in the correct checkout, and
  `isolation_worktree` finally materializes a real per-source worktree — a
  capability 0037 promised but never delivered, now available in local **and**
  in-pod modes. Local single-repo use is byte-for-byte unchanged (same hash, same
  socket, same store). Runtime-added sources need no daemon restart.
- **Negative:** the daemon now owns git-worktree lifecycle (create-on-spawn,
  remove-on-exit) — more state and more failure surface (a source that isn't a
  checkout, a worktree that fails to create, orphaned worktrees on a hard crash,
  mirroring the existing orphaned-socket risk). `SpawnSpec` grows a field and
  `AgentRecord` grows a working-dir, a store-format addition (back-compatible via
  serde defaults). Auto-discovery scans the workspace dir on resolution — cheap,
  but assumes children-with-`.git` is the source convention.
- **Revisit if:** we need sources that are not direct children of the workspace
  root, non-git sources, or a provisioned source registry distinct from what's on
  disk (e.g. sources declared in the CR but not yet cloned) — at which point a
  first-class source manifest (fed by the operator from `spec.workspace.sources`)
  supersedes on-disk auto-discovery. Also revisit the deferred caliban/prospero
  hash-rule duplication if prospero's local discovery ever needs workspace
  awareness.
