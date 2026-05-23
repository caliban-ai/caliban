# Architecture Decision Records

This directory contains durable architectural decisions for caliban, in
[MADR-lite](https://adr.github.io/madr/) format: each file states a single
decision with context, the decision itself, and consequences.

## Status legend

- **accepted** — the decision is currently in effect.
- **superseded** — the decision was replaced by a later ADR; the file is kept for history and links to its successor.
- **proposed** — under discussion; not in effect.
- **rejected** — considered and explicitly declined.

## Index

| # | Title | Status |
|---|---|---|
| [0001](0001-async-runtime.md) | Async runtime → `tokio` | accepted |
| [0002](0002-error-model.md) | Error model → `thiserror` for libs, `anyhow` for binary | accepted |
| [0003](0003-license-agpl-3.0.md) | License → `AGPL-3.0-only` | accepted |
| [0004](0004-naming-conventions.md) | Naming → `caliban-*` libraries, `caliban` binary | accepted |
| [0005](0005-workspace-layout.md) | Workspace layout → `crates/` for libs, binaries at root | accepted |
| [0006](0006-message-schema-ir.md) | Message schema → provider-neutral IR | accepted |
| [0007](0007-transport-trait-pattern.md) | Schema/transport factoring via Transport trait | accepted |
| [0008](0008-system-role-positional.md) | `Role::System` is positional (leading-only) | accepted |
| [0009](0009-agent-core-design.md) | Agent-core design (stream-as-primitive, sequential tools, opt-in compaction) | accepted |
| [0010](0010-workspace-root.md) | WorkspaceRoot path resolution + opt-in restricted mode | accepted |
| [0011](0011-sessions-and-repl.md) | Sessions persisted to disk + interactive REPL | accepted |
| [0012](0012-tui-via-ratatui.md) | TUI via ratatui (replacing the rustyline REPL) | accepted |
| [0013](0013-tui-overlays.md) | TUI overlays + layout v2 | accepted |
| [0014](0014-system-prompt-and-tui-fixes.md) | Default system prompt + TUI stall fixes + debug logging | accepted |

## Adding a new ADR

1. Pick the next available number.
2. Copy an existing ADR as a template.
3. Set status to `proposed` while open for discussion; flip to `accepted` once decided.
4. Add an entry to the table above.
