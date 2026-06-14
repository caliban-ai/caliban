# Interactive background sub-agents (idle / await-input) — Design

**Date:** 2026-06-10
**Status:** Proposed
**Author:** john.ford2002@gmail.com
**Companion ADR:** `docs/adr/0047-interactive-background-subagents.md`
**Amends:** ADR 0037 (background fleet) — the "runs to completion" non-goal.
**Issue:** caliban-ai/caliban#81

## Goal

Let a human operator at `caliban agents attach <id>` **send messages to a
running background sub-agent**, which resumes from that input instead of
ending. Today the per-agent socket is outbound-only (#79): you can watch a
background agent's transcript but not talk to it. This design makes the
socket bidirectional, adds a first-class "await input" run mode to
agent-core, and wires the dormant `AgentStatus::Idle` state.

## Non-goals

- **Automated parent-context re-attachment.** A `bg` sub-agent never feeds
  results back into the *parent agent's* loop. This remains a non-goal
  (ADR 0037, reaffirmed by ADR 0047). Only a human operator drives inbound.
- **Forking / branching an idle agent.** Continuing an idle agent's context
  is in scope; creating a divergent branch from it is a separate primitive.
- **Inbound richer than user text (v1).** The inbound frame carries user
  text (and, trivially, multiple messages). Images / synthetic tool results
  are a later generalization of the frame.
- **Multi-writer coordination (v1).** If two operators attach and both type,
  messages interleave in arrival order. A single-writer lease is a possible
  follow-up, not v1.

## Background: what already exists

- **`Agent::stream_until_done(messages, cancel)`** runs from a fixed
  `Vec<Message>` to completion (`crates/caliban-agent-core/src/stream/mod.rs`).
  The loop is `'outer: for turn_index in 0..max_turns`; a turn whose
  `stop_reason != ToolUse` ends the run unless a hook forces continuation.
- **`TurnDecision::ContinueWith(Vec<Message>)`** (`hooks.rs`): an `after_turn`
  hook can inject messages and force another turn, capped at
  `MAX_FORCED_CONTINUATIONS = 3` (anti hook-death-spiral). This is the
  existing precedent for mid-run injection; the new `InputProvider`
  generalizes it for *external* (human) input, uncapped.
- **Per-agent socket** (`caliban/src/worker.rs`): the worker binds it and,
  via the `EventHub` (#79), replays history then streams live `TurnEvent`
  NDJSON to each attached client (`serve_attach_client`). Currently it never
  reads from the client.
- **`AgentStatus::Idle`** (`crates/caliban-supervisor/src/proto.rs`):
  defined, rendered by `agents list` (`agents_cli.rs::fmt_status`), never
  set. The daemon owns the registry; the worker has no channel to report
  status — it only writes the manifest at spawn and talks to attach clients.
- **Permission gate (#75)** is attached to the worker agent via `.hooks()`;
  it must keep gating resumed turns (it does, automatically — hooks run on
  every turn).

## Architecture

```
  caliban agents attach <id>  ──────────────┐  (bidirectional per-agent socket)
    stdin ─▶ UserMessage frames ───────────▶│
    stdout ◀─ TurnEvent NDJSON ◀────────────│
                                            ▼
                          ┌──────────────────────────────────┐
                          │  caliban __agent-worker            │
                          │   serve_attach_client (per conn):  │
                          │     • outbound: EventHub tail (#79)│
                          │     • inbound: parse UserMessage   │
                          │       frames → InboundInbox (mpsc) │
                          │                                    │
                          │   Agent::stream_until_done_with_   │
                          │     settings(.. input_source ..)   │
                          │     end-of-run boundary:           │
                          │       await InputProvider:         │
                          │         Some(msgs) → continue      │
                          │         None      → end (Done)     │
                          │     reports Idle/Running ──────────┼──▶ caliband
                          └──────────────────────────────────┘    (status channel)
```

`InboundInbox` is a worker-internal mpsc shared by all attach connections
(producers) and the `InputProvider` (consumer). The `InputProvider` is what
agent-core awaits at the end-of-run boundary.

## Part 1 — agent-core: the `InputProvider` run mode

### Trait

```rust
// crates/caliban-agent-core/src/stream/mod.rs (or a new input.rs module)

/// Supplies additional user input to a run that would otherwise end.
/// Implementations block until input is available or end-of-input is
/// signalled.
#[async_trait::async_trait]
pub trait InputProvider: Send + Sync {
    /// Await the next batch of user messages to inject, or `None` to end
    /// the run. The loop calls this at the end-of-run boundary (the model
    /// stopped and no tool call is pending). May be cancelled via the run's
    /// `CancellationToken` — implementations should select on cancellation.
    async fn next_input(&self) -> Option<Vec<caliban_provider::Message>>;
}
```

### Wiring into `RunSettings`

`RunSettings` already exists (`stream/mod.rs`) and is threaded through
`stream_until_done_with_settings`. Add:

```rust
pub struct RunSettings {
    // ... existing fields (session_id, workspace_root, prompt_index) ...
    /// Optional interactive input source. When set, the loop awaits this at
    /// the natural end-of-run boundary instead of ending. `None` (default)
    /// preserves today's run-to-completion behavior exactly.
    pub input_source: Option<std::sync::Arc<dyn InputProvider>>,
}
```

`stream_until_done(messages, cancel)` keeps its signature and passes
`RunSettings::default()` (input_source `None`) — **foreground/headless/
one-shot are unchanged**.

### The end-of-run boundary

Today, when a turn ends with `stop_reason != ToolUse` and no hook continues,
the loop falls out and yields `RunEnd`. Insert the await *at that boundary*:

```
after a turn completes, when the run WOULD end (model stopped, no pending
tool call, after_turn did not ContinueWith):
    if let Some(provider) = &settings.input_source {
        // mark Idle (status hook — see Part 4), then:
        match select(provider.next_input(), cancel.cancelled()) {
            input.next_input -> Some(msgs) => {
                history.extend(msgs);
                // NOT counted against MAX_FORCED_CONTINUATIONS — human-driven.
                // mark Running; emit a synthetic TurnEvent? (see "Events")
                continue 'outer;   // take another turn
            }
            Some(None) | cancel => { /* fall through to RunEnd */ }
        }
    }
    // else: end the run exactly as today.
```

Precise integration points (for the implementer): the existing
`after_turn` decision block (`stream/mod.rs` ~line 1304, the
`TurnDecision` match) and the `continue_loop` / natural-end path are where
the boundary lives. The await must happen **after** `after_turn` (so hooks
still observe the turn) and **only** when the run would otherwise terminate
with `EndOfTurn` (not on failure/cancel/max-turns/hook-stop — those end
immediately regardless of an input source).

### Events

When the loop resumes from injected input, the injected user message(s)
should appear in the transcript. Options (spec decision):
- **(chosen)** Emit a `TurnEvent` for the injected user message so attach
  clients and `stdout.ndjson` show it. Either reuse a generic event or add a
  small `TurnEvent::UserMessage { text }` variant. Adding a variant is a
  one-line serde change (TurnEvent already derives serde, #78) and is the
  honest representation. The `_ => {}` arms in existing renderers ignore
  unknown variants, so it is backward-compatible.

### Tests (agent-core, no LLM)

Use the existing mock provider (`caliban-provider` `mock` feature, seen in
provider tests) to script a model that "ends" after one turn:
1. `input_provider_none_ends_run`: provider returns `None` immediately →
   run ends with `EndOfTurn` after the first turn (same as no provider).
2. `input_provider_resumes_then_ends`: provider returns `Some([user "go"])`
   once, then `None` → the loop takes a second turn with the injected
   message, then ends. Assert two `TurnStart`s and the injected message in
   history.
3. `input_provider_not_capped`: provider returns `Some(..)` >
   `MAX_FORCED_CONTINUATIONS` times then `None` → the run takes >3 extra
   turns (proves human input is uncapped, unlike hook `ContinueWith`).
4. `no_input_source_is_unchanged`: a run with `input_source: None` behaves
   identically to `stream_until_done` (regression guard).

## Part 2 — bidirectional per-agent socket + worker `InputProvider`

### Inbound frame schema

Newline-delimited JSON, one frame per line, mirroring the outbound
`#[serde(tag = "type")]` style:

```rust
// shared location (caliban crate or caliban-supervisor proto)
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum AttachInbound {
    /// Operator sends a user message to inject.
    UserMessage { text: String },
    /// Operator signals end-of-input: the agent should finish after the
    /// current/next boundary (InputProvider returns None).
    EndInput,
}
```

Outbound stays `TurnEvent` NDJSON (#78/#79), so a socket reader can tell the
directions apart by reader role (the client writes `AttachInbound`, the
worker writes `TurnEvent`). They never collide on one half-duplex direction.

### Worker `serve_attach_client` (extended)

Today it only writes. Make each connection full-duplex:
- **Write half:** unchanged — EventHub history replay + live tail.
- **Read half:** a task reading newline frames; each `UserMessage`/`EndInput`
  is forwarded to a worker-global `InboundInbox` (an `mpsc::Sender`). All
  attach connections share one inbox (multiplexed; v1 interleaves by arrival).

### Worker `InputProvider`

```rust
struct SocketInputProvider {
    inbox: tokio::sync::Mutex<mpsc::Receiver<AttachInbound>>,
    idle_timeout: Duration,
    has_clients: Arc<AtomicUsize>, // attach connection count
    status: WorkerStatusReporter,  // Part 4
}

impl InputProvider for SocketInputProvider {
    async fn next_input(&self) -> Option<Vec<Message>> {
        self.status.set_idle();           // Running -> Idle
        let mut rx = self.inbox.lock().await;
        loop {
            select! {
                frame = rx.recv() => match frame {
                    Some(UserMessage { text }) => {
                        self.status.set_running();   // Idle -> Running
                        return Some(vec![Message::user_text(text)]);
                    }
                    Some(EndInput) | None => return None,   // end the run
                }
                _ = sleep(self.idle_timeout), if self.has_clients.load() == 0 => {
                    return None;   // bounded idle: no clients, timed out
                }
            }
        }
    }
}
```

The worker builds this provider and passes it via
`RunSettings { input_source: Some(Arc::new(provider)), .. }` to
`stream_until_done_with_settings`. The worker only enables interactive mode
when appropriate (see "When is a worker interactive?").

### When is a worker interactive?

Not every background agent should idle-await — a fire-and-forget
`caliban --bg "fix the lint"` should still run to completion and exit.
Interactivity is **opt-in** via a new `SpawnSpec` field:

```rust
// caliban-supervisor proto SpawnSpec
#[serde(default)]
pub interactive: bool,   // default false = today's run-to-completion
```

Populated by the spawner (e.g. `caliban agents spawn --interactive` or a
frontmatter `interactive: true`). When `false`, the worker passes no
`InputProvider` and behaves exactly as today.

## Part 3 — `caliban agents attach` send path

`agents_cli.rs::run_attach` (today read-only, #79) becomes full-duplex:
- **Receive:** unchanged — `attach::stream_attach` renders `TurnEvent`s.
- **Send:** a task reading the operator's **stdin**; each line becomes an
  `AttachInbound::UserMessage { text: line }` frame written to the socket.
- **End/detach semantics:**
  - **Ctrl+C** → detach (close the socket, leave the agent idle/awaiting) —
    today's behavior.
  - **Ctrl+D (stdin EOF)** → send `AttachInbound::EndInput` then detach: the
    operator is done; the agent finishes.
  - A blank line / typed text → `UserMessage`.

Surface a one-line hint on attach: `(type to send · Ctrl+D to finish · Ctrl+C
to detach)`.

`stream_attach` is already factored for unit testing (#79); add a small
testable encoder for stdin→frame and unit-test it with an in-memory writer.

## Part 4 — `AgentStatus::Idle` + worker→daemon status reporting

The daemon owns the registry; the worker must tell it about Running↔Idle.
Today there is no worker→daemon channel. Options (spec decision):

- **(chosen) New control request.** Add
  `CtlRequest::ReportStatus { id, status }` to the supervisor proto; the
  worker opens a short connection to the **control** socket (it already
  knows the repo socket path via env/args at spawn — pass it in) and reports
  transitions. The daemon applies them via `set_status_if_running`-style
  guards (Idle only from Running; Running only from Idle). Pros: authoritative,
  immediate, visible in `agents list`. Cons: the worker needs the control
  socket path (add it to the worker CLI args / manifest) and a tiny client.
- **(rejected) Status marker file** the daemon reads on `list`: simpler but
  makes `list` do I/O per agent and is eventually-consistent.

Wire `fmt_status` already renders Idle; the `/agents` overlay (future) shows
`◐ idle`. The monitor task's terminal-status logic is unchanged (Idle is not
terminal; a worker that exits from Idle still goes Done/Failed via
`child.wait`).

## Part 5 — bounded idle lifetime

Covered by `SocketInputProvider` above: `idle_timeout` with no attached
clients → `next_input` returns `None` → run ends `Done`. Default:
`SupervisorConfig.idle_timeout_secs` (proposed default **300s**), surfaced in
unified settings. `EndInput` and `kill` are the explicit ends. An agent with
clients attached can idle indefinitely (a human is present).

## Decomposition into tickets

Each is independently shippable and reviewable (sized like the #71/#78/#79
PRs):

1. **agent-core `InputProvider` run mode** — trait, `RunSettings.input_source`,
   end-of-run boundary await, optional `TurnEvent::UserMessage`, mock-driven
   tests. Foreground unaffected. *(Highest-blast-radius; do first, in
   isolation.)*
2. **Inbound frame protocol + worker duplex + `SocketInputProvider`** — frame
   enum, `serve_attach_client` read half, `InboundInbox`, `SpawnSpec.interactive`.
3. **`agents attach` send path** — stdin→frames, Ctrl+D=EndInput / Ctrl+C=detach.
4. **Status channel + `AgentStatus::Idle`** — `CtlRequest::ReportStatus`,
   worker reporter, daemon guards, control-socket path plumbing.
5. **Idle timeout + settings** — `idle_timeout_secs`, bounded cleanup.

Suggested order: 1 → 2 → 3 → (4, 5 together). 1 can land behind no user-facing
change; 2+3 make it usable; 4+5 polish lifecycle.

## Risks

- **Blast radius of the core-loop change (Part 1).** `stream/mod.rs` is the
  most central file; the boundary insert must not perturb the
  failure/cancel/max-turns/hook-stop paths. Mit: gate the entire behavior on
  `input_source.is_some()`; a regression test asserts byte-identical behavior
  when `None`; land Part 1 alone.
- **Multi-client inbound confusion.** Two operators typing at once interleave.
  Mit: v1 documents arrival-order interleaving; a single-writer lease is a
  scoped follow-up.
- **Idle leak.** An agent idling forever with no client. Mit: the idle
  timeout (Part 5) is mandatory, not optional.
- **Permission gate on resumed turns.** Resumed turns must stay gated (#75).
  This is automatic (hooks run per turn), but a test should assert an
  ungranted tool is still denied on a *resumed* turn.
- **Status races.** Idle↔Running reporting could race the monitor's terminal
  transition. Mit: the daemon applies status with guards (Idle/Running only
  from each other; terminal states win), same discipline as
  `set_status_if_running`.

## Acceptance criteria

- ADR 0047 accepted; ADR 0037's non-goal annotated as revised.
- `cargo fmt/clippy/build/test` clean across the workspace at each ticket.
- agent-core: a run with no `input_source` is provably unchanged; a run with
  one resumes on `Some` and ends on `None`, uncapped.
- End-to-end (live smoke, `#[ignore]`): `caliban agents spawn --interactive
  --prompt "..."`, `caliban agents attach <id>`, type a follow-up, see the
  agent take another turn; Ctrl+D ends it; `agents list` shows `idle` while
  awaiting.
- A resumed turn is still permission-gated (#75 regression).
