# ADR 0003 · License → `AGPL-3.0-only`

- **Status:** accepted
- **Date:** 2026-05-22

## Context

caliban is private now but designed to be open-sourced. The author
explicitly rejects permissive defaults (MIT, Apache-2.0): the goal is
to enforce community contribution from downstream users and hosted-
service providers, not maximize commercial adoption.

The relevant tiers of copyleft are:

- **GPL-3.0** — strong copyleft on distribution; SaaS providers can
  modify and host without releasing source (the "SaaS loophole").
- **AGPL-3.0** — closes the SaaS loophole: hosting modified code as
  a network service triggers the obligation to release source.
- **SSPL** — stronger than AGPL but not OSI-recognized as open source.
- **MPL-2.0** — file-level (weak) copyleft; consumers don't have to
  copyleft their downstream code.

## Decision

Every crate's `Cargo.toml` declares `license = "AGPL-3.0-only"` via
`license.workspace = true`. The full AGPL-3.0 text lives in `LICENSE`
at the workspace root. The README states implications. caliban crates
won't compose into permissive Rust projects on crates.io — this is
intentional.

## Consequences

- **Positive:** forks and hosted services must release modifications.
  Aligns with Mastodon, Nextcloud, Grafana (pre-2018), MongoDB (pre-2018).
  Author's stated philosophy of community contribution is enforced.
- **Negative:** caliban crates won't compose into permissive Rust
  projects on crates.io — depending on `caliban-*` makes the consumer
  AGPL. This is *intentional*: caliban is an end product, not a
  general-purpose library to be embedded.
- **Revisit if:** the AGPL is preventing a legitimate non-commercial
  use case the author wants to support. A future ADR could carve out
  exceptions or dual-license specific crates.
