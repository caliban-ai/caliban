# Design: `caliban-contract` — thin published contract crate (#656)

- **Date:** 2026-09-19
- **Ticket:** [#656](https://github.com/caliban-ai/caliban/issues/656)
- **Status:** approved (scope decisions confirmed)

## Problem

caliban's contract with the components that launch and observe it is implicit and
stringly-typed, and is hand-copied into two other repos:

- **Launch contract.** `caliband` uses a hand-rolled arg parser (`crates/caliban-supervisor/src/bin/caliband.rs`)
  with flag/env names as inline string literals; caliban-operator hand-types the
  same names in `src/resources.rs`. Drift surfaces silently at runtime
  (caliban-operator#30 / #32 / #35 / #44).
- **Control wire protocol.** The wire types live in `caliban-supervisor::proto`
  (+ `Endpoint` in `transport`), but the only published crate — `caliban-supervisor` —
  is the whole daemon (tokio-full, tokio-rustls, git2, sha2, worktrees…). prospero
  therefore refuses to depend on it and hand-mirrors the types in
  `crates/core/src/caliband/wire.rs` (~14 KB).

## Decisions (confirmed)

1. **Defer the feature-gated thin client** to the prospero follow-up. This PR ships
   wire types + launch builder only, keeping the crate genuinely thin (deps:
   `serde`, `serde_json`, `thiserror` — zero async).
2. **Full move + re-export.** Move the whole proto wire surface + `Endpoint` into
   `caliban-contract`; `caliban-supervisor` re-exports them so there is a single
   definition and its public API is unchanged.

## Design

### New crate `crates/caliban-contract`

- **Deps:** `serde` (derive), `serde_json` (tests + any helpers), `thiserror`.
  **No** dependency on any other caliban crate (acceptance criterion).
- **`wire` module** — moved verbatim from `caliban-supervisor`:
  - `Endpoint` (from `transport.rs`), `AgentId`, `AgentStatus`, `AgentRecord`,
    `DaemonStatus`, `SpawnSpec`, `DriveProtocol`, `PermissionPosture`,
    `DrainedAgent`, `CtlRequest`, `CtlReply`, `SupervisorError`.
  - The existing serde round-trip / default tests move with them (including the
    #675/#676 `drive_protocol` / `permission_posture` wire-value tests).
- **`launch` module**:
  - Flag/env **name constants** — the single source of truth, e.g.
    `FLAG_WORKSPACE_ROOT`, `FLAG_LISTEN`, `FLAG_TLS_SERVER_NAME`,
    `ENV_DAEMON_LISTEN`, `ENV_DAEMON_TOKEN`, `ENV_ROUTER_CONFIG`, …
  - `struct CalibandLaunch { workspace_root, socket_path, data_base, listen,
    advertise_host, agent_port_base, tls: Option<CalibandTls { cert, key, ca,
    server_name }>, token, router_config }`.
  - `fn args(&self) -> Vec<String>` and `fn env(&self) -> Vec<(String, String)>`,
    both built from the name constants.
  - Provider env-name helpers: `provider_env_prefix(kind) -> &str` (e.g.
    `ANTHROPIC` → `ANTHROPIC_BASE_URL` / `ANTHROPIC_API_KEY`), matching what the
    operator hand-types today.

### `caliban-supervisor` changes

- Depend on `caliban-contract`.
- `proto.rs` becomes `pub use caliban_contract::wire::*;` (re-export) — its public
  path (`caliban_supervisor::proto::CtlRequest`, `caliban_supervisor::SpawnSpec`,
  etc.) stays valid, so `client.rs`, `registry.rs`, `server.rs`, `store.rs`,
  `proc.rs`, and downstream `caliban` all keep compiling unchanged.
- `transport.rs` imports `Endpoint` from `caliban_contract::wire` instead of
  defining it.
- `lib.rs` re-exports stay as they are (now pointing at the re-exported types).

### Drift guard (acceptance test)

- Refactor `caliband`'s hand-parser to match against the contract's flag-name
  **constants** instead of inline string literals.
- Add a test (in `caliban-supervisor`, which depends on the contract) that
  constructs a fully-populated `CalibandLaunch`, runs `.args()` through the real
  `caliband` parser, and asserts every emitted flag is recognized and the parsed
  values round-trip. A new flag added to the builder but not the parser (or a
  renamed flag) fails this test — the drift guard the ticket asks for.

### Workspace / release

- Add `crates/caliban-contract` to `[workspace].members` and a `version`-pinned
  entry in `[workspace.dependencies]` (lockstep with the release bump).

## Out of scope (follow-ups to file)

- **caliban-operator**: replace hand-typed flags/env in `src/resources.rs` with
  `CalibandLaunch`.
- **prospero**: replace `crates/core/src/caliband/wire.rs` with `caliban-contract`;
  optionally adopt a feature-gated thin client.

## Testing

- Golden serde tests move to `caliban-contract` (unchanged behavior).
- New launch-builder unit tests (`args()` / `env()` emit expected tokens).
- New drift guard test (builder ↔ real parser).
- Full workspace gate (fmt / clippy / build / test) green.
