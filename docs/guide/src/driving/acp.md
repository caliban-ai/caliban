# ACP

`caliban acp serve` exposes caliban as an **Agent Client Protocol (ACP)** agent so editors and editor-shaped tools — the Zed / OpenCode / Grok Build drive-in path — can drive it interactively, turn by turn. Unlike the poll-based [MCP](./mcp-server.md) and [HTTP](./http-serve.md) surfaces, ACP is **push/streaming**: a `session/prompt` call blocks for the duration of one agent turn while the adapter streams `session/update` notifications, then returns a `stopReason`.

The wire is newline-delimited **JSON-RPC 2.0** over stdio — one JSON object per line, no LSP-style `Content-Length` framing.

```bash
caliban acp serve
```

Most editors don't run this by hand — they spawn it as the agent subprocess for their ACP integration. stdio is loopback-inherent, so the [auth gate](./overview.md#auth-model) admits the peer.

## Methods

**Client → agent:**

| Method | Params | Returns |
|--------|--------|---------|
| `initialize` | `{ protocolVersion, clientCapabilities }` | agent capabilities |
| `authenticate` | — | accepted (the shared gate governs auth) |
| `session/new` | `{ cwd, mcpServers }` | `{ sessionId }` |
| `session/prompt` | `{ sessionId, prompt: [ContentBlock] }` | `{ stopReason }` (after streaming updates) |
| `session/cancel` | `{ sessionId }` (notification) | — |

**Agent → client:**

- `session/update` (notification) — one `TurnEvent` mapped to an ACP update: `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`.
- `session/request_permission` (request) — a driven run's `Ask`, surfaced into the editor's own permission UI (see below).

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
← {"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}
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

See [ADR 0055](../adr/0055-driveable-server-surface.md) for the design rationale.
