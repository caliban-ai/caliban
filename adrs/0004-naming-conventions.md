# ADR 0004 · Naming → `caliban-*` libraries, `caliban` binary

- **Status:** accepted
- **Date:** 2026-05-22

## Context

Crate names on crates.io are globally unique. If we eventually publish,
we need names that aren't already taken and that signal ownership.
Within the workspace, naming conventions also affect ergonomics —
module paths, import statements, and clippy's `module_name_repetitions`
lint all interact with crate names.

## Decision

- **Library crates** use the `caliban-` prefix: `caliban-core`,
  `caliban-provider`, `caliban-agent-core`, etc. Directory name
  matches the package name.
- **Binary crate** is named `caliban`. Its package name is `caliban`, so
  Cargo's default binary name matches; `caliban/Cargo.toml` makes this
  explicit with a `[[bin]] name = "caliban"` entry for clarity.
- **Internal module paths** drop the prefix where it would be
  redundant: `caliban_provider::ProviderClient`, NOT
  `caliban_provider::CalibanProviderClient`.
- **Clippy's `module_name_repetitions` lint is allowed** at the
  workspace level to support the internal-naming convention without
  fighting clippy on every type.

## Consequences

- **Positive:** all caliban crates can be reserved on crates.io ahead
  of public release. `cargo install caliban` works once published.
  Internal type names stay terse.
- **Negative:** ~9 extra characters of typing per crate reference in
  `Cargo.toml` dependency lists. Slight redundancy in long import
  paths (`caliban_core::caliban_core_specific::...` — avoided in
  practice by short module names).
- **Revisit if:** the workspace gains so many crates that the prefix
  becomes overhead, or if a sub-org / sub-product emerges that
  warrants its own prefix.
