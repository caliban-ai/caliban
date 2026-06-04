# Architecture & ADRs

Caliban captures every significant architectural decision in an Architecture Decision Record (ADR). Each ADR states the context, the decision, and its consequences — giving contributors (and curious operators) the rationale behind the design, not just the outcome.

ADRs live in the `adrs/` directory of the repository. They use a lightweight [MADR-lite](https://adr.github.io/madr/) format and carry a status:

- **accepted** — currently in effect
- **superseded** — replaced by a later ADR; kept for history
- **proposed** — under discussion, not yet in effect
- **rejected** — considered and explicitly declined

```admonish note title="This is the contributor/internals layer"
You do not need to read ADRs to use caliban. They exist for contributors and operators who want to understand *why* something works the way it does. For crate orientation, see [Crate Map](./crate-map.md).
```

---

## ADR index

### Foundation

| # | Title | Status |
|---|---|---|
| [0001](https://github.com/johnford2002/caliban/blob/main/adrs/0001-async-runtime.md) | Async runtime → `tokio` | accepted |
| [0002](https://github.com/johnford2002/caliban/blob/main/adrs/0002-error-model.md) | Error model → `thiserror` for libs, `anyhow` for binary | accepted |
| [0003](https://github.com/johnford2002/caliban/blob/main/adrs/0003-license-agpl-3.0.md) | License → `AGPL-3.0-only` | accepted |
| [0004](https://github.com/johnford2002/caliban/blob/main/adrs/0004-naming-conventions.md) | Naming → `caliban-*` libraries, `caliban` binary | accepted |
| [0005](https://github.com/johnford2002/caliban/blob/main/adrs/0005-workspace-layout.md) | Workspace layout → `crates/` for libs, binaries at root | accepted |

### Provider & message model

| # | Title | Status |
|---|---|---|
| [0006](https://github.com/johnford2002/caliban/blob/main/adrs/0006-message-schema-ir.md) | Message schema → provider-neutral IR | accepted |
| [0007](https://github.com/johnford2002/caliban/blob/main/adrs/0007-transport-trait-pattern.md) | Schema/transport factoring via Transport trait | accepted |
| [0008](https://github.com/johnford2002/caliban/blob/main/adrs/0008-system-role-positional.md) | `Role::System` is positional (leading-only) | accepted |

### Agent core

| # | Title | Status |
|---|---|---|
| [0009](https://github.com/johnford2002/caliban/blob/main/adrs/0009-agent-core-design.md) | Agent-core design (stream-as-primitive, sequential tools, opt-in compaction) | accepted (sequential-tools clause superseded by 0016) |
| [0010](https://github.com/johnford2002/caliban/blob/main/adrs/0010-workspace-root.md) | WorkspaceRoot path resolution + opt-in restricted mode | accepted |
| [0016](https://github.com/johnford2002/caliban/blob/main/adrs/0016-parallel-tool-dispatch.md) | Parallel tool dispatch (semaphore-bounded; supersedes 0009 sequential clause) | accepted |
| [0021](https://github.com/johnford2002/caliban/blob/main/adrs/0021-sub-agent-primitive.md) | Sub-agent primitive (`AgentTool`; synchronous in-process; allowlist-filtered registry) | accepted |

### TUI & sessions

| # | Title | Status |
|---|---|---|
| [0011](https://github.com/johnford2002/caliban/blob/main/adrs/0011-sessions-and-repl.md) | Sessions persisted to disk + interactive REPL | accepted |
| [0012](https://github.com/johnford2002/caliban/blob/main/adrs/0012-tui-via-ratatui.md) | TUI via ratatui (replacing the rustyline REPL) | accepted |
| [0013](https://github.com/johnford2002/caliban/blob/main/adrs/0013-tui-overlays.md) | TUI overlays + layout v2 | accepted |
| [0014](https://github.com/johnford2002/caliban/blob/main/adrs/0014-system-prompt-and-tui-fixes.md) | Default system prompt + TUI stall fixes + debug logging | accepted |
| [0015](https://github.com/johnford2002/caliban/blob/main/adrs/0015-context-and-path-fixes.md) | Context preservation + path conventions (~ expansion) | accepted |
| [0027](https://github.com/johnford2002/caliban/blob/main/adrs/0027-tui-ergonomics.md) | TUI ergonomics (@file, !, Ctrl+G, Ask modal, transcript viewer) | accepted |
| [0041](https://github.com/johnford2002/caliban/blob/main/adrs/0041-tui-redraw-tick-closeout.md) | TUI redraw tick — close-out (resolves 0014 open question) | accepted |

### Memory & checkpointing

| # | Title | Status |
|---|---|---|
| [0018](https://github.com/johnford2002/caliban/blob/main/adrs/0018-memory-tier-model.md) | Memory tier model (global / project / auto-memory; spliced into system prompt) | accepted |
| [0028](https://github.com/johnford2002/caliban/blob/main/adrs/0028-checkpointing-rewind.md) | Auto-checkpointing + `/rewind` | accepted |
| [0035](https://github.com/johnford2002/caliban/blob/main/adrs/0035-auto-memory.md) | Auto-memory (model-written notes per project) | accepted |
| [0036](https://github.com/johnford2002/caliban/blob/main/adrs/0036-claudemd-ancestry-and-imports.md) | CLAUDE.md ancestor walk + `@`-imports | accepted |

### Permissions & safety

| # | Title | Status |
|---|---|---|
| [0020](https://github.com/johnford2002/caliban/blob/main/adrs/0020-permission-rules.md) | Permission rules layered on Hooks (TOML rule sources; interactive Ask) | accepted |
| [0029](https://github.com/johnford2002/caliban/blob/main/adrs/0029-permission-modes-and-auto-mode.md) | Permission modes (acceptEdits / auto / dontAsk / bypassPermissions) + auto-mode classifier | accepted |
| [0032](https://github.com/johnford2002/caliban/blob/main/adrs/0032-os-sandbox.md) | OS-level sandbox (macOS Seatbelt + Linux bubblewrap) | accepted |
| [0045](https://github.com/johnford2002/caliban/blob/main/adrs/0045-permissions-v2-and-toml-primary-config.md) | Permissions v2 — TOML-primary config + richer rule schema | accepted |

### Configuration & settings

| # | Title | Status |
|---|---|---|
| [0026](https://github.com/johnford2002/caliban/blob/main/adrs/0026-settings-layering.md) | Unified settings hierarchy (managed > user > project > local) | accepted |
| [0043](https://github.com/johnford2002/caliban/blob/main/adrs/0043-arc-swap-shared-state.md) | `arc-swap` as the read-mostly shared-state primitive | accepted |

### Extensibility: hooks, skills, plugins, output styles

| # | Title | Status |
|---|---|---|
| [0019](https://github.com/johnford2002/caliban/blob/main/adrs/0019-skills-loading.md) | Skills loading & invocation (frontmatter + body; `SkillTool` on-demand load) | accepted |
| [0024](https://github.com/johnford2002/caliban/blob/main/adrs/0024-hook-event-taxonomy.md) | Hook event taxonomy (expanded events + handler types) | accepted |
| [0030](https://github.com/johnford2002/caliban/blob/main/adrs/0030-plugin-packaging.md) | Plugin packaging (skills + hooks + agents + MCP + output-styles bundles) | accepted |
| [0031](https://github.com/johnford2002/caliban/blob/main/adrs/0031-output-styles.md) | Output styles (Default / Proactive / Explanatory / Learning + custom) | accepted |
| [0040](https://github.com/johnford2002/caliban/blob/main/adrs/0040-slash-command-registry.md) | Slash command registry (extensible `SlashCommand` trait) | accepted |

### MCP

| # | Title | Status |
|---|---|---|
| [0017](https://github.com/johnford2002/caliban/blob/main/adrs/0017-mcp-client-architecture.md) | MCP client architecture (stdio v1; tools surface as `mcp__<server>__<tool>`) | accepted |
| [0023](https://github.com/johnford2002/caliban/blob/main/adrs/0023-mcp-v2-transports-and-oauth.md) | MCP v2 — transports, OAuth, elicitation, resources | accepted |
| [0044](https://github.com/johnford2002/caliban/blob/main/adrs/0044-rmcp-version-pin.md) | `rmcp` 1.7 version pin (dedicated-PR bumps) | accepted |
| [0046](https://github.com/johnford2002/caliban/blob/main/adrs/0046-two-stage-tool-surface.md) | Two-stage tool surface — lazy MCP schema loading + ToolSearch | accepted |

### Model router & providers

| # | Title | Status |
|---|---|---|
| [0022](https://github.com/johnford2002/caliban/blob/main/adrs/0022-model-routing-architecture.md) | Model routing architecture (Layer 3 `caliban-model-router`; router-impl-Provider) | accepted |
| [0034](https://github.com/johnford2002/caliban/blob/main/adrs/0034-bedrock-and-vertex-providers.md) | Bedrock + Vertex providers | accepted |
| [0038](https://github.com/johnford2002/caliban/blob/main/adrs/0038-model-router-v2.md) | Model router v2 (fallback / hedging / circuit breakers / capability filtering) | accepted |
| [0039](https://github.com/johnford2002/caliban/blob/main/adrs/0039-image-and-vision-input.md) | Image / vision input | accepted |

### Headless / CI & observability

| # | Title | Status |
|---|---|---|
| [0025](https://github.com/johnford2002/caliban/blob/main/adrs/0025-headless-output-protocol.md) | Headless / print mode + JSON output protocol | accepted |
| [0033](https://github.com/johnford2002/caliban/blob/main/adrs/0033-opentelemetry-and-cost.md) | OpenTelemetry export + cost accounting | accepted |

### Sub-agents & background fleet

| # | Title | Status |
|---|---|---|
| [0037](https://github.com/johnford2002/caliban/blob/main/adrs/0037-subagent-isolation-and-background-fleet.md) | Sub-agent worktree isolation + background fleet | accepted |
| [0042](https://github.com/johnford2002/caliban/blob/main/adrs/0042-caliband-binary-placement.md) | `caliband` sibling-binary placement (under `caliban-supervisor`) | accepted |
