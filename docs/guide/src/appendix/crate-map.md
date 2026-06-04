# Crate Map

The caliban workspace is organised into ~24 crates across four main layers. This page gives an operator-facing orientation — enough to know which crate to look at when reading a log line, error message, or ADR. For architecture rationale, see [Architecture & ADRs](./adrs.md).

```admonish note
This map is for the curious. You do not need to know these crates to use caliban — they are implementation details that surface only in debug logs, error messages, and ADR references.
```

---

## Layer 1 — Foundation

Shared types, abstractions, and utilities that every other layer depends on.

| Crate | Purpose |
|---|---|
| `caliban-common` | Provider-neutral message IR, shared error types, and cross-crate utilities |
| `caliban-settings` | Unified settings hierarchy (managed > user > project > local); file loading, schema validation, live reload, `apiKeyHelper` pool |

## Layer 2 — Providers

One adapter per model API. Each translates caliban's message IR to the provider's wire format and back.

| Crate | Purpose |
|---|---|
| `caliban-provider` | Provider trait definition and shared provider types |
| `caliban-provider-anthropic` | Anthropic (Claude) adapter via Anthropic Messages API |
| `caliban-provider-openai` | OpenAI adapter; also used for LM Studio, vLLM, and other OpenAI-compatible servers |
| `caliban-provider-ollama` | Ollama adapter (native `/api/chat` endpoint, GGUF tool-call parsing) |
| `caliban-provider-google` | Google AI Studio / Gemini adapter |
| `caliban-provider-bedrock` | AWS Bedrock adapter (ADR 0034) |
| `caliban-provider-vertex` | Google Cloud Vertex AI adapter (ADR 0034) |
| `caliban-model-router` | Purpose-keyed routing, fallback chains, hedging, circuit breakers, capability filtering (ADR 0022, 0038) |

## Layer 3 — Agent Core

The runtime that drives the model → tool → model loop.

| Crate | Purpose |
|---|---|
| `caliban-agent-core` | Agent loop, turn handling, compaction strategies, permission dispatch, sub-agent orchestration |
| `caliban-tools-builtin` | Built-in tools: Read, Write, Edit, Bash, Glob, Grep, WebFetch, TodoWrite, AgentTool, NotebookEdit, and others |
| `caliban-sandbox` | OS-level tool confinement (macOS Seatbelt, Linux bubblewrap) (ADR 0032) |
| `caliban-skills` | Skill discovery, frontmatter parsing, and `SkillTool` invocation (ADR 0019) |
| `caliban-mcp-client` | MCP server lifecycle: spawn, handshake, `list_tools`, transports, OAuth (ADR 0017, 0023) |
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
| `caliban-supervisor` | Background agent fleet and `caliband` supervisor daemon (ADR 0037, 0042) |

## The binary

| Crate | Purpose |
|---|---|
| `caliban` | The `caliban` binary: CLI parsing (`args.rs`), startup pipeline, TUI (ratatui), headless dispatch, and subcommand handlers |
