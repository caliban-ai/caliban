# HTTP Serve

`caliban http serve` exposes caliban over plain **HTTP/JSON** so a script or `curl` can drive it with no protocol client at all. Same poll-based, cursor-driven shape as the [MCP surface](./mcp-server.md), with the `{ v, event }` [event envelope](./overview.md#the-event-envelope).

```bash
caliban http serve                       # binds 127.0.0.1:8730 by default
caliban http serve --addr 127.0.0.1:9000
```

## Auth and binding

This is where the [bearer path](./overview.md#auth-model) actually bites:

- A **loopback** bind (the default) is open — no token needed.
- A **non-loopback** bind requires `CALIBAN_DRIVE_TOKEN`, and every remote request must send `Authorization: Bearer <token>`. Binding to a non-loopback address with no token set is refused (fail closed).

```bash
CALIBAN_DRIVE_TOKEN=s3cret caliban http serve --addr 0.0.0.0:8730
```

## Endpoints

| Method & path | Body | Returns |
|---------------|------|---------|
| `POST /runs` | `{ prompt, interactive? }` | `{ run_id }` |
| `GET /runs/:id/events?cursor=N` | — | `{ events: [{v,event}…], next_cursor, status, permission_request? }` |
| `GET /runs/:id/status` | — | `{ status }` |
| `POST /runs/:id/input` | `{ text?, end? }` | `{ ok }` |
| `POST /runs/:id/permit` | `{ tool_use_id, allow, reason? }` | `{ ok }` |

## A curl session

Start a run:

```bash
RUN=$(curl -s localhost:8730/runs \
  -H 'content-type: application/json' \
  -d '{"prompt":"list the crates","interactive":false}' | jq -r .run_id)
```

Poll for events, advancing the cursor until the run is terminal:

```bash
cursor=0
while :; do
  resp=$(curl -s "localhost:8730/runs/$RUN/events?cursor=$cursor")
  echo "$resp" | jq -c '.events[].event'
  cursor=$(echo "$resp" | jq .next_cursor)
  state=$(echo "$resp" | jq -r .status.state)
  [ "$state" = "done" ] || [ "$state" = "failed" ] && break
  sleep 0.2
done
```

For an **interactive** run, poll until `.status.state == "awaiting_input"`, then:

```bash
curl -s localhost:8730/runs/$RUN/input -H 'content-type: application/json' -d '{"text":"and now the tests"}'
curl -s localhost:8730/runs/$RUN/input -H 'content-type: application/json' -d '{"end":true}'
```

## Answering a permission prompt

If an `events` response carries a `permission_request`, answer it before the run continues:

```bash
curl -s localhost:8730/runs/$RUN/permit \
  -H 'content-type: application/json' \
  -d '{"tool_use_id":"toolu_01…","allow":true}'
```

A remote client adds `-H "Authorization: Bearer $CALIBAN_DRIVE_TOKEN"` to every request.

See [ADR 0055](../adr/0055-driveable-server-surface.md) for the design rationale.
