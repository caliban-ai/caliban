# ACP

`caliban acp serve` exposes caliban as an **Agent Client Protocol (ACP)** agent so editors and editor-shaped tools — the Zed / OpenCode / Grok Build drive-in path — can drive it interactively, turn by turn. Unlike the poll-based [MCP](./mcp-server.md) and [HTTP](./http-serve.md) surfaces, ACP is **push/streaming**: a `session/prompt` call blocks for the duration of one agent turn while the adapter streams `session/update` notifications, then returns a `stopReason`.

The wire is newline-delimited **JSON-RPC 2.0** — one JSON object per line, no LSP-style `Content-Length` framing. The same wire is served over two transports:

| Transport | How it is reached | Auth |
|---|---|---|
| **stdio** | `caliban acp serve` — an editor spawns it as its agent subprocess | stdio is loopback-inherent, so the [auth gate](./overview.md#auth-model) admits the peer |
| **the per-agent network listener** | a background-fleet spawn selects `drive_protocol = "acp"`; no CLI flag | the listener's TLS + bearer token (see [below](#acp-over-the-network)) |

```bash
caliban acp serve
```

Most editors don't run this by hand — they spawn it as the agent subprocess for their ACP integration.

## ACP over the network

A `caliband`-managed agent can speak ACP instead of the NDJSON session plane, on
the **same** TLS + bearer-token listener it already exposes
([ADR 0059](../adr/0059-acp-over-network-and-permission-posture.md), amending
ADR 0055). This is how a control plane like prospero drives in-cluster agents
over a path it already secures.

The selector is the `drive_protocol` field on the control-plane `SpawnSpec`:

| Value | Meaning |
|---|---|
| `"ndjson"` | *(default)* the live-attach session plane — stream / status / input |
| `"acp"` | the worker serves the Agent Client Protocol on its per-agent listener |

One protocol per agent, chosen per spawn; the worker never serves both at once.

```admonish note title="Protocol-level only"
`drive_protocol` is a field on the spawn spec, not a user-facing knob. There is
no `caliban agents spawn` flag, no CLI flag, and no settings key for it — every
in-tree spawn uses the `ndjson` default. Only a driver that builds the
control-plane `Spawn` request itself (prospero, caliban-operator) can select
`acp`.
```

Auth differs from the stdio path. The ACP-level auth gate is **disabled** on this
transport, because the connection has already been authenticated one layer down:
the per-agent listener requires TLS and a bearer token, supplied to each worker
by the supervisor as `CALIBAN_AGENT_TLS_CERT` / `CALIBAN_AGENT_TLS_KEY` and
`CALIBAN_AGENT_TOKEN`. Network mode is fail-closed — a worker with no token or no
TLS material refuses to start rather than serving an unauthenticated endpoint.
The stdio `caliban acp serve` path is unchanged by any of this.

See [The Background Fleet](../subagents/background-fleet.md) for the listener,
ports, and TLS setup.

## Methods

**Client → agent:**

| Method | Params | Returns |
|--------|--------|---------|
| `initialize` | `{ protocolVersion, clientCapabilities }` | agent capabilities |
| `authenticate` | — | accepted (the shared gate governs auth) |
| `session/new` | `{ cwd, mcpServers }` | `{ sessionId }` |
| `session/prompt` | `{ sessionId, prompt: [ContentBlock] }` | `{ stopReason }`, plus `_meta` accounting when the run ended (after streaming updates) |
| `session/cancel` | `{ sessionId }` (notification) | — |

**Agent → client:**

- `session/update` (notification) — one `TurnEvent` mapped to an ACP update: `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`.
- `session/request_permission` (request) — a driven run's `Ask`, surfaced into the editor's own permission UI (see below).

### Tool-call input

The opening `tool_call` update carries only `toolCallId`, `title`, `kind`, and
`status` — the arguments are still streaming at that point. The adapter
accumulates the streamed input fragments per tool call and attaches them to the
completing **`tool_call_update`** as `rawInput`, so a driver can show what a tool
was actually invoked with. The buffered input is parsed as JSON when it forms a
valid value and passed through as a raw string when it does not; an empty input
omits the key entirely.

### Accounting

When a prompt call ends the run, its result carries an `_meta` object on ACP's
extension channel, namespaced under `caliban/`:

| Key | Value |
|---|---|
| `caliban/usage` | accumulated token usage for the run |
| `caliban/turns` | the run's turn count |

This closes the data-parity gap with the NDJSON drive wire, which exposes the
same accounting. Cost is deliberately not included — it is derivable from usage
plus the model's rates.

```admonish note title="`_meta` appears only when the run ends"
The accounting is built from the run-end event, so a `session/prompt` that
returns at a turn boundary with the run still open (for example
`stopReason: "end_turn"` while awaiting further input) has **no** `_meta`. Treat
it as optional rather than guaranteed on every prompt result.
```

> **v1 scope.** The core session lifecycle above is implemented. `loadSession`, the `fs/*` and `terminal/*` client methods, and applying the `mcpServers` declared at `session/new` are not yet wired.

## A minimal handshake

Each line below is one JSON-RPC frame (`→` sent by the client, `←` received). Requests are correlated by `id`; notifications have no `id`.

```jsonc
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}
← {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{…},"authMethods":[]}}

→ {"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/repo","mcpServers":[]}}
← {"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-1"}}

→ {"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"sess-1","prompt":[{"type":"text","text":"summarize the README"}]}}
← {"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"The README "}}}}
← {"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"describes…"}}}}
← {"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn",
     "_meta":{"caliban/usage":{"input_tokens":1840,"output_tokens":212},"caliban/turns":1}}}
```

Send another `session/prompt` on the same `sessionId` to continue the conversation (it resumes the same run), or `session/cancel` to end the current turn early — its prompt call returns `{ "stopReason": "cancelled" }`.

## Permission prompts

When the run hits a tool call gated by an `Ask` rule, the agent sends a request and waits for the editor's answer:

```jsonc
← {"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{
     "sessionId":"sess-1",
     "toolCall":{"toolCallId":"toolu_01…","title":"Bash","rawInput":{"command":"cargo test"}},
     "options":[{"optionId":"allow","name":"Allow","kind":"allow_once"},
                {"optionId":"reject","name":"Reject","kind":"reject_once"}]}}

→ {"jsonrpc":"2.0","id":7,"result":{"outcome":{"outcome":"selected","optionId":"allow"}}}
```

Selecting `allow` lets the tool run; `reject` (or a `cancelled`/absent outcome) denies it. The decision routes straight into the run through the permission-elicitation bridge.

See [ADR 0055](../adr/0055-driveable-server-surface.md) for the design rationale,
and [ADR 0059](../adr/0059-acp-over-network-and-permission-posture.md) for the
over-the-network transport and the per-session permission posture that goes with
it.
