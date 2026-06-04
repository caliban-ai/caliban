# Config Commands

Caliban ships two subcommand families for inspecting and managing settings: `caliban config` for the unified settings layer, and `caliban settings` for import/export of individual scope files. Both work without a running session.

## `caliban config print`

Prints the fully-merged effective settings as JSON, annotated with the scope each value came from. Honors `--settings` and `--setting-sources` so you can preview what a CI run or a different scope combination would see.

```bash
caliban config print

# Show only project + user scopes (skip local)
caliban --setting-sources user,project config print

# Preview with a CLI overlay applied
caliban --settings '{"model": "claude-opus-4-7"}' config print
```

The output shows the merged `Settings` object. Each top-level key lists the scope that contributed the winning value. This is the headless equivalent of the read-only `Effective` tab in the `/config` TUI overlay.

## `caliban config migrate`

Consolidates legacy per-feature TOML files (`permissions.toml`, `mcp.toml`, `hooks.toml`) in the current workspace into a single `.caliban/settings.toml`. Existing keys in the target file are preserved; the migrated keys are merged on top.

```bash
# Preview what would be written (nothing is changed)
caliban config migrate --dry-run

# Run the migration
caliban config migrate
```

After migration the per-feature files are no longer read (caliban checks for the unified key first). You can safely delete them, or leave them in place — caliban will ignore them once the corresponding key exists in `settings.toml`.

```admonish tip title="When to migrate"
Run `caliban config migrate` once after upgrading to a version that shipped ADR 0026. It is safe to run multiple times — the command is idempotent.
```

## `caliban settings import`

Imports a settings file from a foreign format (Claude Code JSON, Codex JSON, or legacy caliban JSON) into canonical caliban TOML at the target scope.

```bash
# Import ~/.claude.json into the user scope (dry-run first)
caliban settings import --from ~/.claude.json --scope user --dry-run
caliban settings import --from ~/.claude.json --scope user

# Import a project settings file into the project scope
caliban settings import --from /path/to/settings.json
```

Options:

| Flag | Description |
|------|-------------|
| `--from <PATH>` | Path to the source file (required) |
| `--scope <SCOPE>` | Destination scope: `managed`, `user`, `project`, or `local`. Default: `project` |
| `--dry-run` | Print what would be written without making changes |

`caliban settings import` is the recommended migration path when you have an existing Claude Code `settings.json` you want to adopt. The source file is read-only; only the target scope's TOML is written.

## `caliban settings print`

Prints the raw settings for a single scope (before merging), or the merged effective settings when no scope is specified.

```bash
# Print the project-scope settings
caliban settings print

# Print the user-scope settings
caliban settings print --scope user
```

Options:

| Flag | Description |
|------|-------------|
| `--scope <SCOPE>` | Scope to print. Default: `project` |

This differs from `caliban config print` in that it shows the unmerged raw contents of one scope rather than the merged result across all scopes.

---

## TOML-primary write / JSON import-only

Caliban always writes TOML. JSON files at any scope path are accepted on **read** as a legacy or import path, but caliban logs a `WARN` and recommends running `caliban settings import` to migrate.

When both `settings.toml` and `settings.json` exist in the same scope directory, TOML wins and the JSON file is ignored (with a `WARN`).

```admonish warning title="Do not hand-edit JSON if you also have TOML"
If caliban finds both `settings.toml` and `settings.json` in the same scope directory it will silently ignore the `.json` file. Keep one format per scope directory.
```

---

## Live reload and restart-required keys

Most settings changes take effect immediately via the file watcher (250 ms debounce). A subset of keys require a full restart:

- **Restart-required:** `model`, `fallback_model`, `mcp_servers.*`, `output_style`, `auto_compact_threshold`, `micro_compact_enabled`

When a restart-required key changes on disk while caliban is running, caliban logs a `WARN` and shows a "restart required" badge in the `/config` TUI overlay. The new value will be used the next time you launch `caliban`.

All other settings — permissions, hooks, `api_key_helper`, UI keys, `env`, `memory` knobs — are live-reloadable and take effect within one debounce cycle without restarting.
