# Driving Caliban

Print mode ([Print Mode](../automation/print-mode.md)) runs caliban to completion and hands you the result. The **driveable server surfaces** go further: they expose a running agent that an external program *steers* — start a run, watch its events stream by, feed it follow-up input, and answer the permission prompts it raises — with no TUI in the loop.

There are three surfaces, each targeting a different kind of consumer, all built as thin protocol adapters over one shared drive core (see [ADR 0055](../adr/0055-driveable-server-surface.md)):

| Surface | Command | Consumer | Page |
|---------|---------|----------|------|
| **MCP server** | `caliban mcp serve` | another agent calling caliban as a worker/tool | [MCP Server](./mcp-server.md) |
| **ACP** | `caliban acp serve` | a human in an editor (Zed / OpenCode / Grok Build) | [ACP](./acp.md) |
| **HTTP serve** | `caliban http serve` | a script or `curl` with no protocol client | [HTTP Serve](./http-serve.md) |

They are not substitutes — pick the one that matches how your consumer wants to talk to caliban.

## The shared contract

Every surface exposes the same four operations over the drive core. The wire format differs; the semantics do not.

| Operation | Meaning |
|-----------|---------|
| **run** | Start an agent run from a prompt (`interactive` decides whether it pauses for input at each turn boundary). Returns a run/session id. |
| **stream** | Read the run's [`TurnEvent`](../automation/stream-json.md) stream — the same events the TUI and `-p` headless mode emit. |
| **status** | Read the run's lifecycle state. |
| **input** | Deliver a follow-up message to an interactive run, or signal end-of-input. |

### Lifecycle status

A run moves through these states (`caliban_drive::DriveStatus`):

```text
starting → running → (awaiting_input ⇄ running)* → done | failed
```

A non-interactive run never enters `awaiting_input` — it goes `running → done`. `failed` carries an `error` string. On the MCP and HTTP surfaces status is a JSON object tagged by `state` (e.g. `{ "state": "awaiting_input" }`); on ACP the turn boundary is conveyed as the `stopReason` of a `session/prompt` call.

### The event envelope

The poll-based surfaces (MCP, HTTP) wrap each `TurnEvent` in a versioned envelope so the event schema can evolve without breaking clients:

```json
{ "v": 1, "event": { "type": "AssistantTextDelta", "text": "Hello", ... } }
```

ACP instead translates each `TurnEvent` into a native `session/update` notification (`agent_message_chunk`, `tool_call`, …).

## Auth model

One policy, applied uniformly by every surface (`CALIBAN_DRIVE_TOKEN`, building on the caliband bearer scheme):

- **Loopback is open.** A connection from the local host is trusted — loopback / the local filesystem is the boundary. `caliban mcp serve` and `caliban acp serve` run over stdio, which is loopback-inherent, so they always admit the peer.
- **Remote requires a bearer token.** A non-loopback peer must present `Authorization: Bearer <token>` matching the `CALIBAN_DRIVE_TOKEN` set out of band (env / secret), compared in constant time.
- **Fail closed.** Binding a surface to a non-loopback address with no token configured is refused — caliban never serves unauthenticated on the network.

Transport encryption (TLS) for remote binds is the deployment's concern and is out of scope for the gate.

## Permissions

A driven run still enforces [Permissions v2](../permissions/concepts.md). When a tool call matches an `Ask` rule and there is no TUI to prompt, the request is surfaced back over the surface's own channel and its decision routed into the run — the run never bypasses the permission model or fails open:

- **MCP** — the prompt appears in the `caliban_poll` response's `permission_request`; answer it with `caliban_permit`.
- **ACP** — the agent sends a `session/request_permission` request; the client replies with the selected option.
- **HTTP** — the prompt appears in the `GET …/events` response's `permission_request`; answer it with `POST …/permit`.

If no client answers within the timeout (10 minutes), the request is denied.

## Design rationale

See [ADR 0055 · Driveable server surface](../adr/0055-driveable-server-surface.md) for why caliban ships all three surfaces over one core rather than picking one, and how they reuse the existing headless / attach internals.
