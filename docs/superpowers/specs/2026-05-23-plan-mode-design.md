# Plan mode — Design

**Date:** 2026-05-23
**Status:** Sketch
**Target branch:** `jf/docs/roadmap-post-webfetch`
**Sub-project of:** caliban Rust agent harness
**Depends on:** `caliban-agent-core` (Tool trait, dispatcher), `caliban-tools-builtin`

## Goal

Give the agent a way to enter a read-only "planning" mode in which
tool calls that mutate the operator's environment are rejected before
execution. The model formulates and shares a plan; the operator
reviews and explicitly exits plan mode before any work begins. This
mirrors Claude Code's plan mode and is a UX-layer feature on top of
caliban's existing tool dispatcher — no ADR required.

## Non-goals

- Auto-detecting "this is a complex task → enter plan mode" — model
  and operator decide explicitly.
- Persistent plan storage across sessions or to disk.
- `VerifyPlanExecutionTool` (Claude Code's optional after-plan
  verification layer) — deferred.
- Hard provider-side filtering of available tools (we keep the full
  tool schema visible to the model and reject at dispatch time —
  simpler, and the model can reason about what it can't do).
- Per-tool argument-level read-only checks (e.g. `Bash` with a
  read-only-looking command). The allowlist is name-based in v1.

## Plan-mode lifecycle

```
                 user opens /plan      ┌─────────────┐
   ┌─ NORMAL ──┐ or model calls       │  PLAN MODE  │
   │           │ EnterPlanMode ──────▶ │             │
   │           │                       │             │
   │           ◀──── ExitPlanMode ─────│             │
   └───────────┘ or user opens /plan   │             │
                                       └─────────────┘
```

State is a per-session boolean `plan_mode: bool` held on `Session`
(in `caliban-sessions`) and mirrored on the in-memory `Agent` so the
tool dispatcher can read it without a session lookup. Default is
`false` for new sessions; persisted sessions restore the saved value
(usually `false`, since plan mode is typically a within-turn state).

## `EnterPlanMode` tool

Built-in tool in `caliban-tools-builtin`.

### Input schema

```json
{
  "type": "object",
  "properties": {
    "plan": {
      "type": "string",
      "description": "Markdown plan describing what you intend to do, in numbered steps."
    }
  },
  "required": ["plan"]
}
```

### Behavior

1. Set the session's `plan_mode = true`.
2. Echo the plan back as the tool result, prefixed with a confirmation
   header: `→ Plan mode entered. Operator must approve before tools that mutate state will run.`
3. Tool result body: the verbatim plan markdown.

The model is expected to invoke this when the user asks for a
multi-step or potentially-destructive task, before executing it. The
operator can also invoke a `/plan` slash command that injects an
`EnterPlanMode` call with the user's stated plan (or an empty
placeholder for the model to fill).

## `ExitPlanMode` tool

### Input schema

```json
{
  "type": "object",
  "properties": {
    "confirm": {
      "type": "boolean",
      "description": "Operator confirmation; rejected when false.",
      "default": true
    }
  }
}
```

### Behavior

1. If `confirm == false`, return `ToolError::InvalidInput("ExitPlanMode requires confirm=true")`.
2. Otherwise set `plan_mode = false`.
3. Tool result body: `Plan mode exited. Mutating tools are now available.`

The model is **strongly discouraged** from calling `ExitPlanMode`
itself in v1: the operator-driven path is the user pressing a key
binding in the TUI (or typing `/plan` to toggle) which injects the
call. The tool exists at the schema level for symmetry and for
future agent-initiated exit when we trust the model further.

## Tool gating rule

When `plan_mode == true`, the tool dispatcher in `caliban-agent-core`
checks the tool name against a hard-coded read-only allowlist before
invoking. Any tool not on the list is rejected with:

```
ToolError::Execution(PlanModeRejection {
    tool: "<name>",
    message: "Tool '<name>' is not available in plan mode. Use ExitPlanMode to proceed.",
})
```

The result still goes back to the model as a normal `ToolResult` so
it can adapt. No silent drops.

### v1 allowlist

| Tool                | Reason                                           |
| ------------------- | ------------------------------------------------ |
| `Read`              | File read, no side effects                       |
| `Glob`              | Filename match, no side effects                  |
| `Grep`              | Content search, no side effects                  |
| `WebFetch`          | GET-only by design (see web-fetch spec)          |
| `Skill`             | Text injection, no side effects                  |
| `EnterPlanMode`     | Idempotent state set                             |
| `ExitPlanMode`      | The only escape hatch                            |
| MCP tool `read_only` annotation | Per-server self-declaration            |

