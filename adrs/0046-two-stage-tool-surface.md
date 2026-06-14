# ADR 0046 · Two-stage tool surface — lazy MCP schema loading + ToolSearch

- **Status:** accepted
- **Date:** 2026-05-31
- **Spec:** [`docs/superpowers/specs/2026-05-31-two-stage-tool-surface-design.md`](../docs/superpowers/specs/2026-05-31-two-stage-tool-surface-design.md)
- **Related:** ADR 0017 (MCP client architecture), ADR 0021 (Sub-agent
  primitive), ADR 0023 (MCP v2), ADR 0026 (Settings layering),
  ADR 0037 (Sub-agent isolation + fleet), ADR 0043 (`arc-swap` shared
  state).

## Context

`ToolRegistry::to_caliban_tools()` is invoked once per turn at
`crates/caliban-agent-core/src/stream/mod.rs:497-523`, cloning every
registered tool's name + description + JSON Schema into the wire
payload. Built-ins are bounded (~14 entries) but MCP tools scale
linearly with configured servers — three average MCP servers can add
~20K tokens/turn of dormant tool advertising before history is
considered. The problem is structural and will worsen as the
MCP/plugin ecosystem grows, which calls for a design doc + ADR +
multi-PR sequence; this ADR is that decision.

## Decision

1. **Introduce a single new built-in `ToolSearch`** that returns
   matched MCP tools with their full JSON Schemas and activates them
   for the rest of the session in a single round-trip. No separate
   `Activate` tool; no two-step UX.

2. **Store activation state in a sidecar `McpActivationSet`** held by
   `Agent` as `Arc<ArcSwap<McpActivationSet>>`, following the
   read-mostly pattern of ADR 0043. `ToolRegistry` is unchanged; an
   added `to_caliban_tools_filtered(&WireFilter)` returns the per-turn
   wire subset.

3. **Filter MCP tools, never built-ins.** The v1 scope is MCP-only
   laziness; built-ins (`Read`, `Grep`, `Glob`, `Edit`, `Bash`,
   `Write`, `WebFetch`, `WebSearch`, `TodoWrite`, `Skill`,
   `AgentTool`, `EnterPlanMode`/`ExitPlanMode`, memory tools) stay
   always-present. Plugin-tool laziness is moot today (plugins
   contribute skill roots, not tools).

4. **Sticky per session, LRU evict at cap.** Activations persist for
   the rest of the session; `tools.max_active_schemas` (default 24) is
   a soft cap. New activations beyond the cap evict the least
   recently used entry, reported in the `ToolSearch` response text
   so the model sees what dropped.

5. **Sub-agent inheritance is opt-out via frontmatter.** `AgentTool`
   frontmatter gains `inherit_active_mcp: Option<bool>` defaulting to
   `true`. When true, `install_sub_agent` snapshots the parent's
   `McpActivationSet`; when false the child starts fresh. The existing
   `tools: [...]` allowlist still filters.

6. **Default off; opt-in via `tools.lazy_mcp = true`.** Conservative
   v1; flip to default-on in v1.1 after validation. Per-server
   override via `mcp.toml` (`[server.X] lazy = false`) pins
   always-hot servers (e.g. a memory/notes server) to eager mode.

7. **Belt-and-suspenders discovery.** When `lazy_mcp = true` and at
   least one MCP tool is gated, splice a fixed paragraph into the
   system prompt explaining `ToolSearch` plus the deferred count;
   the ToolSearch tool description itself also names the affordance.

8. **`/context` surfaces the active set** as `MCP active: N/cap
   (a, b, c)`. `/usage` is intentionally not touched in v1 (no
   honest counterfactual reporting yet).

## Consequences

- **Positive**: removes a linear-in-MCP-cardinality token tax from
  every turn; matches the function-calling pattern many models are
  trained on; structural readiness for plugin-tool laziness later;
  no protocol change for the eager path (default behavior is
  byte-identical).
- **Positive**: single read-mostly `ArcSwap` for activation state
  fits the existing concurrency model and makes sub-agent snapshot
  trivial.
- **Negative**: introduces a model-facing contract (search-then-call)
  that requires the model to read system-prompt guidance; some
  weaker models may not pick up the pattern reliably (mitigation: it
  is opt-in in v1, and the "model issues tool_use without searching
  first" path still works via registry dispatch + auto-activation).
- **Negative**: tool-list cache prefix is invalidated on each
  activation; a future split-cache optimisation is sketched in the
  spec but out of scope for v1.
- **Compat window**: default `false` for v1; v1.1 flips default to
  `true` (parity matrix rows F.ToolSearch / F.WaitForMcpServers move
  🔴 → 🟡 in v1, 🟡 → ✅ in v1.1).

## Revisit if

- Activation set's read-mostly assumption breaks down (e.g. the
  model starts calling `ToolSearch` every turn) — would warrant a
  finer-grained cache strategy.
- Built-in tool palette grows substantially (e.g. a wave of new
  builtins) and the cardinality problem returns for built-ins —
  would motivate a separate built-in laziness spec.
- A model is observed to reliably ignore the deferred-block guidance
  — would motivate a stronger affordance (e.g. forcing an inert
  ToolSearch tool_use as the first turn under lazy mode).
- Activation persistence across session restart becomes a hot
  request — would warrant the v1.1 follow-up sketched in the spec's
  "Open questions" section.
