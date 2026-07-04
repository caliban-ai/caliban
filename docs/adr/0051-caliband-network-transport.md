# ADR 0051 · caliband network transport: NDJSON over TCP+TLS with a bearer token

- **Status:** accepted
- **Date:** 2026-07-04
- **Source:** [`docs/superpowers/plans/2026-07-04-p1-caliband-network-transport.md`](../superpowers/plans/2026-07-04-p1-caliband-network-transport.md) · caliban [#280](https://github.com/caliban-ai/caliban/issues/280) · epic [#274](https://github.com/caliban-ai/caliban/issues/274). The originating design (`2026-07-03-caliban-k8s-system-design.md`, §"Transport lift") is a cross-repo epic spec in the caliban-ai docs hub, not in this repo.

## Context

caliband's IPC is entirely Unix-domain today. The control socket
(`caliban-supervisor`: `Supervisor::serve` binds a `UnixListener`;
`SupervisorClient` connects a `UnixStream` per request) carries newline-delimited
JSON — `CtlRequest`/`CtlReply` in `caliban_supervisor::proto`. Each agent's stream
socket (bound by the `caliban __agent-worker` in `caliban/src/worker.rs`) carries
NDJSON too: outbound `caliban_agent_core::TurnEvent`, inbound `AttachInbound`. There
is **no transport or address abstraction** anywhere on this path — the code is
hardwired to `UnixListener`/`UnixStream`, and socket endpoints are typed
`PathBuf` throughout (`AgentRecord.socket_path`, `CtlReply::Spawned { socket_path }`,
`CtlReply::AttachAck { socket_path }`, `DaemonStatus.socket_path`).

The k8s epic (#274) needs cross-pod reach: prospero's control plane and its live
session client must talk to a caliband running in another pod, over the Sandbox's
stable DNS name, with transport security and authentication. A Unix socket cannot
cross a pod boundary.

The design spec is internally split on *how*. Its architecture diagram labels the
session plane "gRPC/TLS" (§Architecture), but its prose repeatedly frames the lift
as a **swappable byte transport** — "one client, swappable `Transport`
(`UnixStream` vs `TCP/TLS`)" (§4), "only the transport differs (Unix socket local
vs. TCP/TLS in-cluster)" (§two planes), and calls the workspace-scoping change
"modest … not the protocol" (§3). The protocol on the wire today is already
NDJSON.

Options weighed:

- **(A) NDJSON over TCP + TLS + bearer token.** Keep the existing `serde_json`
  message types (`CtlRequest`/`CtlReply`, `TurnEvent`/`AttachInbound`) verbatim.
  Introduce an endpoint/transport abstraction so the same accept loops and client
  connect paths run over either a Unix socket (local default, unchanged) or a
  `tokio-rustls` TLS stream on TCP. Authenticate network connections with a bearer
  token supplied out of band (env/Secret). The framing (`read_line` / `write` +
  `\n`) is transport-agnostic and unchanged.
- **(B) Native gRPC (tonic/prost).** Define a protobuf service, regenerate the
  prospero-side client, port every message to generated types, move auth to gRPC
  interceptors/mTLS. This is the spec diagram's north-star.
- **(C) Bridging sidecar.** A proxy that terminates TLS and relays to the local
  Unix socket. Explicitly rejected by the spec ("not a bridging sidecar") — it
  leaves caliband local-only and adds a moving part per pod.

## Decision

We will implement **Option A**: caliband gains a **network transport carrying the
existing NDJSON protocol over TCP with TLS (`tokio-rustls`), authenticated by a
bearer token**, while the Unix-domain socket path remains the unchanged local
default.

Concretely:

- Introduce a transport/endpoint abstraction that both the control socket
  (`caliban-supervisor`) and the per-agent stream socket (`caliban/src/worker.rs`)
  bind and connect through, so accept loops and client connects are written once
  over an abstract listener/stream rather than concrete `UnixListener`/`UnixStream`.
- Generalize the `PathBuf` socket endpoints in `proto` to a transport-agnostic
  endpoint that can name either a Unix path or a `host:port` (with TLS), so
  `Spawned`/`AttachAck`/`AgentRecord`/`DaemonStatus` describe a network agent.
- Add TLS via `tokio-rustls` (server acceptor + client connector); reuse the
  workspace's existing `rustls` rather than introducing `native-tls`.
- Authenticate every network (non-Unix) connection with a bearer token read from
  the environment; local Unix connections stay unauthenticated (filesystem
  permissions are the boundary, as today). Deeper authn/mTLS is tracked with
  prospero #2.
- Keep all existing `serde_json` message types and NDJSON framing **byte-for-byte
  on the wire**. This is a transport lift, not a protocol change.

Native gRPC (Option B) is **deferred, not discarded** — it is the spec's stated
long-term direction and is tracked as **[#314](https://github.com/caliban-ai/caliban/issues/314)**.
We choose A now because caliband's protocol is *already* NDJSON, so A reuses the
existing message types and both accept loops, ships network reach + TLS + auth on
the critical path to a working umbrella, and leaves the session-plane shape intact
(prospero's client swaps a byte transport, not a codec).

## Consequences

- **Positive:** smallest faithful lift — no protobuf schema, no client
  regeneration, no message-type churn; the umbrella's session plane works
  cross-pod with TLS + a token. The new transport abstraction is exactly the seam
  gRPC (#314) will later slot into, so this is not throwaway work. Local
  developer flow is untouched (Unix socket, no token, no TLS).
- **Negative:** we own TLS config and a bearer-token scheme by hand rather than
  inheriting gRPC's batteries (interceptors, standard auth, reflection,
  backpressure). The `proto` endpoint-type generalization ripples through every
  `socket_path: PathBuf` site and its prospero-side mirror (prospero's
  `AgentHandle.socket`). NDJSON-over-TCP has no HTTP/2 multiplexing; each control
  request still opens a connection (as today).
- **Revisit if:** we need a typed cross-language contract, streaming
  backpressure/flow-control, or standard auth/observability middleware badly
  enough to pay the rewrite — at which point #314 (gRPC) supersedes this ADR. Also
  revisit the bearer-token choice if multi-tenant isolation (prospero #2) demands
  per-agent mTLS identity rather than a shared daemon token.
