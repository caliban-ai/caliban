# ADR 0055 · Driveable server surface — MCP-server + ACP + headless HTTP serve over one core

- **Status:** accepted
- **Date:** 2026-08-15
- **Source:** epic [#503](https://github.com/caliban-ai/caliban/issues/503) · design spike [#504](https://github.com/caliban-ai/caliban/issues/504). Builds on [0025](0025-headless-output-protocol.md) (headless `-p` NDJSON), [0023](0023-mcp-v2-transports-and-oauth.md) (MCP transports/OAuth), and [0051](0051-caliband-network-transport.md) (caliband NDJSON transport).

## Context

caliban is a first-class MCP *client* but exposes **no server surface**, so nothing
can drive it programmatically. This is the single highest-leverage parity gap in the
competitor readout under `docs/evaluation/competitors/`: Codex ships an `mcp-server`
mode, OpenCode has `serve`/`attach`/ACP, and Grok Build is an ACP agent over
JSON-RPC. Being driveable is also the integration path for **OpenClaw** (gateway) and
**Prospero** (the control plane over caliband) to adopt caliban as a worker backend.

Three candidate surfaces were on the table:

- **MCP-server mode** — caliban *as* an MCP server other agents call.
- **ACP** — Agent Client Protocol over JSON-RPC; editors/tools drive it interactively.
- **Headless HTTP serve** — HTTP/stream over the existing `-p` NDJSON stream.

The design-spike framing (#504) hypothesized picking *one* adapter first. That framing
is wrong: the three surfaces are **not substitutes — they target different consumers**,
so shipping one leaves a real, named gap open:

- **MCP-server** serves *another agent* calling caliban as a worker/tool — coarse-grained
  request/response. This is the OpenClaw-gateway and Prospero-control-plane path, plus
  Codex parity. Pick only this and the editor-integration gap stays open.
- **ACP** serves *a human in an editor* driving caliban over JSON-RPC — fine-grained,
  turn-by-turn streaming with permission prompts surfaced into the editor's own UI. This
  is the OpenCode / Grok Build / Zed path. Pick only this and the worker-backend gap stays
  open.
- **Headless HTTP serve** serves *a script or `curl`* with no protocol client at all —
  start a run, stream events, poll status over plain HTTP/NDJSON.

What makes "all three" affordable rather than three separate builds: caliban already owns
the hard parts. The headless `-p` NDJSON stream (0025), `TurnEvent` serde (#78), the
caliband live-attach/stream protocol (#79), and inbound-message delivery to a running
agent (#81) are exactly a `run / stream / status / input` contract in all but name. A
thin transport-agnostic **core** over those internals turns each surface into a protocol
adapter — a shim — rather than a fresh integration.

## Decision Drivers

- Close the recurring top competitor gap against *every* terminal-agent peer at once, not
  one at a time.
- Unblock both integration consumers — worker-backend (Prospero/OpenClaw) *and* editor
  drive-in — rather than forcing a choice between them.
- Reuse existing internals (0025 NDJSON, #78/#79/#81) so the marginal cost of each surface
  is an adapter, not a subsystem.
- Land value incrementally: a shared core plus one adapter must be shippable before the
  others exist.

## Decision

We will expose caliban as a driveable backend through **all three surfaces — MCP-server,
ACP, and headless HTTP serve — built as thin protocol adapters over one shared,
transport-agnostic core.** We reject the "one surface only" framing of the spike; the
surfaces serve distinct consumers and are individually cheap once the core exists.

**The core.** A single in-process **drive core** exposing a transport-agnostic contract,
factored out of the existing headless/attach internals (0025, #78, #79, #81):

- **run** — start an agent run from a prompt + config (workspace, model, permission
  profile); returns a session/run id.
- **stream** — subscribe to the run's `TurnEvent` stream (the same events the TUI and `-p`
  headless mode already emit).
- **status** — read a run's lifecycle state (running / awaiting-input / idle / done, plus
  the result frame per 0025/0049).
- **input** — deliver an inbound message to a running agent (the #81 path), including
  permission-prompt responses.

Every adapter is a codec/transport over this contract and holds **no agent logic**.

**Sequencing — MCP-server leads v1.** Adapters land over the same core in order:

1. **MCP-server** first — smallest reach to a named consumer (Prospero as worker backend,
   OpenClaw as gateway) and Codex parity. Transports/OAuth follow 0023.
2. **ACP** second — the JSON-RPC editor surface (Zed/OpenCode/Grok Build parity), including
   surfacing permission elicitation into the editor UI.
3. **Headless HTTP serve** — HTTP/NDJSON over the core for client-less `curl`/script
   drive-in. Largely falls out of the core once it is HTTP-shaped internally; shipped as a
   first-class surface, not dropped.

**Auth.** Local/loopback default is unauthenticated (filesystem/loopback is the boundary,
consistent with 0051's Unix-socket posture). Any non-loopback surface requires a bearer
token supplied out of band (env/secret), reusing 0051's bearer scheme; the MCP-server
surface additionally honors the 0023 OAuth flow where a caller drives it as a remote MCP
endpoint.

**Permissions.** All surfaces drive through the existing Permissions v2 engine (0045) —
a driven run carries a permission profile, and tool calls that would prompt are surfaced
back over the surface's own channel (MCP elicitation per 0023; ACP permission requests;
an HTTP `awaiting-input` status + input call). No surface may bypass the permission model
or widen a run's authority beyond its profile.

**In scope for v1:** the drive core; the MCP-server adapter; the run/stream/status/input
contract; bearer auth + loopback default; permission integration; a no-TUI smoke/e2e test.
**Out of scope for v1 (follow-on tickets):** the ACP and HTTP-serve adapters (land next
over the same core), multi-tenant isolation, and any typed cross-language IDL — this ADR
commits to NDJSON/JSON-RPC codecs, not protobuf.

**Implementation sub-tickets to spawn on epic #503:**

1. Factor the transport-agnostic drive core (run/stream/status/input) out of the headless
   `-p`/attach internals.
2. MCP-server adapter — `caliban mcp serve`-style; expose the core as MCP tools; wire 0023
   transports/OAuth.
3. Bearer-auth + loopback-default gate shared by all network surfaces.
4. Permission-elicitation bridge — surface prompts back over each adapter's channel.
5. No-TUI smoke/e2e test that drives caliban end-to-end (run → stream → status → input).
6. ACP adapter over JSON-RPC — editor drive-in + permission surfacing.
7. Headless HTTP serve adapter over the core (HTTP/NDJSON).
8. Docs: driving caliban headlessly (one page per surface).

## Consequences

- **Positive:** closes the driveable-backend gap against Codex, OpenCode, and Grok Build
  in one design, and unblocks both integration consumers (Prospero/OpenClaw worker-backend
  *and* editor drive-in) instead of forcing a choice. The shared core means each surface is
  an adapter, not a subsystem, and MCP-first ships real value before ACP/HTTP exist. Every
  surface inherits the same auth and Permissions v2 (0045) enforcement by construction.
- **Negative:** three public surfaces is a larger long-run maintenance and test matrix than
  one, and three protocol contracts to keep stable. Factoring a clean core out of the
  existing headless/attach internals is real up-front work before the first adapter lands.
  We own bearer auth and (for HTTP) a serve loop by hand rather than inheriting a
  framework's batteries. Permission-prompt surfacing must be implemented once per adapter
  channel.
- **Revisit if:** a surface finds no adopters after shipping (drop it rather than carry dead
  protocol surface), the three adapters accrete divergent logic instead of staying thin
  shims (the core abstraction is leaking — refactor or collapse), or a consumer needs a
  typed cross-language contract badly enough to justify a protobuf/gRPC IDL over the
  JSON codecs chosen here.
