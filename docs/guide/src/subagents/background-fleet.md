# The Background Fleet

Caliban can run sub-agents in the background — detached from your current
session — and let you monitor, attach to, or stop them at will. A
per-workspace supervisor daemon (`caliband`) owns the fleet and keeps agents
alive even after the parent `caliban` process exits.

## Spawning a background agent

### From the command line

The quickest way to fire off a background task is the `--bg` flag:

```bash
caliban --bg "refactor the auth module to use the new token type"
```

This is shorthand for `caliban agents spawn --prompt <task>`. Caliban
auto-starts `caliband` if it is not already running, then returns immediately
with the new agent's id.

### From inside a session

The model can request a background sub-agent by setting `background: true`
in an `AgentTool` call. The parent session receives the id and a note to
check back via `caliban attach <id>`.

## The `caliband` daemon

`caliband` is a separate binary shipped alongside `caliban`. It runs as a
per-*workspace* daemon: each workspace root gets its own daemon instance. A
workspace is usually a single git repository, but since v0.5.0 the supervisor
can manage a workspace spanning multiple sources (repos), each with its own
worktree isolation.

**Socket path** (resolution order):

1. `$CALIBAN_DAEMON_RUNTIME_DIR/<hash>.sock` if `CALIBAN_DAEMON_RUNTIME_DIR`
   is set.
2. `$XDG_RUNTIME_DIR/caliban/<hash>.sock` if `$XDG_RUNTIME_DIR` is set.
3. `$TMPDIR/caliban-daemon/<hash>.sock` (fallback; typical on macOS).

The `<hash>` is a 16-hex-char SHA-256 prefix of the absolute workspace root
path, so each workspace gets a stable, unique socket without naming collisions.
(For a single-repo workspace the workspace root is the repo root, so the socket
is unchanged from earlier releases.)

`caliband` auto-starts when any `caliban agents` command or `--bg` flag
needs it. You should rarely need to launch it directly.

```admonish tip title="Installing caliband"
`cargo install caliban` installs only the `caliban` binary.
To also install the daemon run:

    cargo install caliban-supervisor --bin caliband

Both binaries must be on your `$PATH` for background fleet features to work.
```

## Networked control plane (beta)

By default `caliband` serves its control plane over the local Unix domain
socket described above. Since v0.5.0 it can instead serve that same
line-delimited (NDJSON) protocol over **TCP**, so a remote client (for example
prospero) can drive the fleet across the network rather than only from the same
host. Enable it by passing `--listen <host:port>` (or setting
`CALIBAN_DAEMON_LISTEN`) when the daemon starts:

```bash
caliband --workspace-root /path/to/workspace \
  --listen 0.0.0.0:7070 \
  --tls-cert cert.pem --tls-key key.pem \
  --token "$CALIBAN_DAEMON_TOKEN"
```

```admonish warning title="TCP mode is fail-closed"
The networked control plane requires **both** a bearer token (`--token`) and
TLS (`--tls-cert`/`--tls-key`) — since v0.6.0 the daemon refuses to bind a TCP
listener that is unauthenticated or plaintext. The default Unix-socket mode is
unchanged and needs neither. The TCP transport is still **beta**.
```

## Agent lifecycle states

| State | Meaning |
|---|---|
| `spawning` | Registered, not yet executing |
| `running` | Actively processing turns |
| `idle` | Waiting for input; no compute pending |
| `killed` | Stopped via `kill` |
| `done` | Finished successfully |
| `failed` | Finished with an error |
| `crashed` | Daemon restarted while agent was active; needs recovery |

## `caliban agents` subcommands

### `caliban agents list`

Print all registered agents and their status.

```bash
caliban agents list
```

### `caliban agents spawn`

Spawn a new background agent with an explicit prompt.

```bash
caliban agents spawn --prompt "audit all SQL queries for injection risks"
caliban agents spawn --prompt "write tests for crates/caliban-tools-builtin" --label my-test-agent
```

Options:

| Flag | Description |
|---|---|
| `--prompt <TEXT>` | Initial prompt for the agent (required) |
| `--label <NAME>` | Human-readable label shown in `list` and logs |

### `caliban agents attach <id>`

Stream a running agent's transcript live. Press `Ctrl+D` to detach without
stopping the agent.

```bash
caliban agents attach a3f8b2c1
```

### `caliban agents logs <id>`

Print the agent's session log (`session.json`).

```bash
caliban agents logs a3f8b2c1
```

### `caliban agents kill <id>`

Terminate an agent (SIGTERM, escalating to SIGKILL after a grace period).

```bash
caliban agents kill a3f8b2c1
```

### `caliban agents respawn <id>`

Kill the agent and restart it with the same original spawn spec (same
prompt, model, isolation settings).

```bash
caliban agents respawn a3f8b2c1
```

Note that `respawn` assigns a new id; the old id is removed from the
registry.

### `caliban agents rm <id>`

Remove an agent from the registry. The agent must be stopped first, unless
`--force` is passed.

```bash
caliban agents rm a3f8b2c1
caliban agents rm a3f8b2c1 --force   # remove even if still running
```

## Top-level shorthands

Four common operations have top-level sugar to save typing:

| Shorthand | Equivalent |
|---|---|
| `caliban attach <id>` | `caliban agents attach <id>` |
| `caliban logs <id>` | `caliban agents logs <id>` |
| `caliban stop <id>` | `caliban agents kill <id>` |
| `caliban kill <id>` | `caliban agents kill <id>` |
| `caliban respawn <id>` | `caliban agents respawn <id>` |
| `caliban rm <id>` | `caliban agents rm <id>` |

## `caliban daemon` subcommands

### `caliban daemon status`

Print daemon health, PID, uptime, agent count, and the socket path.

```bash
caliban daemon status
```

### `caliban daemon stop`

Ask the daemon to shut down gracefully after finishing in-flight requests.
Running agents are not automatically killed; stop them first if you want a
clean shutdown.

```bash
caliban daemon stop
```

## Session storage

Each background agent's transcript is stored as a regular caliban session
file at `<base>/agents/<id>/session.json`. This means all session tooling
(compaction, replay, audit) works on background agents out of the box.
Attaching to an agent is conceptually the same as resuming its session over
the agent's per-agent socket.

## Diagram: agent lifecycle

```mermaid
flowchart LR
    A([caliban --bg task]) -->|spawn request| D[caliband daemon]
    D -->|registers| R[(Registry)]
    D -->|starts| W[Agent worker]
    W -->|streams turns| S[(session.json)]
    W -->|per-agent socket| T([caliban attach id])
    W -->|done/failed| R
    T2([caliban agents kill id]) -->|kill request| D
    D -->|SIGTERM→SIGKILL| W
```

For how background agents use git worktree isolation, see
[Worktree Isolation](worktrees.md).
