# Hook event taxonomy expansion — Design

**Date:** 2026-05-24
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** `adrs/0024-hook-event-taxonomy.md`

## Goal

Expand caliban's `Hooks` trait from four in-process events
(`before_turn`/`after_turn`/`before_tool`/`after_tool`) to a richer
taxonomy that mirrors Claude Code's documented lifecycle, and add
support for shell-command / HTTP / MCP / prompt / agent hook handler
types loaded from a `hooks.toml` (or `hooks` section in the unified
`settings.json` once ADR 0026 lands). This is the foundation everything
downstream — plugins, observability, automation, audit logging, and
external integrations — gets to build on.

The current `Hooks` trait stays the in-process API, but it becomes one
implementation of a broader **hook router** that fans events out to
configured handlers. The existing `PermissionsHook` keeps working
unchanged — it's an in-process hook that happens to observe
`before_tool`; the new event names extend the trait surface around it.

## Non-goals

- **Plugin packages.** Bundling skills + hooks + agents + MCP + output-
  styles into a marketplace-installable package is its own design (ADR
  0030 placeholder). This spec gives plugins a way to *register* hooks
  later, but doesn't ship the package format.
- **Hook inheritance for subagents.** Already deferred (ADR 0021 PR #9
  notes); will land alongside the subagent worktree work. This spec
  fires `SubagentStart`/`SubagentStop`/`TaskCreated`/`TaskCompleted`
  from the parent agent so observability hooks already see subagent
  activity, but subagents themselves don't yet auto-inherit hook
  config.
- **Hook output rewriting beyond `updatedInput`.** Claude Code supports
  `updatedInput` in the `before_tool` decision JSON to mutate tool
  arguments before dispatch. We adopt that. We do *not* yet support
  mutating tool *output* via `after_tool` hooks; it lands in a v2.
- **Sandboxing hook handlers.** Shell command handlers run with the
  caliban process's privileges. OS-level sandbox (ADR-TBD) is its own
  initiative.
- **Hook-driven retries.** A hook returning "retry" isn't a thing yet.
  Stop/StopFailure events fire but `retry` is out of scope.

## Architecture

```
                                    config layer
                                       │
                          hooks.toml / settings.json[hooks]
                                       │
                                       ▼
                         ┌──────────────────────────────┐
                         │       HookRouter             │
                         │  Event → MatcherGroup[]      │
                         │            │                 │
                         │            ▼                 │
                         │       Handler[]              │
                         │  (cmd / http / mcp /         │
                         │    prompt / agent /          │
                         │    in-process Hooks impl)    │
                         └──────────────────────────────┘
                                       │
        ┌───────────────┬──────────────┼──────────────┬───────────────┐
        ▼               ▼              ▼              ▼               ▼
    CommandHandler  HttpHandler   McpHandler     PromptHandler   AgentHandler
   (spawn + stdin   (POST JSON   (call MCP      (LLM via         (Task tool;
    JSON; decide    body; read   tool from      router; struct   subagent;
    via stdout      JSON resp)   server)        output via       async only)
    JSON or exit                                json-schema)
    codes)
                                       │
                                       ▼
                              HookDecision result
                  (Allow / Deny(reason) / UpdatedInput(json))


  caliban-agent-core::Hooks trait (in-process)
        │
        └─ HookRouter is itself an impl Hooks → composes into
           the existing pipeline alongside PermissionsHook,
           audit logger, etc.
```

Three building blocks:

1. **Expanded event enum.** New events (`SessionStart`, `SessionEnd`,
   `UserPromptSubmit`, `PreCompact`/`PostCompact`, `ConfigChange`,
   `CwdChanged`, `FileChanged`, `SubagentStart`/`Stop`, `TaskCreated`/
   `Completed`, `PermissionRequest`/`Denied`, `Notification`) become
   methods on `Hooks` with default no-op bodies. Existing
   implementations keep working unchanged.
