# Files & Directories

Caliban follows platform conventions for each OS via the `dirs` crate. The tables below show the resolved path for each category on macOS, Linux (with XDG defaults), and Windows.

```admonish tip title="Override with environment variables"
Many paths can be overridden with environment variables — see [Environment Variables](./env-vars.md). The `CALIBAN_CHECKPOINT_ROOT`, `CALIBAN_MEMORY_DIR`, `CALIBAN_DAEMON_RUNTIME_DIR`, and `CALIBAN_DEBUG` variables are the most commonly needed.
```

---

## Settings Files

Caliban loads settings from up to five scopes in precedence order (highest → lowest). See [Settings Layering](../configuration/settings-layering.md) for merge semantics.

| Scope | macOS | Linux | Windows |
|-------|-------|-------|---------|
| **Managed** (enterprise) | `/Library/Application Support/Caliban/managed-settings.{toml,json}` | `/etc/caliban/managed-settings.{toml,json}` | `C:\ProgramData\Caliban\managed-settings.{toml,json}` |
| **User** | `~/Library/Application Support/caliban/settings.{toml,json}` | `$XDG_CONFIG_HOME/caliban/settings.{toml,json}` (default: `~/.config/caliban/`) | `%APPDATA%\caliban\settings.{toml,json}` |
| **Project** | `<workspace>/.caliban/settings.{toml,json}` | `<workspace>/.caliban/settings.{toml,json}` | `<workspace>\.caliban\settings.{toml,json}` |
| **Local** (gitignored) | `<workspace>/.caliban/settings.local.{toml,json}` | `<workspace>/.caliban/settings.local.{toml,json}` | `<workspace>\.caliban\settings.local.{toml,json}` |
| **CLI overlay** | Supplied via `--settings <FILE_OR_JSON>` | — | — |

Both `.toml` and `.json` are accepted at each scope. TOML is preferred; JSON is accepted for Claude Code import compatibility.

---

## Sessions

Named sessions are stored as JSON files in the sessions directory.

| macOS | Linux | Windows |
|-------|-------|---------|
| `~/Library/Application Support/caliban/sessions/<name>.json` | `$XDG_DATA_HOME/caliban/sessions/<name>.json` (default: `~/.local/share/caliban/sessions/`) | `%LOCALAPPDATA%\caliban\sessions\<name>.json` |

Override with `--sessions-dir <DIR>`.

---

## Checkpoints

Checkpoints use a content-addressed layout keyed on a SHA-256 hash of the canonicalized workspace path.

