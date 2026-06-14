# Sub-agent (`AgentTool`) — Design

**Date:** 2026-05-23
**Status:** Proposed
**Target branch:** `jf/docs/roadmap-post-webfetch`
**Sub-project of:** caliban Rust agent harness
**Depends on:** `caliban-agent-core` (`Agent`, `Hooks`, stream loop),
`caliban-tools-builtin`
**Related ADR:** [0021 — Sub-agent primitive](../../../docs/adr/0021-sub-agent-primitive.md)

## Goal

Add a built-in `AgentTool` that lets the parent agent spawn a synchronous
sub-agent with its own turn loop and a restricted tool palette. The
sub-agent runs in-process, shares the parent's provider, and returns a
single text result to the parent.

This unlocks two everyday patterns:

- **Restricted-tool subtasks** — "search the codebase for X" with a
  read-only allowlist (`Read`, `Grep`, `Glob`).
- **Context isolation** — multi-step investigations whose intermediate
  steps don't pollute the parent transcript.

## Non-goals

- **Async / background sub-agents (Claude Code's `Task`).** Deferred to
  v2. v1 is synchronous: parent blocks on sub-agent completion.
- **Cross-process sub-agents.** Single binary, single runtime.
- **Named agent presets** (Claude Code's `agentDefinitions`). The model
  just passes `prompt` + `tool_allowlist`; no registry of named
  sub-agent personas.
- **Inter-agent messaging.** No back-and-forth between parent and
  sub-agent mid-loop. The sub-agent gets one prompt and returns one
  answer.
- **Recursion in v1.** `AgentTool` is filtered out of every
  sub-agent's tool registry.
- **Per-call cost ceiling.** No router yet; revisit when we have one.

## `AgentTool` schema

Tool name: `"AgentTool"`. Input:

```json
{
  "type": "object",
  "properties": {
    "prompt": {
      "type": "string",
      "description": "The task description handed to the sub-agent as its first user message."
    },
    "tool_allowlist": {
      "type": ["array", "null"],
      "items": { "type": "string" },
      "description": "Names of tools the sub-agent may use. If null or omitted, the sub-agent inherits all parent tools EXCEPT AgentTool itself."
    },
    "model": {
      "type": ["string", "null"],
      "description": "Optional model id override. If null, inherits the parent's model."
    }
  },
  "required": ["prompt"]
}
```

Output: a single `ContentBlock::Text` with the sub-agent's final
assistant text.

## Sub-agent lifecycle

```
Parent turn loop
  │
  ├─ model emits ToolUseBlock { name: "AgentTool", input: { prompt, ... } }
  │
  ├─ AgentTool::call(input, cx)
  │     │
  │     ├─ factory(&input) → Agent (fresh, allowlisted tools, shared provider)
  │     │
  │     ├─ child loop: stream_until_done with first user msg = input.prompt
  │     │     │
  │     │     ├─ propagates cx.cancel to its own ToolContext
  │     │     └─ runs ≤ max_turns turns
  │     │
  │     └─ collect final assistant text; truncate to ~5000 chars
  │
  └─ ToolResultBlock { content: [Text(child_final_text)] }
```

The sub-agent's transcript is created fresh: one system message (the
parent's system prompt) + one user message (the `prompt` from input).
It's discarded after the sub-agent exits — only the final text returns
to the parent.

## Tool allowlisting

Implemented at registry-construction time. The factory has access to
the parent's `ToolRegistry` (via closure capture). For each call:

```rust
let child_tools = match input.tool_allowlist {
    Some(names) => {
        let mut r = ToolRegistry::new();
        for name in names {
            if name == "AgentTool" { continue; }            // never recursive
            if let Some(t) = parent_tools.get(&name) {
                r.register(Arc::clone(t));
            }
        }
        r
    }
    None => {
        // inherit all except AgentTool
        let mut r = ToolRegistry::new();
        for n in parent_tools.names() {
            if n == "AgentTool" { continue; }
            if let Some(t) = parent_tools.get(n) {
                r.register(Arc::clone(t));
            }
        }
        r
    }
};
```

Unknown names in `tool_allowlist` are silently dropped. The sub-agent
will discover the omission on first call; this is a feature (the
parent shouldn't have to know every tool name to dispatch a sub-agent).

## Inheriting parent provider / model

The factory captures `Arc<dyn Provider>` from the parent. Sub-agent's
`Agent` is built with:

- `provider` = parent's provider (same `Arc`).
- `model` = `input.model.unwrap_or(parent_config.model.clone())`.
- `max_tokens` = parent's `max_tokens`.
- `max_turns` = 20 (sub-agent default — not configurable via input).
- `prompt_cache` = true (Anthropic prompt cache locality benefits hold
  because the sub-agent shares the parent's connection pool).
- `hooks` = parent's hooks (so permissions, observability, debug logs
  apply to the sub-agent uniformly).

## Cancellation propagation

The `ToolContext` passed to `AgentTool::call` contains
`cx.cancel: CancellationToken`. The sub-agent's tool-loop driver
creates a child token via `cx.cancel.child_token()` and uses it for
its own `ToolContext`s. Cancelling the parent propagates:

- top-level `cx.cancel.cancel()` → fires the child token → cancels any
  in-flight tool inside the sub-agent → sub-agent loop exits with
  `Error::Cancelled` → `AgentTool::call` returns `ToolError::Cancelled`.

The sub-agent cannot cancel the parent.

## Transcript representation

Parent transcript (one turn):

```
[assistant] ToolUseBlock { name: "AgentTool", input: { prompt: "Find all uses of foo()", tool_allowlist: ["Read","Grep"] } }
[user]      ToolResultBlock { content: [Text("Found 14 callers; key sites are src/foo.rs:42, src/bar.rs:88, …")] }
```

The sub-agent's intermediate turns never enter the parent's `Vec<Message>`.
They live only in the sub-agent's transient buffer. If the operator
wants to see them, the debug log (`--debug`, ADR 0014) captures every
sub-agent stream event tagged with a `sub_agent_id`.

Truncation: if the sub-agent's final assistant text exceeds 5000 chars,
we keep the first 5000 and append `\n\n[sub-agent output truncated]`.

## Token budget

- `max_turns` = 20 (default, hard limit). On exhaustion, the tool
  result is `"[sub-agent exhausted max_turns without completing]\n\n<last assistant text>"`.
- `max_tokens` inherited from parent. No separate sub-agent ceiling.
- No `max_cost_usd` until we have a router (then it joins the input
  schema as an optional field).

## Crate location

`AgentTool` lives in `caliban-tools-builtin`. It needs to construct an
`Agent`, which means `caliban-tools-builtin` gains a dependency edge
on `caliban-agent-core::Agent` (it already depends on `Tool` /
`ToolContext` / `ToolError` from that crate, so this is a small
extension, not a layer violation).

```rust
// crates/caliban-tools-builtin/src/agent_tool.rs
pub struct AgentTool {
    factory: Arc<dyn Fn(&AgentToolInput) -> Agent + Send + Sync>,
}

impl AgentTool {
    pub fn new(factory: Arc<dyn Fn(&AgentToolInput) -> Agent + Send + Sync>) -> Self {
        Self { factory }
    }
}

#[async_trait]
impl Tool for AgentTool { /* call(...) drives the sub-agent loop */ }
```

The `caliban` binary wires the factory in `main`. The factory closes
over `Arc<dyn Provider>`, the parent's `ToolRegistry` (cloned at
factory-build time), and the parent's `Hooks`.

## Testing

Unit tests in `caliban-tools-builtin::agent_tool::tests`:

1. **Roundtrip with a mock provider.** Provider returns a stop-reason
   `EndTurn` with text "OK". Sub-agent returns "OK" to the parent.
2. **Allowlist filters tools.** Mock provider asks for `Bash`;
   sub-agent's registry only contains `Read`. Sub-agent receives an
   "unknown tool" error and gracefully stops; parent sees the textual
   tool result.
3. **`AgentTool` is never visible to a sub-agent.** Even when
   `tool_allowlist` explicitly lists `"AgentTool"`, the sub-agent's
   registry omits it.
4. **`None` allowlist inherits all-except-AgentTool.** Parent has
   `[Read, Grep, AgentTool]`; sub-agent gets `[Read, Grep]`.
5. **Cancellation propagates.** Parent cancels mid-sub-loop; sub-agent
   exits with cancelled; tool result is `ToolError::Cancelled`.
6. **`max_turns` exhaustion produces the exhaustion message.**
7. **Truncation at 5000 chars** appends the truncated footer.
8. **Model override** uses `input.model` when set.
9. **Final assistant text** is the *last* assistant message's text
   blocks concatenated — empty if the sub-agent stopped on a
   tool-use that never completed.
10. **Hook inheritance.** A permissions hook that denies `Bash` denies
    it inside the sub-agent too.

Integration: a small end-to-end test driving a stub provider through
the binary's tool registry; assert the parent transcript contains
exactly one `ToolUseBlock` + `ToolResultBlock` pair for the sub-agent
call.

## Risks

- **Silent allowlist drops.** The model passes `"Bsah"` (typo);
  sub-agent gets an empty registry; sub-agent fails. Surfaceable in
  the sub-agent's response (`"I don't have any tools"`). Acceptable —
  spelling errors should be self-correcting via the model.
- **Long sub-agent runs make the parent look frozen.** TUI shows the
  sub-agent's stream events relayed via the parent's stream, so the
  operator sees progress. If a sub-agent goes silent for >30s, that's
  the same UX as a slow tool — acceptable.
- **Provider rate limits.** The sub-agent's calls count against the
  same provider quota. No deduplication. Operator must budget for it.
- **No memory of sub-agent runs.** A second sub-agent invocation with
  the same prompt does the work again. Cache later via a session-scoped
  key if we see waste in practice.
- **Layer dep edge.** `caliban-tools-builtin` → `caliban-agent-core::Agent`
  is a tighter coupling than other builtins have. If this becomes
  architecturally awkward (e.g., another crate wants to spawn
  sub-agents too), we extract a thin `caliban-sub-agent` crate.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean; `cargo fmt --all -- --check`
  clean.
- `cargo test --workspace` passes — adds ≥ 10 tests in
  `caliban-tools-builtin::agent_tool::tests` plus 1 integration test.
- `AgentTool` re-exported from `caliban_tools_builtin` and registered
  in the `caliban` binary's tool registry by default; disabled with
  `--no-sub-agent`.
- README's tool list gains one sentence on `AgentTool`.
- ADR 0021 lands alongside this spec.