2. **Handler types.** A `HookHandler` enum knows how to invoke a
   `command`, `http`, `mcp`, `prompt`, or `agent` handler against the
   event payload, parse its response, and return a `HookDecision`.
3. **Router.** `HookRouter` reads `hooks.toml`, indexes handlers by
   event + matcher group, and dispatches in parallel (with a
   configurable concurrency cap). `HookRouter` *itself* implements
   `Hooks`, so wiring it into `AgentBuilder` is identical to wiring in
   any other hook stack — chained behind `PermissionsHook` in the
   composition order documented below.

### Composition order (`AgentBuilder::hooks(...)`)

```
inner_user_hooks ──► HookRouter (external hooks.toml handlers)
                  ──► PermissionsHook (rule-gated dispatch)
                  ──► AuditLogger (in-process structured log)
                  ──► NoopHooks (tail)
```

`before_*` events flow top → bottom; `after_*` events flow bottom → top
(LIFO). The **first** handler to return `Deny` short-circuits;
`UpdatedInput` is composable (later handlers see the rewritten input).

## Crate structure (deltas only)

```
crates/caliban-agent-core/
├── src/
│   ├── hooks.rs              # extend trait with new events + decision variants
│   ├── hooks_router/         # NEW module
│   │   ├── mod.rs            # HookRouter + dispatch loop
│   │   ├── config.rs         # hooks.toml parser; merge with settings.json
│   │   ├── handler.rs        # HookHandler enum + invoke()
│   │   ├── command.rs        # spawn child; stdin JSON; parse stdout
│   │   ├── http.rs           # reqwest POST; URL allowlist
│   │   ├── mcp.rs            # invoke MCP tool via caliban-mcp-client
│   │   ├── prompt.rs         # provider call via caliban-model-router
│   │   ├── agent.rs          # delegate to a subagent
│   │   └── matcher.rs        # tool-name glob + permission-rule-style filter
│   └── permissions.rs        # unchanged (composes behind HookRouter)
```

Dependencies added to `caliban-agent-core`:

```toml
caliban-mcp-client    = { path = "../caliban-mcp-client", optional = true }
caliban-model-router  = { path = "../caliban-model-router", optional = true }
reqwest               = { workspace = true, optional = true }
which                 = "6"           # validate `command` is on PATH at parse time
jsonschema            = { workspace = true, optional = true }
```

Optional features `hook-mcp`, `hook-http`, `hook-prompt`, `hook-agent`
gate the cross-crate handlers so a minimal CI build still uses only
`command`-typed hooks.

## Config schema

Either `~/.config/caliban/hooks.toml` *or* a top-level `hooks` table in
the unified `settings.json` (ADR 0026). Both forms resolve to the same
in-memory `HooksConfig`. The schema mirrors Claude Code's event →
matcher group → handler array nesting.

```toml
# ~/.config/caliban/hooks.toml — user-level

# Kill switch
disable_all_hooks = false

# Allow only managed hooks (org policy escape hatch)
allow_managed_hooks_only = false

# HTTP handler safety
allowed_http_hook_urls = [
  "https://hooks.example.com/*",
  "https://my-org.tail-scale.ts.net:9000/*",
]
http_hook_allowed_env_vars = ["AUDIT_TOKEN"]

# Per-event configuration
[[hooks.SessionStart]]
matcher = "*"
[[hooks.SessionStart.handlers]]
type    = "command"
command = "/usr/local/bin/audit"
args    = ["session-start"]
timeout = "5s"

[[hooks.PreToolUse]]
matcher = "Bash"
if      = "Bash:rm *"          # permission-rule-style filter
[[hooks.PreToolUse.handlers]]
type    = "command"
command = "${CALIBAN_PROJECT_DIR}/.caliban/hooks/guard-rm.sh"
async   = false

[[hooks.PreToolUse]]
matcher = "WebFetch"
[[hooks.PreToolUse.handlers]]
type    = "http"
url     = "https://hooks.example.com/preflight"
headers = { Authorization = "Bearer ${AUDIT_TOKEN}" }
timeout = "3s"

[[hooks.PostToolUse]]
matcher = "*"
[[hooks.PostToolUse.handlers]]
type  = "mcp"
mcp   = "audit-server"          # server name from mcp.toml
tool  = "log_tool_call"
async = true                    # fire-and-forget; do not block dispatch

[[hooks.UserPromptSubmit]]
matcher = "*"
[[hooks.UserPromptSubmit.handlers]]
type   = "prompt"
prompt = "Classify the user's request: safe|sensitive|off-topic. Reply JSON only."
model  = "fast"                 # routes via caliban-model-router (FastClassifier purpose)
schema = '{ "type": "object", "properties": { "label": { "enum": ["safe","sensitive","off-topic"] } } }'

[[hooks.FileChanged]]
matcher = "*.rs"
[[hooks.FileChanged.handlers]]
type  = "agent"
agent = "code-review"           # caliban subagent name
async = true
```