| macOS | Linux | Windows |
|-------|-------|---------|
| `~/.caliban/projects/<cwd-hash>/checkpoints/<session>/prompt-NNN/` | `~/.caliban/projects/<cwd-hash>/checkpoints/<session>/prompt-NNN/` | `%USERPROFILE%\.caliban\projects\<cwd-hash>\checkpoints\<session>\prompt-NNN\` |

The `<cwd-hash>` is the first 16 hex characters of `SHA-256(canonicalized_cwd)`.

Override the root with `CALIBAN_CHECKPOINT_ROOT`. Disable recording entirely with `CALIBAN_CHECKPOINT_DISABLED`.

---

## Debug Log

Enabled by `--debug` or `CALIBAN_DEBUG` (any non-empty value). Append-only; rotated automatically.

| macOS | Linux | Windows |
|-------|-------|---------|
| `~/Library/Caches/caliban/debug.log` | `$XDG_CACHE_HOME/caliban/debug.log` (default: `~/.cache/caliban/`) | `%LOCALAPPDATA%\caliban\cache\caliban\debug.log` |

---

## Audit / Permission-Decision Log

Append-only JSONL log of every permission decision (allow/ask/deny) with tool name, matched rule, and session context. Enabled by default; disable via `permissions.audit_log = false` in settings.

| macOS | Linux | Windows |
|-------|-------|---------|
| `~/Library/Application Support/caliban/permission-decisions.jsonl` ¹ | `$XDG_STATE_HOME/caliban/permission-decisions.jsonl` (default: `~/.local/state/caliban/`) | `%LOCALAPPDATA%\caliban\permission-decisions.jsonl` ¹ |

¹ macOS and Windows lack a `state_dir` equivalent; caliban falls back to `data_local_dir` (`~/Library/Application Support/` / `%LOCALAPPDATA%`).

View with `caliban perms audit [--since <ISO>] [--tool <NAME>] [--action <ACTION>] [--head <N>]`.

---

## Skills

Skills are loaded from several roots, checked in this order:

| Root | macOS | Linux | Windows |
|------|-------|-------|---------|
| **Project** | `<workspace>/.caliban/skills/` | `<workspace>/.caliban/skills/` | `<workspace>\.caliban\skills\` |
| **User** | `~/Library/Application Support/caliban/skills/` | `$XDG_CONFIG_HOME/caliban/skills/` | `%APPDATA%\caliban\skills\` |
| **Local data** | `~/Library/Application Support/caliban/skills/` | `$XDG_DATA_HOME/caliban/skills/` | `%LOCALAPPDATA%\caliban\skills\` |
| **Plugin-contributed** | Varies per plugin install | — | — |

Each skill lives in a subdirectory with a `SKILL.md` file: `<root>/<name>/SKILL.md`.

---

## Plugins

| Location | macOS | Linux | Windows |
|----------|-------|-------|---------|
| **Project plugins** | `<workspace>/.caliban/plugins/` | `<workspace>/.caliban/plugins/` | `<workspace>\.caliban\plugins\` |
| **User plugins** | `~/Library/Application Support/caliban/plugins/` | `$XDG_DATA_HOME/caliban/plugins/` (default: `~/.local/share/caliban/plugins/`) | `%LOCALAPPDATA%\caliban\plugins\` |
| **Plugin trust store** | `~/Library/Application Support/caliban/plugin-trust.json` | `~/.local/share/caliban/plugin-trust.json` | `%LOCALAPPDATA%\caliban\plugin-trust.json` |
| **Marketplace allowlist** | `~/.caliban/marketplaces-allowlist.json` | `~/.caliban/marketplaces-allowlist.json` | `%USERPROFILE%\.caliban\marketplaces-allowlist.json` |

---

## MCP Configuration (Legacy)

The legacy `mcp.toml` is still loaded during the back-compat window:

| Location | macOS | Linux | Windows |
|----------|-------|-------|---------|
| **Project** | `<workspace>/.caliban/mcp.toml` | `<workspace>/.caliban/mcp.toml` | `<workspace>\.caliban\mcp.toml` |
| **User** | `~/Library/Application Support/caliban/mcp.toml` | `$XDG_CONFIG_HOME/caliban/mcp.toml` | `%APPDATA%\caliban\mcp.toml` |

MCP servers are now configured in `settings.toml` under `[mcp_servers]`. See [MCP Servers](../extending/mcp.md).

---

## Hooks Configuration (Legacy)

Legacy `hooks.toml` files are still loaded during the back-compat window:

| Location | macOS | Linux | Windows |
|----------|-------|-------|---------|
| **Project** | `<workspace>/.caliban/hooks.toml` | `<workspace>/.caliban/hooks.toml` | `<workspace>\.caliban\hooks.toml` |
| **User** | `~/Library/Application Support/caliban/hooks.toml` | `$XDG_CONFIG_HOME/caliban/hooks.toml` | `%APPDATA%\caliban\hooks.toml` |

Hooks are now configured in `settings.toml` under `[hooks]`. See [Hooks](../extending/hooks.md).

---

## Permissions Configuration (Legacy)

| Location | macOS | Linux | Windows |
|----------|-------|-------|---------|
| **Project** | `<workspace>/.caliban/permissions.toml` | `<workspace>/.caliban/permissions.toml` | `<workspace>\.caliban\permissions.toml` |
| **User** | `~/Library/Application Support/caliban/permissions.toml` | `$XDG_CONFIG_HOME/caliban/permissions.toml` | `%APPDATA%\caliban\permissions.toml` |

Permissions are now configured in `settings.toml` under `[permissions]`. See [Managing Rules](../permissions/managing.md).

---

## Model Router Config

| Location | macOS / Linux / Windows |
|----------|------------------------|
| **Project** | `<workspace>/caliban.toml` (walk-up discovery) |
| **User** | `~/Library/Application Support/caliban/caliban.toml` (macOS) / `$XDG_CONFIG_HOME/caliban/caliban.toml` (Linux) |

Override with `--config <PATH>` or `CALIBAN_ROUTER_CONFIG`.

---

## Output Styles

| Location | macOS | Linux | Windows |
|----------|-------|-------|---------|
| **Project** | `<workspace>/.caliban/output-styles/` | `<workspace>/.caliban/output-styles/` | `<workspace>\.caliban\output-styles\` |
| **User** | `~/Library/Application Support/caliban/output-styles/` | `$XDG_CONFIG_HOME/caliban/output-styles/` | `%APPDATA%\caliban\output-styles\` |
| **Plugin-contributed** | Via plugin data root | — | — |

---

## Tool-Result Overflow Spill

When a tool result exceeds `tool_result_cap_chars`, the full result is spilled to disk and the inline message contains a truncated excerpt with a pointer.

| macOS | Linux | Windows |
|-------|-------|---------|
| `~/Library/Caches/caliban/tool-overflows/<session-id>/<tool-use-id>.txt` | `$XDG_CACHE_HOME/caliban/tool-overflows/<session-id>/<tool-use-id>.txt` | `%LOCALAPPDATA%\caliban\cache\caliban\tool-overflows\<session-id>\<tool-use-id>.txt` |

Falls back to `/tmp/caliban-tool-overflows/` when the cache directory cannot be determined.

---

## Input History

Per-project input history is stored alongside the checkpoint tree:

| All platforms |
|---------------|
| `~/.caliban/projects/<cwd-hash>/input-history.txt` |

All project histories are accessible via `~/.caliban/projects/` (used by the Ctrl+R all-projects search scope).

---

## Worktrees

Git worktrees managed by caliban are kept inside the repository:

| All platforms |
|---------------|
| `<repo-root>/.caliban/worktrees/<name>/` |

---

## Supervisor / Daemon State

| macOS | Linux | Windows |
|-------|-------|---------|
| `~/Library/Application Support/caliban/` (daemon data) | `$XDG_DATA_HOME/caliban/` | `%LOCALAPPDATA%\caliban\` |
| `$XDG_RUNTIME_DIR/caliban/` or `~/Library/Application Support/caliban/run/` (sockets) | `$XDG_RUNTIME_DIR/caliban/` (sockets) | `%LOCALAPPDATA%\caliban\run\` (sockets) |

Override with `CALIBAN_DAEMON_RUNTIME_DIR`.

---

```admonish note title="XDG environment variable overrides"
On Linux, all `$XDG_*` variables are honored when set. If unset, the defaults shown above apply. macOS and Windows do not use XDG paths; the `dirs` crate maps to the platform-native locations shown.
```
