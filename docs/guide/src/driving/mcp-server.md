# MCP Server

`caliban mcp serve` exposes caliban as an **MCP server** that another agent can drive as a worker or tool. This is the path for a gateway or control plane (e.g. Prospero) to adopt caliban as a backend, and for Codex-style `mcp-server` parity.

v1 is **stdio-only** with **poll-based unary tools**: MCP tool calls are request/response, but the drive core is a stream, so a run is driven by `caliban_run` → repeated `caliban_poll` rather than a live push.

```bash
caliban mcp serve
```

The peer connects over stdio and speaks MCP. Because stdio is loopback-inherent, the [auth gate](./overview.md#auth-model) admits the peer; `CALIBAN_DRIVE_TOKEN` only bites on the network-facing HTTP surface.

## Tools

| Tool | Arguments | Returns |
|------|-----------|---------|
| `caliban_run` | `{ prompt, interactive? }` | `{ run_id }` |
| `caliban_poll` | `{ run_id, cursor }` | `{ events: [{v,event}…], next_cursor, status, permission_request? }` |
| `caliban_status` | `{ run_id }` | `{ status }` |
| `caliban_send_input` | `{ run_id, text?, end? }` | `{ ok }` |
| `caliban_permit` | `{ run_id, tool_use_id, allow, reason? }` | `{ ok }` |

## Driving a run

1. `caliban_run { "prompt": "list the crates", "interactive": false }` → note the `run_id`.
2. Loop `caliban_poll { "run_id": …, "cursor": 0 }`, advancing `cursor` to the returned `next_cursor` each time, appending the `events`. Each event is a `{ v, event }` envelope (see [the event envelope](./overview.md#the-event-envelope)).
3. Stop when `status.state` is `done` or `failed`.

For an **interactive** run, poll until `status.state` is `awaiting_input`, then `caliban_send_input { "run_id": …, "text": "…" }` to continue, or `{ "run_id": …, "end": true }` to finish.

## Answering permission prompts

When a poll returns a `permission_request`:

```json
{ "tool_use_id": "toolu_01…", "tool_name": "Bash", "input": { "command": "rm -rf build" } }
```

answer it before the run can proceed:

```json
caliban_permit { "run_id": "…", "tool_use_id": "toolu_01…", "allow": false, "reason": "destructive" }
```

## Registering caliban in an MCP client

Point any MCP client's server config at the stdio command:

```json
{
  "mcpServers": {
    "caliban": {
      "command": "caliban",
      "args": ["mcp", "serve"]
    }
  }
}
```

The transports and OAuth flow that a *remote* MCP endpoint layers on top follow [ADR 0023](../adr/0023-mcp-v2-transports-and-oauth.md); the drive surface itself is defined by [ADR 0055](../adr/0055-driveable-server-surface.md).