### Field semantics

| Field                    | Type                                    | Required        | Default      | Notes |
|--------------------------|-----------------------------------------|-----------------|--------------|-------|
| `disable_all_hooks`      | bool                                    | no              | `false`      | Top-level kill switch; bypasses **all** external handlers (in-process `Hooks` impls still run). |
| `allow_managed_hooks_only` | bool                                  | no              | `false`      | When true, only handlers declared in the *managed* settings scope (ADR 0026) run. |
| `allowed_http_hook_urls` | array of URL globs                      | no              | `[]` (deny all) | HTTP handlers refuse any URL not matched. |
| `http_hook_allowed_env_vars` | array of env-var names               | no              | `[]`         | Allowlist of env vars HTTP handlers may use in `${VAR}` expansion. |
| `hooks.<Event>`          | array of matcher groups                 | no              | `[]`         | Event keyed by exact PascalCase event name (see list below). |
| `matcher`                | tool-name glob *or* `"*"`              | yes (tool events) | `"*"`      | For non-tool events, matcher is ignored (still required as `"*"`). |
| `if`                     | permission-rule-style pattern (e.g. `Bash:rm *`) | no    | none         | Extra filter; same syntax as `permissions.toml::tool`. Both `matcher` and `if` must match. |
| `handlers[].type`        | `"command"`/`"http"`/`"mcp"`/`"prompt"`/`"agent"` | yes   | —            | Handler dispatch. |
| `handlers[].timeout`     | humantime duration                      | no              | `"30s"`      | Hard deadline; on expiry hook is treated as `Allow` with a warning. Async hooks ignore this. |
| `handlers[].async`       | bool                                    | no              | `false`      | Async-true handlers run on a background task pool; their decision is ignored (can only observe). Useful for audit / metrics. |
| `handlers[].command`     | string                                  | yes (cmd)       | —            | Absolute path or PATH-resolvable binary. Validated with `which` at parse time. |
| `handlers[].args`        | array of strings                        | no              | `[]`         | Extra argv after `command`. `${VAR}` expansion. |
| `handlers[].env`         | table str→str                           | no              | `{}`         | Extra env to pass; only allowlisted keys honored. |
| `handlers[].url`         | string                                  | yes (http)      | —            | Must match `allowed_http_hook_urls`. |
| `handlers[].headers`     | table str→str                           | no              | `{}`         | Static request headers; `${VAR}` expanded against `http_hook_allowed_env_vars`. |
| `handlers[].mcp`/`.tool` | string                                  | yes (mcp)       | —            | Reference to an `mcp.toml` server + tool. |
| `handlers[].agent`       | string                                  | yes (agent)     | —            | Name of a configured subagent. Async-only. |
| `handlers[].prompt`      | string                                  | yes (prompt)    | —            | LLM prompt text; payload event JSON appended as a user message. |
| `handlers[].model`       | `"fast"`/`"main"`/`"summarization"`/concrete name | no   | `"fast"`     | Maps to `RequestPurpose` for the model router. |
| `handlers[].schema`      | string (JSON schema)                    | no              | none         | When set, prompt handler returns `structured_output` and that's the hook payload. |

