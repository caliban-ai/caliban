# Summary

[Caliban User Guide](./introduction.md)

# Introduction

- [What Is Caliban?](./intro/what-is-caliban.md)
- [Philosophy](./intro/philosophy.md)
- [Project Status](./intro/status.md)

# Getting Started

- [Installation & Building](./getting-started/installation.md)
- [Your First Session](./getting-started/first-session.md)
- [The Interactive TUI](./getting-started/tui.md)
- [Headless Basics](./getting-started/headless.md)

# Interactive Use

- [Sessions & Persistence](./interactive/sessions.md)
- [The TUI in Depth](./interactive/tui-in-depth.md)
- [Prompts, Attachments & Images](./interactive/prompts-attachments.md)
- [Slash Commands](./interactive/slash-commands.md)

# Providers & Models

- [Supported Providers](./providers/overview.md)
- [Configuring Providers & API Keys](./providers/configuration.md)
- [Model Selection](./providers/models.md)
- [The Model Router](./providers/router.md)

# Configuration

- [Settings Layering](./configuration/settings-layering.md)
- [File Locations](./configuration/locations.md)
- [Settings Reference](./configuration/reference.md)
- [Config Commands](./configuration/commands.md)

# Permissions

- [Concepts](./permissions/concepts.md)
- [Pattern Grammar](./permissions/patterns.md)
- [Permission Modes](./permissions/modes.md)
- [Managing Rules](./permissions/managing.md)
- [Headless & Audit](./permissions/headless-and-audit.md)

# Tools

- [Built-in Tools](./tools/builtin.md)
- [Tool Execution](./tools/execution.md)
- [The OS Sandbox](./tools/sandbox.md)

# Extending Caliban

- [Skills](./extending/skills.md)
- [Custom Slash Commands](./extending/slash-commands.md)
- [Hooks](./extending/hooks.md)
- [MCP Servers](./extending/mcp.md)
- [Plugins](./extending/plugins.md)
- [Output Styles](./extending/output-styles.md)

# Sub-agents & Background Work

- [Sub-agents](./subagents/overview.md)
- [The Background Fleet](./subagents/background-fleet.md)
- [Worktree Isolation](./subagents/worktrees.md)

# Memory & Context

- [Memory Tiers](./memory/tiers.md)
- [CLAUDE.md & Imports](./memory/claude-md.md)
- [Auto-Memory](./memory/auto-memory.md)
- [Checkpoints & Rewind](./memory/checkpoints.md)
- [Context & Compaction](./memory/context-compaction.md)

# Automation & Headless

- [Print Mode](./automation/print-mode.md)
- [The stream-json Protocol](./automation/stream-json.md)
- [Structured Output](./automation/structured-output.md)
- [CI Patterns](./automation/ci.md)

# Observability

- [Telemetry & Cost](./observability/telemetry.md)
- [Health Checks](./observability/doctor.md)

# Reference

- [CLI Reference](./reference/cli.md)
- [Settings Schema](./reference/settings-schema.md)
- [Slash Command Index](./reference/slash-index.md)
- [Environment Variables](./reference/env-vars.md)
- [Files & Directories](./reference/paths.md)

# Help

- [Troubleshooting](./troubleshooting.md)

# Appendix

- [Glossary](./appendix/glossary.md)
- [Parity vs Claude Code](./appendix/parity.md)
- [Crate Map](./appendix/crate-map.md)
- [Architecture & ADRs](./appendix/adrs.md)

# Changelog

- [Changelog](./changelog.md)

# Architecture Decisions

