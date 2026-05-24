# ADR 0024 · Hook event taxonomy + external handler types

- **Status:** proposed
- **Date:** 2026-05-24
- **Spec:** `docs/superpowers/specs/2026-05-24-hooks-expansion-design.md`

## Context

caliban's `Hooks` trait today exposes four events
(`before_turn`/`after_turn`/`before_tool`/`after_tool`) and is only
addressable from in-process Rust code: there's no way to drop a shell
script into `~/.config/caliban/` and have it run on `SessionStart`, no
HTTP callback for audit servers, no MCP-tool-as-policy-gate, no
LLM-classifier for `UserPromptSubmit`. Claude Code's documented hook
surface covers ~25 event names and five handler types; closing that
gap is Tier-1 foundation work because plugins, observability, and
automation all build on it. The full spec is in
`docs/superpowers/specs/2026-05-24-hooks-expansion-design.md`; this
ADR records the architectural commitments only.

## Decision

### Event names mirror Claude Code's PascalCase taxonomy

Add 15+ event methods to the `Hooks` trait, all with default no-op
implementations so existing `Hooks` impls keep compiling unchanged.
First-class events: `SessionStart`, `SessionEnd`, `UserPromptSubmit`,
`PreCompact`, `PostCompact`, `ConfigChange`, `CwdChanged`,
`FileChanged`, `SubagentStart`, `SubagentStop`, `TaskCreated`,
`TaskCompleted`, `PermissionRequest`, `PermissionDenied`,
`Notification`, `Stop`, `StopFailure`, `PostToolUseFailure`. Reserved
but not-yet-fired in v1: `Setup`, `UserPromptExpansion`,
`PostToolBatch`, `InstructionsLoaded`, `WorktreeCreate`,
`WorktreeRemove`, `Elicitation`, `ElicitationResult`, `TeammateIdle`.

### Five external handler types — `command`/`http`/`mcp`/`prompt`/`agent`

A new `HookRouter` consumes `hooks.toml` (or the `hooks` table inside
the unified `settings.json` once ADR 0026 lands) and dispatches events
to externally-configured handlers. The router itself implements
`Hooks`, so it composes into `AgentBuilder` like any other in-process
hook stack — behind `PermissionsHook` in the chain.

- **command:** spawn a child; stdin is event JSON; stdout JSON (or
  exit code) determines the decision.
- **http:** `POST` event JSON; response JSON is the decision.
- **mcp:** invoke a configured MCP server's tool with the event JSON.
- **prompt:** call the model router (default `FastClassifier` purpose)
  with the prompt + event JSON; `schema` enables structured-output.
- **agent:** delegate to a subagent (async-only).

### Decision protocol — `stdout JSON` *or* exit codes

Shell-command handlers signal their decision via stdout JSON
(`hookSpecificOutput.permissionDecision` ∈ `allow|deny|ask`,
`permissionDecisionReason`, optional `updatedInput`) **or** via exit
codes (0 = Allow, 2 = Deny with stderr as reason, anything else =
Allow + warning). HTTP and MCP handlers use the same response shape.

We extend `HookDecision` with `UpdatedInput(Value)` so hooks can
rewrite a tool's input before dispatch. The rewritten input is
validated against the tool's `input_schema()`; validation failure is
a hard deny.

### Stdin payload uses snake_case + camelCase mix, deliberately

The envelope's hook-protocol fields (`hookEventName`,
`hookSpecificOutput`) match Claude Code so existing CC hook scripts
work with a one-line wrapper. Caliban-specific fields
(`session_id`, `tool.useId`, `turn_index`) keep snake_case for
parity with our internal JSON. The diff is documented in the README.

### URL allowlist for HTTP hooks; env-var allowlist for `${VAR}` expansion

HTTP handlers fail closed: the operator must list each allowed URL
glob in `allowed_http_hook_urls` (default empty). Headers and URL
`${VAR}` expansion is gated by `http_hook_allowed_env_vars`. This
prevents a project-scope `hooks.toml` from exfiltrating user-scope
secrets via an attacker-controlled callback URL.

### Async handlers detach onto a bounded task pool; their decisions are ignored

`async = true` handlers are fire-and-forget: useful for audit, metrics,
and code-review subagents that observe but don't gate. A
`Semaphore`-bounded pool (default 16) caps the parallel async-handler
count. Agent-type handlers are async-only by definition (synchronous
subagent calls from a hook would risk turn-budget blowup and
recursion).

### Parallel tool dispatch ordering caveat is preserved

Under parallel tool dispatch (ADR 0016), `PostToolUse` fires in
*completion* order, not assistant-message order. We document this on
the trait and surface `tool_use_id` in `ToolCtx` so hook authors can
correlate. The router serializes hook handlers per-tool-call but lets
distinct `tool_use_id`s run concurrently.

### Kill switch and managed-only mode are first-class

`disable_all_hooks = true` blocks all external handlers but leaves
in-process `Hooks` impls running (`PermissionsHook`, audit, anything
the binary wires up). `allow_managed_hooks_only = true` further
restricts execution to handlers loaded from the managed settings
scope (ADR 0026). Both flags are visible in the `/hooks` overlay.

## Consequences

- **Positive:** Closes nine 🔴 rows under "B. Hooks & extensibility"
  in `docs/parity-gap-matrix.md` in one PR (only "Plugin packages"
  and "Hook inheritance for subagents" remain — both gated on other
  initiatives). Establishes the substrate plugins and observability
  build on. Shell-command hooks let operators glue caliban into
  existing audit / CI / policy stacks without touching Rust.
- **Negative:** Hook handlers run with caliban's privileges; shell
  hooks are arbitrary code execution by design. Until an OS sandbox
  lands, a hostile project-scope `hooks.toml` is a real risk —
  mitigated by the URL/env allowlists and managed-only mode, but
  fundamentally a "trust your repos" model. The `Hooks` trait grows
  from 4 to ~18 methods; default no-ops keep call-sites compatible
  but the trait's IDE-completion surface bloats.
- **Revisit if:** Plugin system (ADR 0030) lands and needs richer
  package-level hook registration. If hook latency becomes a
  bottleneck under heavy parallel dispatch, promote sync-handler
  invocation off the dispatcher's hot path. If `UpdatedInput` proves
  too error-prone, narrow it to specific tools or remove it. If
  Claude Code stabilizes additional event names (Elicitation /
  Setup / etc.) we promote them from reserved-but-stubbed to
  actually-fired.
