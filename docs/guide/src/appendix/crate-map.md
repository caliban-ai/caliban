# Crate Map

The caliban workspace is organised into ~26 crates across four main layers. This page gives an operator-facing orientation — enough to know which crate to look at when reading a log line, error message, or ADR. For architecture rationale, see [Architecture & ADRs](./adrs.md).

```admonish note
This map is for the curious. You do not need to know these crates to use caliban — they are implementation details that surface only in debug logs, error messages, and ADR references.
```

---

## Layer 1 — Foundation

Shared types, abstractions, and utilities that every other layer depends on.

| Crate | Purpose |
|---|---|
| `caliban-common` | Provider-neutral message IR, shared error types, and cross-crate utilities |
| `caliban-config-types` | Leaf crate of shared configuration **data types** — the MCP server, permission rule, permission-mode, and hook config shapes, plus their `mcp.toml` / `hooks.toml` loaders. Both `caliban-settings` and the crates that consume those shapes depend on it *downward*, which is what lets settings store and project them without depending upward on its own consumers (ADR 0061) |
| `caliban-contract` | Caliban's launch and control-plane contract: the `CalibandLaunch` builder (flag/env name constants), provider credential-env helpers, and the supervisor wire types. Dependency-light (serde only, no daemon internals) so out-of-tree drivers — prospero, caliban-operator — can depend on it instead of hand-copying names (ADR 0059) |
| `caliban-settings` | Unified settings hierarchy (managed > user > project > local) plus the `CALIBAN_*` environment layer; file loading, schema validation, `apiKeyHelper` pool; live-reload watcher module (scaffolded, not yet wired). Names the config shapes it stores via `caliban-config-types` (ADR 0061) |

## Layer 2 — Providers

One adapter per model API. Each translates caliban's message IR to the provider's wire format and back.

| Crate | Purpose |
|---|---|
| `caliban-provider` | Provider trait definition and shared provider types |
| `caliban-provider-anthropic` | Anthropic (Claude) adapter via Anthropic Messages API |
| `caliban-provider-openai` | OpenAI adapter; also used for LM Studio, vLLM, and other OpenAI-compatible servers |
| `caliban-provider-google` | Google AI Studio / Gemini adapter |
| `caliban-provider-bedrock` | AWS Bedrock adapter (ADR 0034) |
| `caliban-provider-vertex` | Google Cloud Vertex AI adapter (ADR 0034) |
| `caliban-model-router` | Purpose-keyed routing, fallback chains, hedging, circuit breakers, capability filtering (ADR 0022, 0038) |

## Layer 3 — Agent Core

The runtime that drives the model → tool → model loop.

| Crate | Purpose |
|---|---|
| `caliban-agent-core` | Agent loop, turn handling, compaction strategies, permission dispatch, sub-agent orchestration. The permission, permission-mode, and hook *config types* it re-exports now live in `caliban-config-types` (ADR 0061) |
| `caliban-tools-builtin` | Built-in tools: Read, Write, Edit, Bash, Glob, Grep, WebFetch, TodoWrite, AgentTool, NotebookEdit, and others |
| `caliban-sandbox` | OS-level tool confinement (macOS Seatbelt, Linux bubblewrap) (ADR 0032) |
| `caliban-skills` | Skill discovery, frontmatter parsing, and `SkillTool` invocation (ADR 0019) |
| `caliban-mcp-client` | MCP server lifecycle: spawn, handshake, `list_tools`, transports, OAuth (ADR 0017, 0023). The `mcp.toml` config types and loader it re-exports now live in `caliban-config-types` (ADR 0061) |
| `caliban-plugins` | Plugin package management: manifest parsing, trust gating, namespace expansion (ADR 0030) |
| `caliban-images` | Image / vision input: clipboard, `@path`, drag-and-drop, provider wire shapes (ADR 0039) |

## Layer 4 — Sessions, State & Infrastructure

Persistence, memory, observability, and the background fleet.

| Crate | Purpose |
|---|---|
| `caliban-sessions` | Session persistence (JSON on disk), load/save, session directory management |
| `caliban-checkpoint` | Per-prompt checkpoint snapshots and `/rewind` restoration (ADR 0028) |
| `caliban-memory` | Three-tier memory (global/project/auto-memory), CLAUDE.md ancestor walk and `@`-imports (ADR 0018, 0035, 0036) |
| `caliban-output-styles` | Built-in and custom output style loading and activation (ADR 0031) |
| `caliban-telemetry` | OpenTelemetry export, cost accounting, metric emission (ADR 0033) |
| `caliban-worktrees` | Git worktree creation and lifecycle management for sub-agent isolation (ADR 0037) |
| `caliban-supervisor` | Background agent fleet and `caliband` supervisor daemon, including the network session plane and graceful drain/resume of daemon agents. Re-exports its wire types from `caliban-contract`, so there is a single definition (ADR 0037, 0042, 0051, 0057, 0059) |
| `caliban-drive` | Transport-agnostic drive core (run / stream / status / input) behind `caliban mcp serve`, `acp serve`, and `http serve`, and behind a fleet worker's networked ACP endpoint (ADR 0055, 0059) |

## The binary

| Crate | Purpose |
|---|---|
| `caliban` | The `caliban` binary: CLI parsing (`args.rs`), startup pipeline, TUI (ratatui), headless dispatch, and subcommand handlers |
