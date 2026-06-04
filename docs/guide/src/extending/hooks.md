# Hooks

Hooks let you attach external logic to caliban's event stream — shell scripts, HTTP callbacks, or MCP tools — without modifying the agent or recompiling. Hooks run in-process (for the built-in `PermissionsHook` and audit hooks) or via an external `HookRouter` (for operator-configured handlers).

## Event taxonomy

Caliban fires events at the following lifecycle points (ADR 0024):

| Event | When it fires |
|---|---|
| `SessionStart` | Once at startup, before the first turn |
| `SessionEnd` | On clean exit |
| `UserPromptSubmit` | Before each user message is sent (including slash commands; payload includes `is_slash`) |
| `PreCompact` | Before context compaction begins |
| `PostCompact` | After compaction completes |
| `PreToolUse` | Before each tool call; can gate or rewrite the call |
| `PostToolUse` | After each tool call completes |
| `PostToolUseFailure` | When a tool call errors |
| `ConfigChange` | When a settings file changes on disk (live reload) |
| `CwdChanged` | When the working directory changes |
| `FileChanged` | When a file the agent edited is detected to have changed |
| `SubagentStart` / `SubagentStop` | When a sub-agent is spawned or exits |
| `TaskCreated` / `TaskCompleted` | When a sub-agent task is enqueued or finishes |
| `PermissionRequest` | When the agent requests permission for a tool call |
| `PermissionDenied` | When a tool call is denied |
| `Notification` | General notification events |
| `Stop` / `StopFailure` | When the agent loop stops (cleanly or with error) |

Additional events (`Setup`, `UserPromptExpansion`, `PostToolBatch`, `InstructionsLoaded`, `WorktreeCreate`, `WorktreeRemove`, `Elicitation`, `ElicitationResult`, `TeammateIdle`) are reserved but not yet fired.

## Handler types

Each hook entry declares one or more handlers. Two handler types are fully wired; three are stubs (see below).

| Type | Status | Description |
|---|---|---|
| `command` | Fully wired | Spawn a child process; stdin is event JSON; decision via stdout or exit code |
| `http` | Fully wired | POST event JSON to a URL; decision via response JSON |
| `mcp` | Experimental stub | Invoke an MCP server tool with the event JSON |
| `prompt` | Experimental stub | Call the model router with a classifier prompt |
| `agent` | Experimental stub | Delegate to a sub-agent (async only) |

```admonish warning title="mcp / prompt / agent handlers are stubs"
The `mcp`, `prompt`, and `agent` handler types are defined in the config schema and appear in `/hooks` output, but their dispatch logic is not yet wired. They will be activated as their upstream dependencies (ADR 0023 MCP wiring, ADR 0037 sub-agent fleet) land. Until then, any handler of these types is silently skipped at dispatch time.
```

## Decision protocol

For `PreToolUse` and `UserPromptSubmit`, `command` and `http` handlers report their decision as:

**Stdout JSON** (preferred):
```json
{
  "hookSpecificOutput": {
    "permissionDecision": "allow",
    "permissionDecisionReason": "matched allowlist",
    "updatedInput": {}
  }
}
```

`permissionDecision` values: `allow`, `deny`, `ask`. `updatedInput` lets the hook rewrite the tool input before dispatch (the rewritten input is validated against the tool's schema; validation failure is a hard deny).

**Exit codes** (shell-script shorthand):
- `0` — Allow
- `2` — Deny (stderr becomes the reason)
- anything else — Allow with a logged warning

`PostToolUse` and observer-only hooks ignore the decision even when a handler provides one. Handlers marked `async = true` are fire-and-forget; their decisions are always ignored.

## Config: settings `hooks` table (preferred)

Hooks live in the unified settings file under the `hooks` key. The table maps event names to arrays of handler groups. See [Settings Layering](../configuration/settings-layering.md) for how scopes merge — hook arrays concatenate across scopes (project entries append to user entries).

```toml
# .caliban/settings.toml  — project scope

disable_all_hooks = false
allow_managed_hooks_only = false

allowed_http_hook_urls = [
  "https://hooks.example.com/*",
]
http_hook_allowed_env_vars = ["AUDIT_TOKEN"]

[[hooks.SessionStart]]
matcher = "*"
[[hooks.SessionStart.handlers]]
type    = "command"
command = "/usr/local/bin/caliban-audit"
args    = ["session-start"]
timeout = "5s"

[[hooks.PreToolUse]]
matcher = "Bash"
if      = "Bash:rm *"
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
mcp   = "audit-server"
tool  = "log_tool_call"
async = true
```

## Config: legacy `hooks.toml` (compat)

If no `hooks` key appears in any settings file, caliban falls back to loading:

- `<workspace>/.caliban/hooks.toml` (project scope)
- `~/.config/caliban/hooks.toml` (user scope)

The legacy file uses the same TOML shape shown above (top-level keys plus `[[hooks.<Event>]]` arrays). The two scopes merge with project entries taking priority. This path is deprecated — prefer the unified settings file for new configurations.

## Safety controls

| Setting / flag | Effect |
|---|---|
| `disable_all_hooks = true` | Bypasses all external handlers; in-process hooks (permissions, audit) still run |
| `allow_managed_hooks_only = true` | Only handlers from the managed settings scope fire |
| `allowed_http_hook_urls` | URL glob allowlist; HTTP handlers fail closed if the URL isn't listed |
| `http_hook_allowed_env_vars` | Env vars that may be expanded in HTTP handler headers |
| `--no-hooks` | One-off CLI override; mirrors `disable_all_hooks` for a single run |
| `CALIBAN_NO_HOOKS=1` | Same, via environment variable |

```admonish tip title="Audit without gating"
Mark your audit hooks `async = true`. Async handlers observe the event but their decision is discarded, so they can never accidentally block a tool call. They run on a bounded task pool (default 16 concurrent) so they don't pile up under heavy load.
```

## Related pages

- [Permissions concepts](../permissions/concepts.md)
- [Plugins](./plugins.md) — plugins can bundle hook configurations
- [Slash Command Index](../reference/slash-index.md) — `/hooks` shows the active handler set
