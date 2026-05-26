# ADR 0042 · `caliband` sibling-binary placement

- **Status:** accepted
- **Date:** 2026-05-26

## Context

The workspace declares two binaries:

- `caliban` — the primary user-facing TUI/CLI. Source at the workspace
  root (`caliban/src/main.rs`).
- `caliband` — the supervisor daemon (ADR 0037). Source nested under
  its owning crate at `crates/caliban-supervisor/src/bin/caliband.rs`,
  declared via the `[[bin]]` entry in
  `crates/caliban-supervisor/Cargo.toml`.

ADR 0005 ("Workspace layout") establishes the convention that
"primary" binaries live at the workspace root. `caliband` does not —
it lives nested under its owning crate. ADR 0037 introduces the
daemon obliquely (its name, its on-disk paths, and its protocol) but
does not document the placement choice. The 2026-05-25 ADR conformance
audit (Finding 7) flagged this gap.

## Decision

`caliband` stays nested under `caliban-supervisor` as a secondary
binary, with its `[[bin]]` declaration in the supervisor crate's
`Cargo.toml`.

## Consequences

- **Clean process boundary** between the user-facing `caliban` CLI/TUI
  and the supervisor daemon. The two never share a `main` entry point;
  they communicate over a Unix socket per ADR 0037.
- **Direct crate access.** `caliband` consumes
  `caliban-supervisor`'s modules directly without going through a
  public API surface — appropriate because they ship together.
- **No accidental dispatch.** Launching `caliban` never accidentally
  invokes `caliband`'s `main` (or vice versa); they're distinct
  binaries from `cargo` and from the user's `$PATH`.
- **`cargo install` requires `--bin caliband`** explicitly. The
  supervisor crate's README documents this; the `caliban agents`
  subcommand spawns `caliband` from the same install prefix as
  `caliban` (per ADR 0037).
- **Workspace-root parsimony.** The root stays focused on the primary
  product (`caliban`); the daemon is appropriately filed under the
  crate that owns its implementation.

## Why this differs from ADR 0005

ADR 0005's "binaries at root" rule was written assuming a single
binary. With two, the rule needs nuance:

- A binary whose sole purpose is to expose a crate's library
  functionality as an executable belongs **with that crate**.
- A binary that integrates many crates into the product surface
  belongs **at the workspace root**.

`caliban` is the latter; `caliband` is the former. This ADR amends
ADR 0005's rule by adding that nuance.

## Revisit if

- A third sibling binary appears (e.g., a `caliban-mcp` daemon for
  remote MCP servers). At that point the workspace should consider
  a `binaries/` subdirectory rather than continuing the case-by-case
  pattern.
- `caliband` outgrows its current sole consumer (the `caliban agents`
  subcommand) and starts being launched standalone by other tooling —
  it might then belong at the root for discoverability.

## References

- ADR 0005 (workspace layout — sets the "binaries at root" convention
  this ADR refines).
- ADR 0037 (subagent isolation + fleet — introduces `caliband`).
- 2026-05-25 ADR conformance audit, Finding 7.
- Source: `crates/caliban-supervisor/src/bin/caliband.rs`.
- Declaration: `crates/caliban-supervisor/Cargo.toml` (`[[bin]]`).
