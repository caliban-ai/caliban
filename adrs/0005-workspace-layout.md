# ADR 0005 · Workspace layout → `crates/` for libraries, binaries at root

- **Status:** accepted
- **Date:** 2026-05-22

## Context

The workspace is planned to grow to ~11 crates across 4 layers
(foundation, integration, routing, UX surfaces). Layout patterns
seen in Rust workspaces:

- **Flat** (`crate1/`, `crate2/` at root) — used by tokio, serde,
  axum. Simpler for small workspaces, clutters root past ~8 crates.
- **All-in-`crates/`** — used by ruff. Binary and libraries
  intermingled; clean root but binary entry points are buried.
- **Apps/libs split** (`crates/` for libs, `apps/` for bins) —
  principled but less common; over-engineered for our size.
- **Binaries at root, libraries in `crates/`** — used by deno
  (with `cli/`), zed, helix. Entry points are top-level visible;
  libraries are clearly cataloged.

## Decision

Adopt the last pattern: library crates under `crates/caliban-<name>/`,
binary crates as first-class subdirectories of the workspace root
(`caliban/`, future `caliban-tui/`, `caliban-orchestrator/`) rather
than nested under a shared parent directory. Workspace members are
listed explicitly in root `Cargo.toml`, no globs.

## Consequences

- **Positive:** root-level `ls` reveals entry points (binaries) and
  config files. `crates/` reveals reusable libraries. Explicit member
  list catches typos and missing members at workspace-parse time.
- **Negative:** new-crate workflow has two patterns rather than one
  (`cargo new --lib crates/<name>` for libraries,
  `cargo new <name>` at root for binaries). Documented in README.
- **Revisit if:** the workspace stays small (<5 crates) and the
  `crates/` directory feels like overhead, or grows past ~25 crates
  where a flat-but-grouped layout (e.g. `crates/layer-1/`,
  `crates/layer-2/`) becomes warranted.
