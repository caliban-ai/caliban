# ADR 0057 · Graceful drain, checkpoint, and resume for daemon-managed agents

- **Status:** accepted
- **Date:** 2026-09-13
- **Source:** caliban [#286](https://github.com/caliban-ai/caliban/issues/286)
  (operator drain finalizer + idle pause/resume), part of the Kubernetes epic
  [#274](https://github.com/caliban-ai/caliban/issues/274). Builds on the
  network control plane of [ADR 0051](0051-caliband-network-transport.md), the
  workspace-scoped daemon of [ADR 0052](0052-workspace-scoped-caliband.md), the
  background sub-agent fleet of [ADR 0037](0037-subagent-isolation-and-background-fleet.md)
  / [ADR 0047](0047-interactive-background-subagents.md), and the file
  checkpoint store of [ADR 0028](0028-checkpointing-rewind.md).

## Context

The k8s epic (#274) runs each `CalibanTask`'s caliband in a `Sandbox` pod. Two
lifecycle events must not destroy in-progress work:

- **Delete / teardown.** Deleting a `CalibanTask` deletes its `Sandbox`, which
  kills the caliband pod and every agent it manages. Today the only stop path
  is `CtlRequest::Kill` (SIGTERM escalating to SIGKILL, `server.rs`) — abrupt,
  with no guarantee the agents' state is durable first.
- **Idle pause.** A `CalibanTask` that sits idle should release its compute
  (suspend the pod) and resume later, rather than burn a running pod forever.

The operator CRD **already declares** the intent — `spec.lifecycle.idleTimeout`,
`spec.lifecycle.onDelete` (`checkpoint` | `delete`), `status.checkpointRef`, and
a `Draining` phase (`caliban-operator/src/crd.rs`) — but **no controller logic
acts on any of it**: there is no finalizer, no idle→pause reconcile, and
`checkpointRef` is never written. On the daemon side there is likewise no
graceful-drain or resume path:

- `CtlRequest` has `List/Spawn/Attach/Kill/Respawn/Rm/Status/Shutdown/ReportStatus`
  — **no `Drain`, no `Pause`, no `Resume`** (`crates/caliban-supervisor/src/proto.rs`).
- `SpawnSpec` always starts an agent **fresh** from `initial_prompt`; it carries
  no way to continue a persisted conversation.
- `caliban-supervisor` has **no checkpoint code**. The file checkpoint store
  (ADR 0028) was wired only into the interactive TUI (#549); it snapshots the
  *working tree per prompt*, not a daemon agent's conversation.

What *does* exist to build on:

- Each agent has a durable **session directory** on disk
  (`crates/caliban-supervisor/src/store.rs`). Today the daemon worker writes
  only an append-flushed `stdout.ndjson` (`TurnEvent` transcript) there — it
  does **not** yet persist a resumable `session.json` (caliban-sessions format).
  That resumable-session persistence is introduced with resume-from-session
  (#651); it is the state a `Drain` checkpoint flushes. (A pre-existing doc
  comment claimed the runtime already wrote `session.json`; it does not — this
  ADR corrects the record and #651 makes it true.)
- The `Sandbox` mounts a **retained workspace PVC** that survives pod restarts
  (`caliban-operator/src/resources.rs`), and the `Sandbox` already models a
  `Suspended` state (`sandbox.rs`).

So the durable state needed for resume is *already written to disk on a volume
that outlives the pod* — what is missing is (a) a graceful **flush-and-record**
step, (b) a way to **resume** an agent from that state, and (c) the operator
**lifecycle glue** that invokes them.

Options weighed:

- **(A) Drain / checkpoint / resume over the existing session-dir + PVC.**
  Treat the flushed session directory on the retained PVC as the checkpoint.
  Add a graceful `Drain` control verb that stops agents *after* guaranteeing
  their session state is flushed and returns a resume reference per agent; add
  a resume path that re-spawns an agent continuing its persisted session; have
  the operator finalizer call `Drain` before teardown and resume on re-activation.
- **(B) Process-level snapshot/restore** (CRIU-style memory snapshot of the
  worker). Rejected: heavyweight, brittle across kernels/images, and
  unnecessary — the agent's meaningful state is its conversation, already on disk.
- **(C) Do nothing new; rely on re-attach to the session store.** Rejected: no
  graceful-flush guarantee (an abrupt Kill can lose the last turn), no operator
  lifecycle, and `checkpointRef`/`idleTimeout` stay decorative.

## Decision

Adopt **Option A**. Concretely, across two repos:

**caliband (`caliban-supervisor`):**

1. **`Drain` control command.** A new `CtlRequest::Drain { grace }` gracefully
   stops every managed agent — SIGTERM (not SIGKILL) so the worker flushes its
   durable outputs and exits cleanly within `grace` — and replies with a
   per-agent **resume reference** (the session-directory path). `Drain` is
   semantically distinct from `Kill`: `Kill` is abrupt termination; `Drain` is
   "checkpoint, then stop." The resumable `session.json` that a drain flushes is
   written by the worker as part of resume-from-session (#651); until that lands
   the drained session dir carries only the transcript, so #650 delivers the
   graceful-stop + resume-ref protocol and #651 fills in the resumable state.
2. **Resume from a persisted session.** A daemon agent can be re-spawned
   *continuing* a persisted session rather than starting fresh — via a resume
   reference on the spawn path (`SpawnSpec` gains a `resume_session` pointer, or
   a dedicated `Resume { session_dir }` verb; the implementing ticket picks the
   exact shape). The `__agent-worker` entry point gains resume-from-session
   support mirroring the main CLI's `--resume`.
3. **The checkpoint *is* the flushed session directory on the retained PVC** — a
   durable, content-complete session dir plus a reference to it, **not** a memory
   snapshot. ADR 0028 file-tree checkpoints **compose** with this (a resume can
   also restore files) but are **not required** for conversation resume.

**caliban-operator:**

4. **Drain finalizer.** The `CalibanTask` controller adds a finalizer. On
   delete, if `spec.lifecycle.onDelete == checkpoint`, it (a) drives the task to
   the `Draining` phase, (b) calls caliband `Drain` over the control plane
   (ADR 0051 TLS+token), (c) records the resume reference in
   `status.checkpointRef`, then (d) removes the finalizer so the `Sandbox` (and
   its retained PVC) can be torn down with the checkpoint intact. `onDelete:
   delete` skips the checkpoint and deletes directly.
5. **`idleTimeout` → Sandbox suspend (pause).** When every agent has reported
   `Idle` (ADR 0047 status reporting) for `spec.lifecycle.idleTimeout`, the
   controller `Drain`s and **suspends** the `Sandbox` (releasing compute, keeping
   the PVC).
6. **Resume on re-activation.** Re-activating a suspended/checkpointed task
   un-suspends the `Sandbox`; caliband resumes its agents from
   `status.checkpointRef` on the PVC.

This is decomposed for implementation into: caliban —
[#650](https://github.com/caliban-ai/caliban/issues/650) (the `Drain` command +
worker flush guarantee) and
[#651](https://github.com/caliban-ai/caliban/issues/651) (resume-from-session on
the spawn path); and caliban-operator —
[#42](https://github.com/caliban-ai/caliban-operator/issues/42) (the drain
finalizer + `checkpointRef`) and
[#43](https://github.com/caliban-ai/caliban-operator/issues/43) (the
`idleTimeout`→suspend + resume-on-attach reconcile). #286 becomes the tracking
epic.

## Consequences

- **Positive:** graceful teardown and idle pause no longer lose work; resume
  restores an agent's conversation on a fresh pod. Reuses machinery that already
  exists (session-dir persistence, the retained PVC, the `Suspended` sandbox
  state, the ADR 0051 control plane), so the protocol additions are small and
  the operator work is glue, not new storage. `checkpointRef`/`idleTimeout`/
  `onDelete` stop being decorative.
- **Negative / limits:** the checkpoint is only as durable as **PVC retention** —
  a reclaimed volume loses it. Resume replays from the **persisted conversation**,
  not live process state: a tool call in flight at drain time is **not** resumed
  mid-execution (the drain either lets it finish within `grace` or it is dropped
  and the turn re-run on resume). The operator must reach the control plane to
  drain, adding a drain-timeout failure mode (a finalizer that cannot drain must
  fall back to a bounded force-delete rather than wedging deletion forever).
- **Revisit if:** we need mid-tool-call resumption, cross-node/cross-cluster
  migration of a live agent, or checkpoint durability independent of the
  workspace PVC (e.g. object-store-backed checkpoints) — at which point a richer
  snapshot format (closer to Option B, or a gonzalo-backed checkpoint store)
  would supersede the "flushed session dir on the PVC" definition here.

_Historical note (dependency-free): the operator CRD shipped the
`lifecycle`/`checkpointRef` schema ahead of any controller behavior; this ADR is
what gives those fields meaning. The Draining phase and `Suspended` sandbox
state predate it and are reused unchanged._