`Bash`, `Write`, `Edit`, and any MCP tool not marked `read_only` are
blocked. The MCP read-only signal is whatever the server advertises
in its tool annotations (`MCP` exposes a `readOnlyHint` field); when
unset, the safe default is "not read-only" → blocked.

### v2 hook

Add `Tool::is_read_only(&self) -> bool` to the `Tool` trait (default
`false`). Built-in tools override appropriately; the dispatcher
consults the method instead of a hard-coded name list. Deferred to
keep v1 reviewable.

## TUI indicator

The status line shows a `📋 plan` chip when `plan_mode == true`,
rendered next to the existing model/session indicators. The chip
uses a high-contrast color (yellow on the dark theme) so it's
unmistakable. The `/plan` slash command toggles the mode (open if
off, close if on) — the existing slash-command dispatcher in
`caliban/src/tui.rs` adds a new branch.

A future TUI affordance — a footer banner with "Plan submitted —
press Ctrl-X to approve, Ctrl-D to dismiss" — is deferred; v1 ships
with the chip and slash-command toggle.

## Interaction with existing tools

- **Hooks layer:** `before_tool` hooks run before the plan-mode
  check. A hook that already denies (e.g. a per-host `WebFetch`
  deny rule) still runs and still wins. Plan mode is an additional
  gate, not a replacement.
- **MCP tools:** the gating uses the tool's registered name + its
  `read_only` annotation. No special-case path for MCP.
- **System prompt:** when `plan_mode` is active, prepend one line to
  the per-turn message: `[plan mode active — only read-only tools
  are available; call ExitPlanMode (or have the user toggle /plan)
  to proceed]`. This is set on the message, not the persisted
  system prompt — turning plan mode on/off does not rewrite session
  state.
- **Sessions:** `Session.plan_mode` is serialized so a session
  resumed mid-plan stays in plan mode. The TUI shows the chip
  immediately on load.

## Testing

Unit + integration tests across `caliban-agent-core` and
`caliban-tools-builtin`:

1. `enter_plan_mode_sets_session_flag` — invoke `EnterPlanMode`; assert
   `session.plan_mode == true`.
2. `exit_plan_mode_clears_session_flag` — round-trip enter then exit.
3. `exit_plan_mode_requires_confirm` — `confirm=false` → `InvalidInput`.
4. `mutating_tool_blocked_in_plan_mode` — Bash invocation while
   `plan_mode` → `Execution` with rejection message; no `/bin/sh` spawned.
5. `read_only_tool_allowed_in_plan_mode` — Read invocation while
   `plan_mode` → success.
6. `plan_mode_chip_renders_in_status_line` — TUI snapshot test.
7. `plan_slash_command_toggles_state` — `/plan` while off → on; again → off.
8. `mcp_tool_with_read_only_hint_allowed` — wrapped MCP tool with
   `read_only_hint=true` invoked in plan mode → succeeds.
9. `mcp_tool_without_hint_blocked` — same wrapper without the hint → blocked.
10. `plan_mode_persists_across_session_save_load` — round-trip via
    `SessionStore`.

Target ~10 new tests.

## Risks

- **Allowlist gaps.** A new mutating tool added later forgets to be
  in the allowlist → silently usable in plan mode. Mitigation: the
  v2 `is_read_only()` trait method makes this a compile-time
  concern (every Tool impl declares its stance).
- **Bash with read-only-looking arguments.** `Bash("ls")` is
  effectively read-only but still blocked. Acceptable — the
  operator can exit plan mode for safe inspection, or use
  `Glob`/`Grep` instead. Argument-level inspection is intentionally
  out of scope (false-negative risk too high).
- **Model bypass via `ExitPlanMode`.** A misbehaving model could
  call `ExitPlanMode` immediately and proceed. Mitigation in v1: the
  TUI rendering of the exit makes it visible; v2 may require
  operator-side confirmation before honoring a model-initiated exit.
- **MCP `read_only_hint` is operator-trust.** A hostile MCP server
  could lie. Same trust model as installing the server in the first
  place; documented.

## Acceptance criteria

- `cargo build --workspace` clean; clippy + fmt clean.
- `cargo test --workspace` passes — adds ≥ 10 new tests across
  `caliban-agent-core` (dispatcher gating) and `caliban-tools-builtin`
  (the two tools).
- `EnterPlanMode` and `ExitPlanMode` registered in the `caliban`
  binary's tool registry.
- TUI shows the `📋 plan` chip when active; `/plan` toggles state.
- Mutating tools (Bash / Write / Edit) provably blocked in plan mode
  via integration test.
- No new ADR — design is fully captured here and reuses existing
  dispatcher and Tool-trait plumbing.
