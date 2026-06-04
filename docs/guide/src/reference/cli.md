# CLI Reference

`caliban` is the main binary. Run it with no arguments to enter the interactive TUI; supply a prompt or flags to drive it headlessly or invoke a subcommand.

```bash
caliban [FLAGS/OPTIONS] [PROMPT]
caliban [FLAGS/OPTIONS] <SUBCOMMAND>
```

---

## Prompts

| Flag | Default | Description |
|------|---------|-------------|
| `PROMPT` (positional) | — | User prompt text. Use `-` to read from stdin. |
| `--prompt <TEXT>` | — | Alternative way to pass the prompt (same effect as positional). |

---

## Headless / Print Mode

These flags activate and configure non-interactive (`-p`) mode. See [Print Mode](../automation/print-mode.md) and [The stream-json Protocol](../automation/stream-json.md).

| Flag | Default | Description |
|------|---------|-------------|
| `-p`, `--print [PROMPT]` | — | Headless mode. Drives the agent non-interactively. Accepts an optional prompt; otherwise reads from `--prompt`, the positional `PROMPT`, or stdin (capped at 10 MiB). |
| `--output-format <FMT>` | `text` | Stream output format. Values: `text`, `json`, `stream-json`. |
| `--input-format <FMT>` | `text` | Stdin format. Values: `text`, `stream-json`. |
| `--no-auto-print` | `false` | Suppress the automatic headless dispatch when stdout is piped or stdin is non-TTY. Explicit `--print` / `--output-format` always override this. |
| `--max-budget-usd <USD>` | — | Abort the run (exit 137) once cumulative cost exceeds this value in USD. Unknown model/provider pairs contribute $0 and emit a warning. |
| `--bare` | `false` | CI-deterministic mode: skips hooks, skills, plugins, MCP, auto-memory, and CLAUDE.md discovery. |
| `--json-schema <FILE_OR_JSON>` | — | Force structured final output matching the given JSON Schema. Value can be inline JSON or a path to a `.json` file. |
| `--include-partial-messages` | `false` | Emit assistant text deltas as separate `text` frames in `stream-json` mode (default: aggregate into one `message` frame). |
| `--include-hook-events` | `false` | Emit a `hook_event` frame per fired hook event in `stream-json` mode. |
| `--replay-user-messages` | `false` | Echo each user prompt as a `user` frame in `stream-json` mode. |

---

## Session

| Flag | Default | Description |
|------|---------|-------------|
| `-c`, `--continue` | `false` | Resume the most recently updated session. |
| `-r`, `--resume <NAME>` | — | Resume a named session. |
| `--session <NAME>` | — | Load or create a named session; persists to the configured sessions directory. |
| `--no-save` | `false` | Don't write the session back to disk after the run. |
| `--sessions-dir <DIR>` | platform default | Override the sessions directory. |

---

## Model & Provider

| Flag | Default | Description |
|------|---------|-------------|
| `--provider <PROVIDER>` | Resolved from settings, then `anthropic` | Provider to use. Values: `anthropic`, `openai`, `ollama`, `google`. |
| `--model <MODEL>` | Provider default (see table below) | Model name. |
| `--fallback-model <MODEL>` | From settings | Fallback model when the primary errors (ADR 0038). |
| `--max-tokens <N>` | `2048` | Per-turn output token limit (must be ≥ 1). |
| `--max-turns <N>` | `50` | Maximum agent loop iterations. |
| `--temperature <F>` | — | Sampling temperature in `[0.0, 2.0]`. |

**Provider defaults:**

| Provider | Default model |
|----------|--------------|
| `anthropic` | `claude-sonnet-4-6` |
| `openai` | `gpt-5.5` |
| `ollama` | `llama3.1` |
| `google` | `gemini-2.0-flash` |

---

## Workspace & Tools

| Flag | Default | Description |
|------|---------|-------------|
| `--workspace <DIR>` | Current working directory | Workspace root for file and shell tools. Must be an existing directory. |
| `--no-tools` | `false` | Disable all tools (chat-only mode). |
| `--restrict-paths` | `false` | Reject tool paths outside the workspace root. |
| `--quiet` | `false` | Suppress tool-execution announcements. |

---

## System Prompt

These flags are mutually exclusive.

| Flag | Default | Description |
|------|---------|-------------|
| `--system <STRING>` | — | Override system prompt with the given text. |
| `--system-file <PATH>` | — | Override system prompt with the contents of a file. |
| `--no-system` | `false` | Run with no system prompt (disables the default). |

---

## Permissions

See [Permission Modes](../permissions/modes.md) and [Managing Rules](../permissions/managing.md).

