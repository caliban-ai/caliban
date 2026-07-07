# caliband Session-Plane Authn/TLS Hardening — Design (#288)

**Goal:** Close the fail-open gap on caliband's TCP session plane: a
network-listening agent must never accept unauthenticated clients, and must
never carry a bearer token over plaintext. Enforce fail-closed at startup so
"unauthenticated clients are rejected; prospero authenticates successfully."

Part of #274 (k8s epic). Spec:
`docs/superpowers/specs/2026-07-03-caliban-k8s-system-design.md`. Relates to
prospero #2 / #82 (the client that already presents TLS + a bearer token).

## Background / finding

#280 built the transport primitives and wired the TCP `--listen` path:

- `transport::{tls_server_from_pem, server_check_token}` exist; `Listener::accept`
  performs the TLS handshake and, **when a token is configured**, checks the
  bearer-token preamble (`transport.rs`). Accept-time enforcement is already
  tested (`tcp_token_accept_and_reject`: good token → Ok, bad token → Err).
- But the worker's `--listen` branch is **fail-open**:
  - `let token = std::env::var("CALIBAN_AGENT_TOKEN").ok();` — unset ⇒ `None` ⇒
    the listener accepts **unauthenticated** clients.
  - `load_agent_tls()` returns `None` when cert/key env is unset ⇒ **plaintext**
    TCP. A mandatory token over plaintext would leak to an on-path observer —
    the exact risk #280 already guards on the *client* dial ("never build a
    plaintext+token client").

Unix mode (`--socket`) is local and filesystem-permission-scoped; it is
intentionally tokenless and is **out of scope** — unchanged.

## Decision

On TCP (`--listen`) mode, require **both** credentials before binding, failing
closed (refuse to start) otherwise. This mirrors #280's client-side posture.

Add a pure policy function in `caliban/src/worker.rs`:

```rust
/// Fail-closed credential policy for the TCP session plane (#288). Returns
/// `Err(reason)` when a `--listen` (network) agent would bind unsafely.
fn require_network_credentials(token: Option<&str>, tls_present: bool) -> Result<(), String> {
    let token = token.map(str::trim).filter(|t| !t.is_empty());
    if token.is_none() {
        return Err("CALIBAN_AGENT_TOKEN is required for --listen (network) mode; \
                    refusing to bind an unauthenticated listener".to_owned());
    }
    if !tls_present {
        return Err("agent TLS (CALIBAN_AGENT_TLS_CERT/KEY) is required for --listen mode; \
                    refusing to send the bearer token over plaintext".to_owned());
    }
    Ok(())
}
```

Wire it into the `--listen` branch of `worker::run`: after `load_agent_tls()`
and reading `CALIBAN_AGENT_TOKEN`, call the guard; on `Err`, `eprintln!` the
reason and return exit code **78** (`EX_CONFIG`). Only on `Ok` build the
`BindSpec` and bind. Empty/whitespace-only token is treated as absent.

### Deliberate choices (and why)

- **Token AND TLS both mandatory on TCP.** Token alone over plaintext leaks the
  secret; TLS alone leaves the surface unauthenticated. Both, or don't bind.
- **Fail at startup, not per-connection.** A wide-open network listener should
  never come up at all — clearer operationally than binding and rejecting.
- **Unix mode unchanged.** Local, filesystem-scoped; no token/TLS required.
- **Reuse existing accept-time enforcement.** Bad/absent tokens from a *client*
  are already rejected at accept (tested). This ticket only guarantees the
  server never runs in a fail-open configuration.

## Files

- `caliban/src/worker.rs` — add `require_network_credentials`; call it in the
  `--listen` branch before `BindSpec` construction; add `#[cfg(test)] mod`
  unit tests for the policy.

## Testing

Unit tests on `require_network_credentials` (pure, no I/O):

- token `None`, tls `false` → `Err` mentioning the token requirement.
- token `Some("")` / `Some("   ")` → `Err` (empty treated as absent).
- token `Some("t")`, tls `false` → `Err` mentioning TLS/plaintext.
- token `Some("t")`, tls `true` → `Ok`.

The existing `transport::tcp_token_accept_and_reject` continues to cover
accept-time rejection of a bad token.

## Acceptance

A `--listen` agent refuses to start unless a non-empty `CALIBAN_AGENT_TOKEN`
and agent TLS are both configured; with both set, prospero (presenting TLS + the
token, prospero #82) authenticates and unauthenticated clients are rejected.
Relates #2; part of #274.
