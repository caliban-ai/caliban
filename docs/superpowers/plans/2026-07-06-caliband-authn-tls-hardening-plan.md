# caliband Authn/TLS Hardening — Implementation Plan (#288)

> **For agentic workers:** TDD. Steps use checkbox syntax.

**Goal:** Fail-closed credential policy on the TCP session plane — a `--listen`
agent refuses to start without both a non-empty `CALIBAN_AGENT_TOKEN` and agent
TLS.

**Architecture:** A pure policy function `require_network_credentials`, unit-
tested, called in `worker::run`'s `--listen` branch before binding.

**Tech Stack:** Rust, `caliban` bin, `caliban-supervisor::transport`.

## Global Constraints

- TCP (`--listen`) requires token AND TLS; empty/whitespace token = absent.
- Fail at startup with exit code 78 (`EX_CONFIG`); do not bind.
- Unix (`--socket`) mode unchanged (tokenless, local).
- Full local gate before PR: `cargo fmt --all -- --check`, `cargo clippy
  --workspace --all-targets -- -D warnings`, `cargo build --workspace
  --all-targets`, `cargo test --workspace`.

---

### Task 1: Fail-closed network credential policy

**Files:**
- Modify: `caliban/src/worker.rs`
  - Add `fn require_network_credentials(token: Option<&str>, tls_present: bool)
    -> Result<(), String>`.
  - In `run()`'s `(Some(addr), _)` `--listen` branch: after `load_agent_tls()`
    and reading `CALIBAN_AGENT_TOKEN`, call the guard; on `Err`, `eprintln!` +
    `return 78`.
  - Add `#[cfg(test)] mod` unit tests for the policy.

**Interfaces:**
- Produces: `require_network_credentials(Option<&str>, bool) -> Result<(), String>`.

- [ ] **Step 1: Write the failing tests** — the four cases (no token; empty
  token; token but no TLS; token + TLS ok).
- [ ] **Step 2: Run, watch fail** — `cargo test -p caliban require_network`
  → fails to compile (fn absent).
- [ ] **Step 3: Implement** the function + wire it into the `--listen` branch.
- [ ] **Step 4: Run, watch pass** — same command green; also
  `cargo test -p caliban-supervisor transport` (accept-time token test intact).
- [ ] **Step 5: touch changed .rs, then full gate** — fmt/clippy/build/test.
- [ ] **Step 6: Commit** — `feat(sub-agents): fail-closed token+TLS on the
  caliband TCP session plane (#288)`.

---

## Self-Review

- Spec coverage: policy fn + wiring + all four test cases — covered.
- No placeholders. Type consistency: single signature used in fn, wiring, tests.