| Flag | Default | Description |
|------|---------|-------------|
| `--allow <PAT>` | — | Add an Allow rule at top priority. Repeatable. Pattern: `Tool` or `Tool:first-arg-glob`. |
| `--deny <PAT>` | — | Add a Deny rule at top priority. Repeatable. |
| `--ask <PAT>` | — | Add an Ask rule at top priority. Repeatable. |
| `--permission-mode <MODE>` | From settings or `default` | Initial permission mode. Valid values (camelCase): `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`. Env: `CALIBAN_DEFAULT_PERMISSION_MODE`. |
| `--no-permissions` | `false` | Disable permission gating entirely (all tool calls allowed). Env: `CALIBAN_NO_PERMISSIONS`. Conflicts with `--allow`, `--deny`, `--ask`, `--auto-allow`. |
| `--auto-allow` | `false` | **Dangerous.** Allow the model to run any Ask-rule tool without prompting in non-interactive mode. Env: `CALIBAN_AUTO_ALLOW`. |
| `--allow-dangerously-skip-permissions` | `false` | **Dangerous.** Required to enter `bypassPermissions` mode. Without this flag the binary refuses to start in bypass mode. |
| `--disable-auto-mode` | `false` | Disable the auto-mode classifier; every call falls through to the Ask handler (ADR 0029). Env: `CALIBAN_DISABLE_AUTO_MODE`. |
| `--permission-prompt-tool <MCP_TOOL>` | — | Route permission Ask events to the named MCP tool via the MCP elicitation channel (ADR 0023 Phase C). |

---

## Hooks, Skills, MCP & Plugins

| Flag | Default | Description |
|------|---------|-------------|
| `--no-hooks` | `false` | Bypass every external hook handler. In-process hooks (PermissionsHook, audit) still run. Env: `CALIBAN_NO_HOOKS`. |
| `--no-skills` | `false` | Disable the Skill tool (no skill discovery at startup). Env: `CALIBAN_NO_SKILLS`. |
| `--no-mcp` | `false` | Disable MCP server discovery (skips `settings.json` `mcp_servers` and the legacy `mcp.toml` shim). Env: `CALIBAN_NO_MCP`. |
| `--no-plugins` | `false` | Disable plugin discovery (ADR 0030). Env: `CALIBAN_NO_PLUGINS`. |
| `--mcp-oauth-port <PORT>` | `0` (ephemeral) | Override the loopback port for the OAuth callback server (ADR 0023 Phase C). Env: `CALIBAN_MCP_OAUTH_PORT`. |
| `--no-sub-agent` | `false` | Disable the built-in `AgentTool` (the sub-agent primitive). Env: `CALIBAN_NO_SUB_AGENT`. |

---

## Config & Settings

| Flag | Default | Description |
|------|---------|-------------|
| `--config <PATH>` | Walk-up discovery | Explicit path to `caliban.toml`. When the file declares `[router]`, a model router is wired (ADR 0038). Env: `CALIBAN_ROUTER_CONFIG`. |
| `--settings <FILE_OR_JSON>` | — | Inject a virtual settings scope above local (ADR 0026). Accepts inline JSON or a path to `.json` / `.toml`. |
| `--setting-sources <CSV>` | All scopes | Restrict which `settings.json` scopes are read. CSV of `managed,user,project,local`. |

---

## Caching & Performance

| Flag | Default | Description |
|------|---------|-------------|
| `--max-attach-bytes <N>` | `262144` (256 KB) | Maximum size of a single `@`-attachment in bytes. Env: `CALIBAN_MAX_ATTACH_BYTES`. |
| `--attach-budget-bytes <N>` | `1048576` (1 MB) | Aggregate size cap across all `@`-attachments in one message. Env: `CALIBAN_ATTACH_BUDGET_BYTES`. |
| `--no-prompt-cache` | `false` | Disable Anthropic-style prompt caching. Env: `CALIBAN_NO_PROMPT_CACHE`. |
| `--no-parallel-tools` | `false` | Disable parallel tool execution (run `tool_use` blocks serially). Env: `CALIBAN_NO_PARALLEL_TOOLS`. |
| `--parallel-tool-limit <N>` | CPU cores − 1 (min 1) | Max concurrent tool invocations per turn. Env: `CALIBAN_PARALLEL_TOOL_LIMIT`. |

---

## Diagnostics

| Flag | Default | Description |
|------|---------|-------------|
| `--debug` | `false` | Append-log events and draws to the platform debug log. `CALIBAN_DEBUG` (any non-empty value) also enables this. |

---

## Background Agents

| Flag | Default | Description |
|------|---------|-------------|
| `--bg <TASK>` | — | Spawn a background sub-agent with the given task and return immediately. Equivalent to `caliban agents spawn --bg --prompt <TASK>` (ADR 0037). |

---

## Subcommands

### `caliban doctor [--deep]`

Run health checks against the local caliban install (settings, MCP, sandbox, stores, providers). Exit 0 on pass, 1 on failure.

| Option | Description |
|--------|-------------|
| `--deep` | Include deep checks (provider auth pings — costs one API call per configured provider). |

---

### `caliban config`

Inspect and migrate settings (ADR 0026).

| Sub-subcommand | Description |
|----------------|-------------|
| `config print` | Print the merged effective settings as JSON, including the per-key scope chain. Honors `--settings` / `--setting-sources`. |
| `config migrate [--dry-run]` | Round-trip legacy per-feature TOMLs (`permissions.toml`, `mcp.toml`, `hooks.toml`) into a single project-scope `settings.json` under `<workspace>/.caliban/`. |

