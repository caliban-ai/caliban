# ADR 0002 · Error model → `thiserror` for libraries, `anyhow` for binary

- **Status:** accepted
- **Date:** 2026-05-22

## Context

Rust libraries benefit from precise error enums — consumers want to
match on variants and react differently to different failure modes.
Binaries benefit from ergonomic context propagation — operators want
a readable error chain showing where things went wrong, not pattern-
matching on every variant.

A shared "uber error" crate that every other crate depends on creates
a foundation-coupling crate and forces every error change to ripple
through the workspace. We want errors to be local.

## Decision

Every `caliban-*` library crate defines its own `Error` enum using
`thiserror`, and exposes:

```rust
pub type Result<T> = std::result::Result<T, Error>;
```

Cross-crate errors convert at boundaries with `#[from]` or explicit
`From` impls. No shared error crate.

The `caliban` binary will use `anyhow::Result` in `main()` and top-level
command handlers once real command logic exists. `?` propagates errors
with context using `.context("...")` from `anyhow::Context`.

At Layer 0 the binary is an argv-only stub returning `std::process::ExitCode`
directly (so it can distinguish exit codes 0 / 2 for success vs. misuse);
`anyhow` is declared as a workspace-inherited dependency and will be
imported as soon as the first error-propagating command lands.

## Consequences

- **Positive:** adding a new error variant is local to one crate.
  Library consumers can match precisely; binary code gets readable
  context. No god-error-crate.
- **Negative:** slight boilerplate per library (the `Error` enum
  and `Result` alias). `From` impls must be added at boundaries.
- **Revisit if:** a real shared error type emerges (e.g., a
  cross-crate "Cancelled" or "Timeout" that every layer must surface
  identically).
