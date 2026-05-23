# ADR 0001 · Async runtime → `tokio`

- **Status:** accepted
- **Date:** 2026-05-22

## Context

caliban's foundation is heavily I/O-bound: provider HTTPS calls, streaming
responses from LLM endpoints, MCP transports, and eventually a multi-session
orchestrator. Rust's async story is fragmented across runtimes (`tokio`,
`async-std`, `smol`, `embassy`), and futures from one runtime cannot
always be polled by another. Picking a runtime up front prevents subtle
cross-runtime breakage as the workspace grows.

## Decision

Standardize on `tokio` (multi-threaded scheduler, `features = ["full"]`)
across every crate in the workspace. The workspace root pins the version
in `[workspace.dependencies]`; member crates declare `tokio.workspace = true`
and may select their own feature subset.

No nested runtimes. Each binary creates a single `tokio::runtime::Runtime`
(or uses `#[tokio::main]`) for its entire lifetime.

## Consequences

- **Positive:** direct compatibility with `reqwest`, `tower`, `hyper`,
  `axum`, `tonic`, every major MCP transport, and most LLM SDKs.
  Predictable async behavior across the workspace. Easy onboarding —
  tokio is the de facto Rust async runtime.
- **Negative:** locks the workspace out of `smol`/`embassy` ecosystems
  (acceptable — no embedded targets planned). Binary size larger than
  a minimal runtime would produce.
- **Revisit if:** caliban needs to run in a `no_std` or `embassy`-only
  environment, or if a critical dependency requires a different runtime.
