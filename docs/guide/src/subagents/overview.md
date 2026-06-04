# Sub-agents

Caliban can spawn a nested agent — a *sub-agent* — to handle a focused
subtask without polluting the parent's transcript. The parent's turn loop
pauses while the sub-agent runs, then resumes with the sub-agent's
condensed result as a single tool-result block.

## The AgentTool

Sub-agents are exposed to the model as a built-in tool named `AgentTool`.
When the model invokes it, caliban spins up a fresh `Agent` instance in the
same process and drives it to completion.

Key properties of an `AgentTool` invocation:

| Property | Value |
|---|---|
| **Process boundary** | None — in-process, same tokio runtime |
| **Max turns** | 20 (hard limit) |
| **Output returned to parent** | Final assistant text, truncated to 5 000 chars |
| **Intermediate turns** | Not recorded in the parent session; visible in debug logs |
| **Cancellation** | Inherits the parent's cancellation token |
| **Provider / model** | Inherits parent's provider; `model` input overrides the model |
| **Hooks** | Inherited by default; opt out with `inherit_hooks: false` |

### Tool allowlist

The `tool_allowlist` input controls which tools the sub-agent may call:

- **Omitted or `null`** — inherits every tool the parent has, *except*
  `AgentTool` itself.
- **Explicit list** — sub-agent gets exactly those tools. Unknown names are
  silently dropped.

```admonish note title="No recursion"
AgentTool is always stripped from the sub-agent's registry. Sub-agents
cannot spawn further sub-agents. Nested fan-out is planned for a future
release.
```

### Isolation mode

Each `AgentTool` invocation carries an `isolation` field (`none` or
`worktree`):

- **`none`** (default) — sub-agent shares the parent's working directory.
  Suitable for read-only work (investigation, summarization).
- **`worktree`** — sub-agent runs in a dedicated git worktree materialized
  at `.caliban/worktrees/<name>`. Suitable for tasks that write files.
  See [Worktree Isolation](worktrees.md) for details.

### Background mode

Setting `background: true` in the `AgentTool` input detaches the sub-agent
from the parent and hands it off to the `caliband` supervisor daemon. The
parent's call returns immediately with the new agent's id. See
[The Background Fleet](background-fleet.md).

```admonish warning title="Hook inheritance and background mode"
Closure-based hooks cannot cross the process boundary. When
`background: true` is set and the parent has closure hooks installed,
caliban drops those hooks with a warning and continues. Only
config-expressible hooks survive the handoff. Pass `inherit_hooks: false`
to suppress the warning if you know the sub-agent does not need the parent's
hooks.
```

## The `--no-sub-agent` flag

Pass `--no-sub-agent` (or set `CALIBAN_NO_SUB_AGENT=1`) to remove
`AgentTool` from the tool registry entirely. The model will never see the
tool and cannot spawn sub-agents.

```bash
caliban --no-sub-agent "review this codebase"
```

This is useful when you want a strict single-agent session, or when
operating in an environment where spawning child work is undesirable (CI
cost budgets, audit requirements).

## When to use sub-agents

| Use case | Recommended approach |
|---|---|
| Read-only research (grep, read, glob) without context bloat | `AgentTool` with `tool_allowlist: ["Read","Grep","Glob"]` |
| File-writing subtask that must not mix diffs | `AgentTool` with `isolation: worktree` |
| Long-running task that should survive the parent session | `AgentTool` with `background: true`, or `--bg <task>` |
| Strict single-agent run | `--no-sub-agent` |

For the full set of built-in tools the sub-agent can draw on, see
[Built-in Tools](../tools/builtin.md).
