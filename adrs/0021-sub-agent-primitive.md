# ADR 0021 · Sub-agent primitive via `AgentTool`

- **Status:** accepted
- **Date:** 2026-05-23

## Context

caliban's turn loop is a single agent calling tools. Several real-use
patterns benefit from a sub-agent primitive: parallel search over a
large codebase without polluting the parent's context, subtasks with a
restricted tool palette, or delegating multi-step investigations whose
intermediate steps shouldn't bloat the parent transcript.

Claude Code has two related primitives — synchronous `Agent` (a tool)
and `Task` (async background runs you poll). We need the synchronous
one. Async `Task` is a separate, larger piece of work.

## Decision

### Surface: a tool, not a new core type

Sub-agents are spawned by the model invoking a built-in tool
`AgentTool`. Input: `{prompt, tool_allowlist?, model?}`. Output: one
`ContentBlock::Text` containing the sub-agent's final assistant text
(truncated to ~5000 chars).

### In-process, not child-process

The sub-agent runs an entire turn loop on its own `Agent` instance in
the same tokio runtime. Single binary, single runtime — cancellation
and tracing stay unified. Sub-agent shares the parent's `Provider`
instance, inheriting HTTP/2 multiplexing, the connection pool, and
Anthropic-side prompt cache locality. No IPC, no serialization. The
cost is no OS-level isolation, which is acceptable: the existing trust
model (operator already runs `BashTool`-capable code) doesn't gain
much from a child process.

### Construction via factory

`AgentTool::new(factory: Arc<dyn Fn(&AgentToolInput) -> Agent + Send + Sync>)`.
The factory is wired from `main` and closes over the parent's
provider, tool registry, and hooks. Each invocation builds a *fresh*
`Agent` with the parent's provider; `model` from input (or parent's);
a `ToolRegistry` filtered by `tool_allowlist`; and `max_turns = 20`
(operator-tunable in code, not from model input).

### Tool allowlist semantics

- `tool_allowlist: ["Read", "Grep"]` → sub-agent gets exactly those.
  Unknown names are silently dropped.
- `tool_allowlist: null` or omitted → sub-agent inherits every parent
  tool EXCEPT `AgentTool` itself.

No recursion in v1: `AgentTool` is filtered out of every sub-agent's
registry. Nested sub-agents are a v2 problem (depth limits, fan-out,
cost ceilings).

### Budgets

`max_turns` = 20 (hard). Sub-agent inherits the parent's `max_tokens`.
No per-call cost ceiling because we don't have a router yet; add
`max_cost_usd` later.

### Transcript representation

Parent transcript gets the `ToolUseBlock` (`name = "AgentTool"`, input
JSON) and a `ToolResultBlock` containing the sub-agent's final
assistant text (truncated to ~5000 chars). Intermediate sub-turns are
**not** persisted in the parent session — they live only in the
sub-agent's transient buffer. Debug logs capture the full trace.

### Not a `Task` primitive

Claude Code's `Task` is async-with-lifecycle (spawn, poll, cancel,
retrieve). `AgentTool::call` is synchronous: the parent's turn loop
blocks on the sub-agent's loop completing. Async `Task` is v2.

## Consequences

- **Positive:** unlocks the "parallel exploration without context
  bloat" pattern; reuses every existing primitive (`Agent`, `Hooks`,
  `ToolRegistry`, `CancellationToken`). Permissions apply to the
  sub-agent's tools just like the parent's, because the sub-agent's
  `Agent` is built with the same hooks chain.
- **Negative:** synchronous-only — if a sub-agent loop takes minutes,
  the parent appears stuck. Mitigation: sub-agent stream events bubble
  to the TUI via the parent's stream so the operator still sees
  progress. Token accounting at the parent level shows sub-agent usage
  as a single line (the `ToolUseBlock`); cost attribution to specific
  sub-turns lives only in the debug log.
- **Revisit if:** users routinely want to dispatch many sub-agents in
  parallel — at that point we promote `AgentTool` from synchronous to
  the v2 `Task` primitive and add lifecycle management.
