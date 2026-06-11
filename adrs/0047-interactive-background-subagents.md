# ADR 0047 · Interactive background sub-agents (idle / await-input)

- **Status:** accepted
- **Date:** 2026-06-10
- **Spec:** `docs/superpowers/specs/2026-06-10-interactive-background-subagents-design.md`
- **Amends:** ADR 0037 (sub-agent worktree isolation + background fleet) — revises one non-goal clause; see "Decision".
- **Builds on:** ADR 0009 (agent-core stream-as-primitive), ADR 0024 (hook taxonomy), ADR 0037 (background fleet + per-agent socket).
- **Author:** john.ford2002@gmail.com
- **Issue:** caliban-ai/caliban#81

## Context

ADR 0037 shipped the background fleet: `bg = true` sub-agents owned by the
`caliband` daemon, each exposing a per-agent socket carrying its `TurnEvent`
stream. Issues #71 / #78 / #79 / #75 / #76 / #77 implemented that runtime —
workers launch, stream their transcript live over `caliban agents attach`,
clean up on exit, and run behind a permission gate.

ADR 0037 deliberately scoped **inbound** interaction out. Its non-goals say:

> **Re-attaching a stopped sub-agent into the parent's context.** Once
> detached, a background sub-agent runs to completion (or is killed). The
> parent reads its final summary via `caliban agents attach` or the
> `/agents` overlay.

and its design spec describes the per-agent socket as carrying
"`TurnEvent`s **and inbound user messages**" — a capability that was
documented but never built. The result today: an attached operator can
*watch* a background agent but cannot *talk* to it. When the agent finishes
its prompt, it ends; there is no way to say "good, now also do X" without
`respawn` (which loses all context).

Two facts make this worth revisiting now:

1. **It is a small generalization, not a rewrite.** The agent loop already
   has `TurnDecision::ContinueWith(Vec<Message>)` (ADR 0024 hook taxonomy):
   an `after_turn` hook can inject messages and force another turn. That is
   exactly "resume a finished turn with new input" — capped at
   `MAX_FORCED_CONTINUATIONS = 3` only to stop *hook* death-spirals.
2. **The fleet UX expects it.** `AgentStatus::Idle` ("awaiting input; no
   compute pending") is defined in the proto and rendered by `agents list`
   but is never set, because nothing awaits input.

This ADR records the architectural commitments for closing that gap.
Mechanics live in the companion design spec.

## Decision

### Revise ADR 0037's "runs to completion" non-goal

ADR 0037's non-goal is **narrowed**, not deleted:

- **Still a non-goal:** re-attaching a sub-agent into the **parent agent's
  automated context**. A `bg` sub-agent never feeds results back into the
  parent's running loop; the parent reads a final summary out-of-band. This
  ADR does not change that.
- **Now permitted:** an **operator** (a human at `caliban agents attach`)
  may send user messages to a *running* background sub-agent, which resumes
  from that input rather than ending. This is interactive operator I/O over
  the per-agent socket — categorically different from automated
  parent-context re-attachment.

The distinction matters: the danger ADR 0037 guarded against was *automated*
fan-in (a sub-agent silently resuming the parent). A human typing into an
attached session carries no such hazard and is the natural way to steer a
long-running background task.

### Interactivity is a first-class agent-core run mode, not a hook hack

We add an optional **`InputProvider`** to a run (via `RunSettings`), rather
than overloading `after_turn` + `ContinueWith`. When the model reaches a
natural end-of-run boundary (it stopped and no tool call is pending), the
loop — *if an `InputProvider` is configured* — awaits the provider for the
next user message:

- `Some(messages)` → inject into history, mark **Idle → Running**, take
  another turn.
- `None` → the provider signalled end-of-input; the run ends normally
  (`StopCondition::EndOfTurn`, status `Done`).

We choose a pull-based `InputProvider` over the existing
`ContinueWith` hook path because:
- **It is not death-spiral-prone**, so it is correctly **uncapped** (a human
  drives it; `MAX_FORCED_CONTINUATIONS` stays as the anti-spiral cap for
  *hook*-forced continuations only).
- **It models "await input" honestly** — the loop blocks on external I/O at
  a well-defined boundary, which `after_turn` (fires every turn) does not.
- **It composes with hooks** — `before_turn`/`after_turn`/permission hooks
  still run on the resumed turns unchanged.

Foreground and one-shot runs pass no `InputProvider` and are byte-for-byte
unchanged (the boundary check is `if let Some(provider)`).

### The per-agent socket becomes bidirectional

ADR 0037's per-agent socket carried worker→client `TurnEvent`s only (#79).
It becomes **bidirectional**: the worker continues writing `TurnEvent` NDJSON
outbound, and now reads **inbound user-message frames** (newline-delimited
JSON, a small tagged frame type) from attached clients. The worker's
`InputProvider` is fed by these frames. `caliban agents attach` gains a send
path (stdin → user-message frames). Read-only viewers (e.g. a future
`/agents` overlay tail) simply never send.

### Idle is a real, reported lifecycle state

`AgentStatus::Idle` is wired: the worker reports **Running → Idle** when it
begins awaiting input and **Idle → Running** when it resumes. Because the
daemon — not the worker — owns the registry, this requires a **worker →
daemon status-report channel** (the worker currently only talks to attach
clients). The design spec picks the mechanism; the commitment here is that
Idle is observable in `agents list` and the `/agents` overlay.

### Bounded idle: an idle agent must not live forever

An agent awaiting input with **no attached clients** is a resource leak in
waiting. The run ends (`Done`) when any of:
- the `InputProvider` returns `None` (an attached operator sent an explicit
  end / detached with end-intent),
- a configurable **idle timeout** elapses with no inbound message and no
  attached client, or
- `caliban agents kill` (unchanged).

Default idle timeout is conservative (minutes, configurable per
`SupervisorConfig`); the spec sets the exact default.

## Consequences

- **Positive.** Closes the last documented gap in the ADR 0037 per-agent
  socket ("inbound user messages"). Turns background sub-agents from
  fire-and-forget into steerable long-running workers — the natural UX for
  "kick off a background refactor, watch it, nudge it." Reuses the audited
  permission gate (#75) on resumed turns. Wires the long-dormant `Idle`
  state. The `InputProvider` abstraction is reusable beyond background
  agents (e.g. a future scripted multi-turn driver).
- **Negative.** A new first-class run mode in agent-core (small, but it
  touches the core loop's end-of-run boundary — the highest-blast-radius
  file in the codebase). A worker→daemon status channel that did not exist.
  A bidirectional socket protocol (frame schema, multi-client inbound
  multiplexing). The idle-timeout adds a timer to the worker. None of these
  affect foreground/headless runs.
- **Revisit if:** multi-client inbound proves confusing (two operators
  typing at one agent) — may need a single-writer lease. If `InputProvider`
  wants richer turns than user text (images, tool results), generalize the
  inbound frame. If operators want to *fork* an idle agent's context rather
  than continue it, that is a separate "branch" primitive, out of scope.

## Decomposition (see spec for detail)

This ADR is intentionally larger than one PR. The spec breaks it into
independently-shippable tickets:
1. agent-core `InputProvider` run mode (+ tests; foreground unaffected).
2. Bidirectional per-agent socket frame protocol + worker `InputProvider`
   backed by the socket.
3. `caliban agents attach` send path (stdin → frames; end/detach semantics).
4. Worker → daemon status reporting + `AgentStatus::Idle` wiring.
5. Idle timeout + bounded-lifetime cleanup.