### Event names

Mirror Claude Code's PascalCase taxonomy. Caliban v1 covers:

- **Session lifecycle:** `SessionStart`, `SessionEnd`, `Notification`
- **Per-turn lifecycle:** `UserPromptSubmit`, `Stop`, `StopFailure`
- **Tool lifecycle:** `PreToolUse`, `PostToolUse`, `PostToolUseFailure`,
  `PermissionRequest`, `PermissionDenied`
- **Compaction:** `PreCompact`, `PostCompact`
- **Filesystem/env:** `ConfigChange`, `CwdChanged`, `FileChanged`
- **Subagents:** `SubagentStart`, `SubagentStop`, `TaskCreated`,
  `TaskCompleted`

Not yet implemented (placeholders accepted in the parser but with a
`debug!` log on use): `Setup`, `UserPromptExpansion`,
`PostToolBatch`, `InstructionsLoaded`, `WorktreeCreate`,
`WorktreeRemove`, `Elicitation`, `ElicitationResult`, `TeammateIdle`.

## Decision protocol (shell-command handlers)

Caliban writes the event JSON to the handler's stdin and reads stdout
(blocking up to `timeout`). The handler signals a decision **either**:

### A) JSON on stdout

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "deny",
    "permissionDecisionReason": "blocked by site policy",
    "updatedInput": { "command": "echo redacted" }
  }
}
```

Recognized fields:

- `permissionDecision`: `"allow" | "deny" | "ask"`; `"allow"` is the
  default if absent.
- `permissionDecisionReason`: string surfaced to the user on deny/ask.
- `updatedInput`: when present, replaces the tool's input JSON. Must
  validate against the tool's `input_schema()`; if validation fails,
  the dispatch is denied with an error.

### B) Exit codes

When stdout is empty or unparseable JSON:

| Exit code | Decision                          |
|-----------|-----------------------------------|
| 0         | Allow                             |
| 2         | Deny (stderr surfaced as reason)  |
| any other | Allow + log warning (treats failure as non-blocking; mirrors Claude Code) |

A handler may print non-JSON to stderr unconditionally; it's captured
verbatim into the `tracing` span under `hook.stderr` (truncated at
8 KiB).

### Stdin payload shape

Common envelope:

```json
{
  "hookEventName": "PreToolUse",
  "sessionId": "01HW...",
  "cwd": "/home/me/proj",
  "turn_index": 7,
  "tool": {
    "name": "Bash",
    "useId": "toolu_01ABC",
    "input": { "command": "rm -rf /tmp/x" }
  }
}
```

Event-specific fields (snake_case for parity with our existing JSON,
*not* camelCase — diverging from Claude Code here, documented in the
ADR):

- `UserPromptSubmit`: `prompt`, `attachments[]`
- `PreCompact`/`PostCompact`: `token_count_before`, `token_count_after`
- `FileChanged`: `path`, `kind` (`created`/`modified`/`deleted`)
- `PermissionRequest`: `tool.name`, `tool.input`, `rule.action`,
  `rule.comment`
- `SubagentStart`/`Stop`: `agent_name`, `task_id`
- `ConfigChange`: `changed_keys[]`, `new_settings_summary{}`

## HTTP handlers

`POST <url>` with the same envelope body. Response is parsed identically
to a command handler's stdout JSON. Non-2xx HTTP status → Allow + log
warning (matches command-handler "any-other-exit-code" semantics).

URL must match at least one glob in `allowed_http_hook_urls`. The list
defaults to **empty** (deny-all) — operators must opt in explicitly.
Headers and URL `${VAR}` expansion is gated by
`http_hook_allowed_env_vars`.

## MCP handlers

Invoke an MCP server's tool with the event envelope as the tool input.
The server's response content is parsed for the same
`hookSpecificOutput` shape. Async MCP hooks are useful for audit servers
that just log; sync MCP hooks for policy servers that gate.

## Prompt handlers

Call the LLM via `caliban-model-router` with the configured
`RequestPurpose` (default `FastClassifier`). The prompt text is the
system message; the event envelope JSON is the user message. When a
`schema` is set, the router uses provider-native structured output
(`json_schema` on Anthropic, etc.); the parsed object is the hook
result. Prompt handlers always have an implicit 15s `timeout`.

## Agent handlers

Delegate to a configured subagent via the existing `AgentTool`. The
event envelope is the subagent's initial prompt. Agent handlers are
**async-only** — synchronous subagent calls during a hook would risk
recursion and turn-budget blowup. The agent's final response is
captured and surfaced under `tracing` but does not influence the
parent's decision.

## Matcher groups & filters

`matcher` is a glob over the tool name (`"Bash"`, `"mcp__linear__*"`,
`"*"`). For non-tool events, only `"*"` is meaningful (set as the
sentinel).

`if` is the same syntax as `permissions.toml::tool` — `Tool` or
`Tool:first-arg-glob`. Combines AND with `matcher` (both must match).
Reuses `caliban_agent_core::permissions::matches_glob` and
`first_arg`.

A handler runs when:

```
event matches               (Event registered)
AND matcher matches         (tool name glob OR non-tool event)
AND if matches              (or no `if` set)
AND !disable_all_hooks
AND scope is allowed        (managed-only mode honors `allow_managed_hooks_only`)
```

## Async handlers + parallel dispatch caveat

Async handlers (`async = true`) detach onto a background `tokio` task
pool with bounded concurrency (default 16, configurable via
`hooks.maxConcurrentAsync`). Their `HookDecision` is ignored.

For tool events under **parallel tool dispatch** (ADR 0016), sync
handlers serialize per-tool-call (one handler chain per `tool_use_id`)
but multiple `tool_use_id`s run concurrently. The ordering caveat
already documented on the `Hooks` trait still applies:
`PostToolUse` fires in *completion* order, not in assistant-message
order. Hook authors who need ordering must correlate via `tool_use_id`.

## Kill switch + managed-only mode

- **`disable_all_hooks = true`**: external handlers (command/http/mcp/
  prompt/agent) are skipped entirely. In-process `Hooks` impls
  (PermissionsHook, AuditLogger) still run — they're not "hooks" in the
  external sense. This is the documented emergency escape hatch.
- **`allow_managed_hooks_only = true`**: only handlers loaded from the
  *managed* settings scope (per ADR 0026) run. Mirrors Claude Code's
  org-policy mode.

Both flags are visible in `/hooks` overlay.

## `/hooks` slash command

The TUI slash command lists configured hooks and their state:

```
┌─ Hooks ──────────────────────────────────────────────────────────────┐
│ SessionStart   * → command audit (5s)                       managed  │
│ PreToolUse     Bash:rm * → command guard-rm.sh (3s)         project  │
│ PreToolUse     WebFetch  → http hooks.example.com/preflight user     │
│ PostToolUse    *         → mcp audit-server:log_tool_call async      │
│ UserPromptSubmit *       → prompt fast-classifier (15s)     project  │
│ FileChanged    *.rs      → agent code-review                async    │
└──────────────────────────────────────────────────────────────────────┘
[esc] close   [↑/↓] navigate   [enter] focus   [t] toggle  [r] reload
```

Keys: `t` toggles a handler (writes `enabled = false` to the scope's
config), `r` re-parses `hooks.toml` (live reload, see ADR 0026), `enter`
opens a detail pane showing the parsed config and last-3 execution
results (success / decision / timing).

## Public API sketches

```rust
// crates/caliban-agent-core/src/hooks.rs (additions)