---

### `caliban settings`

Import and print settings files.

| Sub-subcommand | Description |
|----------------|-------------|
| `settings import --from <PATH> [--scope <SCOPE>] [--dry-run]` | Import a settings JSON (Claude Code / Codex / legacy caliban) into canonical caliban TOML. Default scope: `project`. |
| `settings print [--scope <SCOPE>]` | Print the settings for a scope (or the merged effective settings). Default scope: `project`. |

---

### `caliban perms`

Manage permission rules across all config scopes. See [Managing Rules](../permissions/managing.md).

| Sub-subcommand | Description |
|----------------|-------------|
| `perms list [--scope <SCOPE>] [--effective] [--json]` | List permission rules. `--effective` shows the merged rule list across all scopes. |
| `perms test <TOOL> [INPUT_JSON]` | Test whether a tool call would be allowed, denied, or asked. |
| `perms explain <TOOL> [INPUT_JSON]` | Show which rule first matches a tool call. |
| `perms add <PATTERN> <ACTION> [--scope <SCOPE>] [--comment <TEXT>] [--reason <TEXT>]` | Add a permission rule. Action: `allow`, `ask`, or `deny`. Default scope: `project`. |
| `perms remove [--index <N>] [--pattern <PAT>] [--scope <SCOPE>]` | Remove a permission rule by ordinal or pattern. Default scope: `project`. |
| `perms import --from <PATH> [--scope <SCOPE>] [--dry-run]` | Import rules from a foreign config (Claude Code JSON, legacy caliban TOML). Default scope: `user`. |
| `perms export [--scope <SCOPE>] [--format toml\|json]` | Export permission rules to stdout. Default format: `toml`. |
| `perms audit [--since <ISO>] [--tool <NAME>] [--action <ACTION>] [--head <N>]` | Show the permission-decision audit log. |
| `perms lint [--scope <SCOPE>]` | Check for duplicate or conflicting rules. Default scope: `project`. |

---

### `caliban agents`

List, attach, and manage background sub-agents (ADR 0037).

| Sub-subcommand | Description |
|----------------|-------------|
| `agents list` | List registered background agents. |
| `agents spawn --prompt <TEXT> [--label <LABEL>]` | Spawn a new background agent. |
| `agents attach <ID>` | Stream a running agent's transcript live (Ctrl+D detaches). |
| `agents logs <ID>` | Print the agent's session log. |
| `agents kill <ID>` | Terminate an agent (SIGTERM → SIGKILL after grace period). |
| `agents respawn <ID>` | Restart an agent with the same spawn spec. |
| `agents rm <ID> [--force]` | Remove an agent from the registry (must be stopped unless `--force`). |

**Shortcut aliases** (top-level sugar):

| Command | Equivalent to |
|---------|--------------|
| `caliban attach <ID>` | `caliban agents attach <ID>` |
| `caliban logs <ID>` | `caliban agents logs <ID>` |
| `caliban stop <ID>` | `caliban agents kill <ID>` |
| `caliban kill <ID>` | `caliban agents kill <ID>` |
| `caliban respawn <ID>` | `caliban agents respawn <ID>` |
| `caliban rm <ID> [--force]` | `caliban agents rm <ID>` |

---

### `caliban daemon`

Supervisor daemon management (ADR 0037).

| Sub-subcommand | Description |
|----------------|-------------|
| `daemon status` | Print daemon health and the socket path. |
| `daemon stop` | Ask the daemon to shut down gracefully. |

---

### `caliban router debug`

Router diagnostics (ADR 0038).

| Sub-subcommand | Description |
|----------------|-------------|
| `router debug` | Print the candidate list the router would resolve for a synthetic request, plus breaker state and effort knobs. |

---

### `caliban plugin <VERB> [ARGS…]`

Manage plugin packages (ADR 0030). The plugin CLI parses its own verbs directly:

| Verb | Description |
|------|-------------|
| `plugin list` | List all discovered plugins with enable/disable status. |
| `plugin info <NAME>` | Show manifest details for a plugin. |
| `plugin install <NAME>@<MARKETPLACE> [--yes]` | Install a plugin from a marketplace. |
| `plugin install --dir <PATH>` | Install a plugin from a local directory. |
| `plugin update <NAME> [--yes]` | Update an installed plugin. |
| `plugin remove <NAME>` | Remove an installed plugin. |
| `plugin enable <NAME>` | Enable a disabled plugin. |
| `plugin disable <NAME>` | Disable an enabled plugin. |

Run `caliban plugin help` for the full plugin CLI reference.

---

```admonish note title="Exit codes"
Caliban follows ADR 0025 exit-code conventions: `0` = success, `1` = check/health failure, `64` = usage error (`EX_USAGE`), `78` = configuration error (`EX_CONFIG`), `130` = Ctrl+C, `137` = budget exceeded.
```
