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
| [0009](0009-agent-core-design.md) | Agent-core design (stream-as-primitive, sequential tools, opt-in compaction) | accepted (sequential-tools clause superseded by [0016](0016-parallel-tool-dispatch.md)) |
| [0010](0010-workspace-root.md) | WorkspaceRoot path resolution + opt-in restricted mode | accepted |
| [0011](0011-sessions-and-repl.md) | Sessions persisted to disk + interactive REPL | accepted |
| [0012](0012-tui-via-ratatui.md) | TUI via ratatui (replacing the rustyline REPL) | accepted |
| [0013](0013-tui-overlays.md) | TUI overlays + layout v2 | accepted |
| [0014](0014-system-prompt-and-tui-fixes.md) | Default system prompt + TUI stall fixes + debug logging | accepted |
| [0015](0015-context-and-path-fixes.md) | Context preservation + path conventions (~ expansion) | accepted |
| [0016](0016-parallel-tool-dispatch.md) | Parallel tool dispatch (semaphore-bounded; supersedes 0009 sequential clause) | accepted |
| [0017](0017-mcp-client-architecture.md) | MCP client architecture (stdio v1; tools surface as `mcp__<server>__<tool>`) | accepted |
| [0018](0018-memory-tier-model.md) | Memory tier model (global / project / auto-memory; spliced into system prompt) | accepted |
| [0019](0019-skills-loading.md) | Skills loading & invocation (frontmatter + body; `SkillTool` on-demand load) | proposed |
| [0020](0020-permission-rules.md) | Permission rules layered on Hooks (TOML rule sources; interactive Ask) | proposed |
| [0021](0021-sub-agent-primitive.md) | Sub-agent primitive (`AgentTool`; synchronous in-process; allowlist-filtered registry) | proposed |
| [0022](0022-model-routing-architecture.md) | Model routing architecture (Layer 3 `caliban-model-router`; router-impl-Provider) | accepted |
| [0023](0023-mcp-v2-transports-and-oauth.md) | MCP v2 — transports, OAuth, elicitation, resources | proposed |
| [0024](0024-hook-event-taxonomy.md) | Hook event taxonomy (expanded events + handler types) | proposed |
| [0025](0025-headless-output-protocol.md) | Headless / print mode + JSON output protocol | proposed |
| [0026](0026-settings-layering.md) | Unified settings hierarchy (managed > user > project > local) | proposed |
| [0027](0027-tui-ergonomics.md) | TUI ergonomics (@file, !, Ctrl+G, Ask modal, transcript viewer) | proposed |
| [0028](0028-checkpointing-rewind.md) | Auto-checkpointing + `/rewind` | proposed |
| [0029](0029-permission-modes-and-auto-mode.md) | Permission modes (acceptEdits / auto / dontAsk / bypassPermissions) + auto-mode classifier | proposed |
| [0030](0030-plugin-packaging.md) | Plugin packaging (skills + hooks + agents + MCP + output-styles bundles) | proposed |
| [0031](0031-output-styles.md) | Output styles (Default / Proactive / Explanatory / Learning + custom) | proposed |
| [0032](0032-os-sandbox.md) | OS-level sandbox (macOS Seatbelt + Linux bubblewrap) | proposed |
| [0033](0033-opentelemetry-and-cost.md) | OpenTelemetry export + cost accounting | proposed |
| [0034](0034-bedrock-and-vertex-providers.md) | Bedrock + Vertex providers | proposed |
| [0035](0035-auto-memory.md) | Auto-memory (model-written notes per project) | proposed |
| [0036](0036-claudemd-ancestry-and-imports.md) | CLAUDE.md ancestor walk + `@`-imports | proposed |
| [0037](0037-subagent-isolation-and-background-fleet.md) | Sub-agent worktree isolation + background fleet | proposed |
| [0038](0038-model-router-v2.md) | Model router v2 (fallback / hedging / circuit breakers / capability filtering) | proposed |
| [0039](0039-image-and-vision-input.md) | Image / vision input | proposed |
| [0040](0040-slash-command-registry.md) | Slash command registry (extensible `SlashCommand` trait) | proposed |

## Adding a new ADR

1. Pick the next available number.
2. Copy an existing ADR as a template.
3. Set status to `proposed` while open for discussion; flip to `accepted` once decided.
4. Add an entry to the table above.