#[async_trait]
pub trait Hooks: Send + Sync {
    // existing 4 events …

    async fn session_start(&self, _ctx: &SessionCtx<'_>) -> Result<()> { Ok(()) }
    async fn session_end  (&self, _ctx: &SessionCtx<'_>) -> Result<()> { Ok(()) }
    async fn user_prompt_submit(&self, _ctx: &PromptCtx<'_>) -> Result<HookDecision> { Ok(HookDecision::Allow) }
    async fn pre_compact (&self, _ctx: &CompactCtx<'_>) -> Result<()> { Ok(()) }
    async fn post_compact(&self, _ctx: &CompactCtx<'_>) -> Result<()> { Ok(()) }
    async fn config_change(&self, _ctx: &ConfigChangeCtx<'_>) -> Result<()> { Ok(()) }
    async fn cwd_changed  (&self, _ctx: &CwdChangedCtx<'_>) -> Result<()> { Ok(()) }
    async fn file_changed (&self, _ctx: &FileChangedCtx<'_>) -> Result<()> { Ok(()) }
    async fn subagent_start(&self, _ctx: &SubagentCtx<'_>) -> Result<()> { Ok(()) }
    async fn subagent_stop (&self, _ctx: &SubagentCtx<'_>) -> Result<()> { Ok(()) }
    async fn task_created (&self, _ctx: &TaskCtx<'_>) -> Result<()> { Ok(()) }
    async fn task_completed(&self, _ctx: &TaskCtx<'_>) -> Result<()> { Ok(()) }
    async fn permission_request(&self, _ctx: &PermissionCtx<'_>) -> Result<()> { Ok(()) }
    async fn permission_denied (&self, _ctx: &PermissionCtx<'_>) -> Result<()> { Ok(()) }
    async fn notification (&self, _ctx: &NotificationCtx<'_>) -> Result<()> { Ok(()) }
}

#[derive(Debug, Clone)]
pub enum HookDecision {
    Allow,
    Deny(String),
    UpdatedInput(serde_json::Value),   // NEW
}
```

```rust
// crates/caliban-agent-core/src/hooks_router/mod.rs

pub struct HookRouter {
    config: HooksConfig,
    managed_scopes: Vec<Scope>,
    mcp:   Option<Arc<caliban_mcp_client::McpClientManager>>,
    model: Option<Arc<caliban_model_router::ModelRouter>>,
    http:  reqwest::Client,
    inner: Arc<dyn Hooks>,
    sem:   Arc<tokio::sync::Semaphore>,  // async-handler concurrency cap
}

impl HookRouter {
    pub fn from_settings(settings: &Settings, inner: Arc<dyn Hooks>) -> Result<Self> { … }
    pub fn reload(&self, settings: &Settings) -> Result<()> { … }
}

#[async_trait]
impl Hooks for HookRouter { /* fans every event out to handlers + inner */ }
```

## Testing strategy

~20 enumerated tests landed alongside the implementation:

1. **Trait default no-ops.** New events on `NoopHooks` compile + return Ok.
2. **Command handler — exit 0 = Allow.** Spawn `true`; verify Allow.
3. **Command handler — exit 2 = Deny w/ stderr.** Spawn a script printing "blocked" to stderr; assert reason surfaces.
4. **Command handler — stdout JSON Deny.** Script writes the documented JSON; assert decision + reason.
5. **Command handler — stdout JSON UpdatedInput.** Script rewrites a `Bash.command`; assert tool is dispatched with new input.
6. **Command handler — UpdatedInput fails schema validation.** Script returns garbage; dispatch denied.
7. **Command handler — timeout = Allow + warning.** Script `sleep 5` with 100ms timeout.
8. **HTTP handler — `wiremock` 200 with deny JSON.** Assert decision.
9. **HTTP handler — URL not allowlisted = skipped + warning.**
10. **HTTP handler — `${VAR}` not allowlisted = expansion fails.**
11. **MCP handler — calls server tool, parses response.** Uses the same in-tree test server as MCP v2.
12. **Prompt handler — structured output → Deny.** Mocks router to return JSON matching the schema.
13. **Agent handler is async-only.** Loader rejects `async = false` with a parse error.
14. **Matcher glob filters correctly.** `Bash` event with `matcher = "WebFetch"` → handler skipped.
15. **`if` filter combines AND.** Matcher `Bash`, `if = "Bash:rm *"` — only fires on `rm`.
16. **Parallel tool dispatch ordering.** Two concurrent tool calls; assert hooks fire per-tool but ordering across calls is by completion.
17. **`disable_all_hooks` blocks all external handlers but lets PermissionsHook run.**
18. **`allow_managed_hooks_only` blocks user/project scopes.**
19. **`/hooks` overlay rendering.** Snapshot test of the layout.
20. **Live reload.** Mutate `hooks.toml`, call `HookRouter::reload`, verify new handlers fire on next event.

Plus 4 cross-crate integration tests under `crates/caliban-agent-core/tests/`:
session-lifecycle event round-trip, compaction event round-trip,
file-watcher → `FileChanged` round-trip, subagent lifecycle event
round-trip.

## Risks

- **Shell hooks are arbitrary code.** A malicious `hooks.toml` can pwn
  the host. Mitigation: documented loudly in the README; managed-only
  mode for org policy; future OS sandbox.
- **Timeouts hide failures.** A slow audit server silently becomes
  Allow on timeout. Mitigation: every timeout logs WARN with a
  rate-limited summary in the TUI status line; `/hooks` overlay flags
  handlers that have timed out in the last N runs.
- **Hook explosion under parallel dispatch.** Eight concurrent tool
  calls × five handlers per event = 40 spawns. Mitigation: per-event
  concurrency cap (default 16 async, 32 sync), backed by a
  `tokio::sync::Semaphore`.
- **`UpdatedInput` mutation is hard to debug.** Tool args may not
  match what the model produced. Mitigation: `tracing` span records
  both the original and rewritten input under `hook.updated_input`;
  the transcript shows a small `↻` glyph next to rewritten tool calls.
- **Naming drift from Claude Code.** Our stdin payload uses snake_case
  while CC uses camelCase. Mitigation: `hookEventName` and
  `hookSpecificOutput` keep camelCase to match the decision-protocol
  JSON; everything else is snake_case for parity with our existing
  internal JSON. Documented in the ADR.
- **Subagent hook inheritance is pending.** Until ADR 0021 PR #9 lands,
  subagents fire `SubagentStart`/`Stop`/`TaskCreated`/`Completed` events
  on the *parent's* router but don't run hooks themselves. Acceptable
  for v1; called out in `/hooks` overlay copy.
- **Hook reload races.** Live reload while an in-flight hook is
  running. Mitigation: `HookRouter::reload` swaps the config behind an
  `ArcSwap`; in-flight handlers complete against their original config.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean; `cargo fmt --all -- --check`
  clean.
- At least 20 new tests under `caliban-agent-core` plus 4 cross-crate
  integration tests, all passing.
- `Hooks` trait has all events listed in §"Event names" implemented at
  least as default no-op stubs; the four pre-existing events keep
  their signatures.
- `HookDecision::UpdatedInput(...)` is honored by the dispatcher and
  surfaces in `tracing` under `hook.updated_input`.
- A working `hooks.toml` example in `docs/examples/hooks/` covers all
  five handler types with a one-paragraph README.
- All rows under **B. Hooks & extensibility** in
  `docs/parity-gap-matrix.md` move 🔴 → ✅ except the **Plugin
  packages** row (separate ADR 0030 follow-up).
- `/hooks` slash command lists configured handlers and toggles state.
- ADR 0024 lands in `accepted` status alongside this implementation.