- [ADR Index](./adr/index.md)
<!-- adrs -->
  - [ADR 0000 · Record architecture decisions](./adr/0000-architecture-decision-records.md)
  - [ADR 0001 · Async runtime → `tokio`](./adr/0001-async-runtime.md)
  - [ADR 0002 · Error model → `thiserror` for libraries, `anyhow` for binary](./adr/0002-error-model.md)
  - [ADR 0003 · License → `AGPL-3.0-only`](./adr/0003-license-agpl-3.0.md)
  - [ADR 0004 · Naming → `caliban-*` libraries, `caliban` binary](./adr/0004-naming-conventions.md)
  - [ADR 0005 · Workspace layout → `crates/` for libraries, binaries at root](./adr/0005-workspace-layout.md)
  - [ADR 0006 · Message schema → provider-neutral IR](./adr/0006-message-schema-ir.md)
  - [ADR 0007 · Schema/transport factoring via Transport trait](./adr/0007-transport-trait-pattern.md)
  - [ADR 0008 · Role::System messages are positional (leading-only)](./adr/0008-system-role-positional.md)
  - [ADR 0009 · Agent-core design (stream-as-primitive, sequential tools, opt-in compaction)](./adr/0009-agent-core-design.md)
  - [ADR 0010 · WorkspaceRoot path resolution + opt-in restricted mode](./adr/0010-workspace-root.md)
  - [ADR 0011 · Sessions persisted to disk + interactive REPL](./adr/0011-sessions-and-repl.md)
  - [ADR 0012 · TUI via ratatui (replacing the rustyline REPL)](./adr/0012-tui-via-ratatui.md)
  - [ADR 0013 · TUI overlays + layout v2 (input bracketed by horizontal rules)](./adr/0013-tui-overlays.md)
  - [ADR 0014 · Default system prompt + TUI stall fixes + debug logging](./adr/0014-system-prompt-and-tui-fixes.md)
  - [ADR 0015 · Context preservation + path conventions (~/dev fix)](./adr/0015-context-and-path-fixes.md)
  - [ADR 0016 · Parallel tool dispatch (supersedes ADR 0009 §"sequential tools")](./adr/0016-parallel-tool-dispatch.md)
  - [ADR 0017 · MCP client architecture](./adr/0017-mcp-client-architecture.md)
  - [ADR 0018 · Memory tier model (CLAUDE.md ingestion + auto-memory)](./adr/0018-memory-tier-model.md)
  - [ADR 0019 · Skills loading](./adr/0019-skills-loading.md)
  - [ADR 0020 · Permission rules layered on top of `Hooks`](./adr/0020-permission-rules.md)
  - [ADR 0021 · Sub-agent primitive via `AgentTool`](./adr/0021-sub-agent-primitive.md)
  - [ADR 0022 · Model routing architecture](./adr/0022-model-routing-architecture.md)
  - [ADR 0023 · MCP v2 — transports, OAuth, elicitation, resources](./adr/0023-mcp-v2-transports-and-oauth.md)
  - [ADR 0024 · Hook event taxonomy + external handler types](./adr/0024-hook-event-taxonomy.md)
  - [ADR 0025 · Headless `-p` mode + JSON output protocol](./adr/0025-headless-output-protocol.md)
  - [ADR 0026 · Layered settings.json + `/config` editor](./adr/0026-settings-layering.md)
  - [ADR 0027 · TUI ergonomics pack](./adr/0027-tui-ergonomics.md)
  - [ADR 0028 · Checkpointing + `/rewind`](./adr/0028-checkpointing-rewind.md)
  - [ADR 0029 · Permission modes + auto-mode classifier](./adr/0029-permission-modes-and-auto-mode.md)
  - [ADR 0030 · Plugin packaging](./adr/0030-plugin-packaging.md)
  - [ADR 0031 · Output styles](./adr/0031-output-styles.md)
  - [ADR 0032 · OS-level sandbox](./adr/0032-os-sandbox.md)
  - [ADR 0033 · OpenTelemetry export + cost tracking](./adr/0033-opentelemetry-and-cost.md)
  - [ADR 0034 · Bedrock + Vertex providers](./adr/0034-bedrock-and-vertex-providers.md)
  - [ADR 0035 · Auto-memory (model-written notes)](./adr/0035-auto-memory.md)
  - [ADR 0036 · CLAUDE.md ancestor walk + `@`-imports](./adr/0036-claudemd-ancestry-and-imports.md)
  - [ADR 0037 · Sub-agent worktree isolation + background fleet](./adr/0037-subagent-isolation-and-background-fleet.md)
  - [ADR 0038 · Model router v2 — fallback, hedging, breakers, capabilities, binary wiring](./adr/0038-model-router-v2.md)
  - [ADR 0039 · Image + vision input](./adr/0039-image-and-vision-input.md)
  - [ADR 0040 · Slash command registry](./adr/0040-slash-command-registry.md)
  - [ADR 0041 · TUI redraw tick close-out](./adr/0041-tui-redraw-tick-closeout.md)
  - [ADR 0042 · `caliband` sibling-binary placement](./adr/0042-caliband-binary-placement.md)
  - [ADR 0043 · `arc-swap` as the read-mostly shared-state primitive](./adr/0043-arc-swap-shared-state.md)
  - [ADR 0044 · `rmcp` 1.7 version pin](./adr/0044-rmcp-version-pin.md)
  - [ADR 0045 · Permissions v2 — TOML-primary config + richer rule schema](./adr/0045-permissions-v2-and-toml-primary-config.md)
  - [ADR 0046 · Two-stage tool surface — lazy MCP schema loading + ToolSearch](./adr/0046-two-stage-tool-surface.md)
  - [ADR 0047 · Interactive background sub-agents (idle / await-input)](./adr/0047-interactive-background-subagents.md)
