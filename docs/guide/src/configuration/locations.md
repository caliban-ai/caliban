# File Locations

Caliban resolves settings files from four on-disk scopes. Paths are **XDG-first on every platform** ([ADR 0050](https://github.com/caliban-ai/caliban/blob/main/docs/adr/0050-xdg-first-path-locations.md)) — the same layout on Linux, macOS, and Windows.

## Scope paths

### Managed scope

Set by a system administrator. Caliban reads but never writes this directory.

| OS | Path |
|----|------|
| Unix (Linux + macOS) | `/etc/caliban/managed-settings.toml` |
| Windows | `C:\ProgramData\Caliban\managed-settings.toml` |

The JSON equivalent (`managed-settings.json`) is accepted on read as a legacy path but triggers a `WARN` on startup.

### User scope

Per-user settings that apply across all projects. Caliban resolves the user-configuration directory XDG-first: `$XDG_CONFIG_HOME/caliban/`, defaulting to `~/.config/caliban/` on every platform.

| OS | Path |
|----|------|
| All (Linux / macOS / Windows) | `~/.config/caliban/settings.toml` (or `$XDG_CONFIG_HOME/caliban/settings.toml`) |

### Project scope

Committed alongside your code. This file should be checked into version control and shared with your team.

| OS | Path |
|----|------|
| All | `<workspace>/.caliban/settings.toml` |

### Local scope

Machine-local overrides that should **not** be committed. Add `.caliban/settings.local.toml` to your `.gitignore`.

| OS | Path |
|----|------|
| All | `<workspace>/.caliban/settings.local.toml` |

## Per-feature files (legacy)

Caliban still loads standalone per-feature TOML files during the current compatibility window. They are consulted **only when the corresponding key is absent from the unified settings file** in the same scope directory.

| File | Key governed | Notes |
|------|-------------|-------|
| `.caliban/permissions.toml` | `permissions` | Can also coexist alongside `settings.toml`; its `permissions` block overrides the `permissions` key in `settings.toml` for that scope |
| `.caliban/mcp.toml` | `mcp_servers` | Legacy transport key is `transport`; canonical key is `type` |
| `.caliban/hooks.toml` | `hooks`, `disable_all_hooks`, `allow_managed_hooks_only`, `allowed_http_hook_urls`, `http_hook_allowed_env_vars` | |

```admonish warning title="Deprecation timeline"
Per-feature TOML files are deprecated. Caliban logs a `WARN` when it falls back to them. After two minor releases the warning becomes an error. Run `caliban config migrate` to consolidate them into a single `settings.toml`.
```

## TOML vs JSON

TOML is the canonical write format. JSON is accepted on read as a legacy/import path:

- When both `settings.toml` **and** `settings.json` exist in the same scope directory, `.toml` wins and caliban logs a `WARN` about the ignored `.json` file.
- When only `settings.json` exists, caliban loads it with a `WARN` recommending migration.
- Caliban's own write paths (modal, `caliban perms add`, `/permissions` editor) always emit TOML.

## Atomic writes

All caliban-owned writes use an atomic flock + temp-file rename pattern:

1. A sibling `.settings.toml.lock` file is exclusively flocked.
2. Content is written to a uniquely-named `.toml.tmp.<pid>.<tid>` file.
3. The temp file is synced and renamed onto the target.
4. The lock is released.

This ensures concurrent writers (e.g. two terminal sessions) never produce a corrupted file.

```admonish tip title="Consolidated path reference"
See [Files & Directories](../reference/paths.md) for the full list of all caliban-managed paths including sessions, cache, logs, and debug output.
```
