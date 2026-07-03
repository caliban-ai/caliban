# Files & Directories

Caliban is **XDG-first on every platform** ([ADR 0050](https://github.com/caliban-ai/caliban/blob/main/docs/adr/0050-xdg-first-path-locations.md)). It uses the same directory layout on Linux, macOS, and Windows: `~/.config`, `~/.local/share`, `~/.local/state`, and `~/.cache` (each overridable by the matching `XDG_*_HOME` variable), with a `caliban/` app segment. macOS does **not** use `~/Library/Application Support`, and there is no `~/.caliban` home dotdir — a terminal-first tool keeps its files where CLI users look.

```admonish warning title="Breaking change (v0.4.0)"
Earlier builds stored user data under `~/Library/Application Support/caliban` (macOS) and `~/.caliban` (all platforms). Those locations are **abandoned with no automatic migration** — caliban starts fresh at the XDG paths below. To carry over Claude Code / Codex settings, use the manual importer: `caliban settings import --from ~/.claude/settings.json --scope user`.
```

```admonish tip title="Override with environment variables"
Every base dir honors its XDG variable (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME`) on all platforms. Category-specific overrides (`CALIBAN_CHECKPOINT_ROOT`, `CALIBAN_MEMORY_DIR`, `CALIBAN_DAEMON_RUNTIME_DIR`, `CALIBAN_ROUTER_CONFIG`, `CALIBAN_DEBUG`) take precedence — see [Environment Variables](./env-vars.md).
```

The tables below show the canonical path. Unless noted, it is identical on all platforms; on Windows the defaults resolve under `%USERPROFILE%` (e.g. `%USERPROFILE%\.config\caliban\`).

---

## Settings Files

Caliban loads settings from up to five scopes in precedence order (highest → lowest). See [Settings Layering](../configuration/settings-layering.md) for merge semantics.

| Scope | Path |
|-------|------|
| **Managed** (enterprise) | `/etc/caliban/managed-settings.{toml,json}` (Unix) · `C:\ProgramData\Caliban\managed-settings.{toml,json}` (Windows) |
| **User** | `$XDG_CONFIG_HOME/caliban/settings.{toml,json}` (default `~/.config/caliban/`) |
| **Project** | `<workspace>/.caliban/settings.{toml,json}` |
| **Local** (gitignored) | `<workspace>/.caliban/settings.local.{toml,json}` |
| **CLI overlay** | Supplied via `--settings <FILE_OR_JSON>` |

Both `.toml` and `.json` are accepted at each scope. TOML is preferred; JSON is accepted for Claude Code import compatibility.

---

## Sessions

Named sessions are stored as JSON files in the data directory. Override with `--sessions-dir <DIR>`.

`$XDG_DATA_HOME/caliban/sessions/<name>.json` (default `~/.local/share/caliban/sessions/`)

---

## Checkpoints

Checkpoints use a content-addressed layout keyed on a SHA-256 hash of the canonicalized workspace path. The `<cwd-hash>` is the first 16 hex characters of `SHA-256(canonicalized_cwd)`.

`$XDG_DATA_HOME/caliban/projects/<cwd-hash>/checkpoints/<session>/prompt-NNN/`
(default `~/.local/share/caliban/projects/…`)

Override the root with `CALIBAN_CHECKPOINT_ROOT`. Disable recording with `CALIBAN_CHECKPOINT_DISABLED`.

---

## Debug Log

Enabled by `--debug` or `CALIBAN_DEBUG` (any non-empty value). Append-only; rotated automatically.

`$XDG_CACHE_HOME/caliban/debug.log` (default `~/.cache/caliban/`)

---

## Audit / Permission-Decision Log

Append-only JSONL log of every permission decision (allow/ask/deny) with tool name, matched rule, and session context. Enabled by default; disable via `permissions.audit_log = false` in settings.

`$XDG_STATE_HOME/caliban/permission-decisions.jsonl` (default `~/.local/state/caliban/`)

View with `caliban perms audit [--since <ISO>] [--tool <NAME>] [--action <ACTION>] [--head <N>]`.

---

## Skills

Skills are loaded from several roots, checked in this order. Each skill lives in a subdirectory with a `SKILL.md`: `<root>/<name>/SKILL.md`.

| Root | Path |
|------|------|
| **Project** | `<workspace>/.caliban/skills/` |
| **User (config)** | `$XDG_CONFIG_HOME/caliban/skills/` (default `~/.config/caliban/skills/`) |
| **User (data)** | `$XDG_DATA_HOME/caliban/skills/` (default `~/.local/share/caliban/skills/`) |
| **Plugin-contributed** | Via plugin data root |

---

## Plugins

| Location | Path |
|----------|------|
| **Project plugins** | `<workspace>/.caliban/plugins/` |
| **User plugins** | `$XDG_DATA_HOME/caliban/plugins/` (default `~/.local/share/caliban/plugins/`) |
| **Managed plugins** | `/etc/caliban/plugins/` (Unix) · `C:\ProgramData\Caliban\plugins\` (Windows) |
| **Plugin trust store** | `$XDG_DATA_HOME/caliban/trust/plugins.json` |
| **Marketplace allowlist** | `$XDG_DATA_HOME/caliban/marketplaces-allowlist.json` |

---

## MCP Configuration (Legacy)

The legacy `mcp.toml` is still loaded during the back-compat window. Because it now resolves under the same `~/.config/caliban` as `settings.toml`, defining a server via `[mcp_servers]` in `settings.toml` and via `mcp.toml` no longer diverge.

| Location | Path |
|----------|------|
| **Project** | `<workspace>/.caliban/mcp.toml` |
| **User** | `$XDG_CONFIG_HOME/caliban/mcp.toml` (default `~/.config/caliban/mcp.toml`) |

MCP servers are now configured in `settings.toml` under `[mcp_servers]`. See [MCP Servers](../extending/mcp.md).

---

## Hooks / Permissions Configuration (Legacy)

Legacy per-feature TOMLs are still loaded during the back-compat window.

| File | Project | User |
|------|---------|------|
| **`hooks.toml`** | `<workspace>/.caliban/hooks.toml` | `$XDG_CONFIG_HOME/caliban/hooks.toml` |
| **`permissions.toml`** | `<workspace>/.caliban/permissions.toml` | `$XDG_CONFIG_HOME/caliban/permissions.toml` |

Both are now configured in `settings.toml` under `[hooks]` / `[permissions]`.

---

## Model Router Config

| Scope | Path |
|-------|------|
| **Project** | `<workspace>/caliban.toml` (walk-up discovery) |
| **User** | `$XDG_CONFIG_HOME/caliban/caliban.toml` (default `~/.config/caliban/caliban.toml`) |

Override with `--config <PATH>` or `CALIBAN_ROUTER_CONFIG`.

---

## Output Styles

| Scope | Path |
|-------|------|
| **Project** | `<workspace>/.caliban/output-styles/` |
| **User** | `$XDG_CONFIG_HOME/caliban/output-styles/` |
| **Plugin-contributed** | Via plugin data root |

---

## Memory

| Tier | Path |
|------|------|
| **Global CLAUDE.md** | `$XDG_CONFIG_HOME/caliban/CLAUDE.md` |
| **Auto-memory** | `$XDG_DATA_HOME/caliban/projects/<cwd-slug>/memory/` (override: `CALIBAN_MEMORY_DIR`) |
| **User rules** | `$XDG_CONFIG_HOME/caliban/rules/` |
| **Project rules** | `<workspace>/.caliban/rules/` |
| **Imports allowlist** | `$XDG_STATE_HOME/caliban/imports-allowlist.json` |

---

## Tool-Result Overflow Spill

When a tool result exceeds `tool_result_cap_chars`, the full result is spilled to disk and the inline message carries a truncated excerpt with a pointer.

`$XDG_CACHE_HOME/caliban/tool-overflows/<session-id>/<tool-use-id>.txt`

Falls back to `/tmp/caliban-tool-overflows/` when the cache directory cannot be determined.

---

## Input History

Per-project input history is stored alongside the checkpoint tree; all histories are reachable under the projects root (used by the Ctrl+R all-projects search scope).

`$XDG_DATA_HOME/caliban/projects/<cwd-hash>/input-history.txt`

---

## Worktrees

Git worktrees managed by caliban are kept inside the repository:

`<repo-root>/.caliban/worktrees/<name>/`

---

## Supervisor / Daemon State

| Kind | Path |
|------|------|
| **Daemon data** | `$XDG_DATA_HOME/caliban/` (default `~/.local/share/caliban/`) |
| **Sockets** | `$XDG_RUNTIME_DIR/caliban/` when set, else under the daemon data dir |

Override with `CALIBAN_DAEMON_RUNTIME_DIR`.

---

```admonish note title="XDG environment variable overrides"
All `$XDG_*_HOME` variables are honored on **every** platform, not just Linux. When unset, the `~/.config` / `~/.local/share` / `~/.local/state` / `~/.cache` defaults apply uniformly — including macOS and Windows.
```
