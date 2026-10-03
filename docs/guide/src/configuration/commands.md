# Config Commands

Caliban ships two subcommand families for inspecting and managing settings: `caliban config` for the unified settings layer, and `caliban settings` for import/export of individual scope files. Both work without a running session.

## `caliban config print`

Prints the fully-merged effective settings as JSON, annotated with the scope each value came from.

```bash
caliban config print
```

The output is an envelope with four keys:

| Key | Contents |
|---|---|
| `settings` | the merged `Settings` object |
| `_sources` | each settings file that was loaded, with its scope, path, and format |
| `_provenance` | per top-level key, the scope that contributed the winning value |
| `_env_overrides` | each [environment-layer](./settings-layering.md#the-environment-layer) override that won over the file, naming both the settings key and the `CALIBAN_*` variable that set it |

This is the headless equivalent of the read-only `Effective` tab in the `/config` TUI overlay.

```admonish warning title="`config print` ignores `--settings` and `--setting-sources`"
The command loads the scopes that a normal run would discover on disk, but it
does **not** apply a `--settings` overlay or honor a `--setting-sources`
filter — passing either alongside `config print` changes nothing in the output.
To preview a CI overlay you currently have to start a session with it. (The
subcommand's own `--help` text still claims otherwise; the behavior above is
what the code does.)
```

## `caliban config migrate`

Consolidates legacy per-feature TOML files (`permissions.toml`, `mcp.toml`, `hooks.toml`) in the current workspace into a single project-scope `.caliban/settings.json`.

```admonish note title="migrate writes JSON"
Unlike the other settings writers, `config migrate` currently writes `settings.json`, not
TOML. Run `caliban settings import --from .caliban/settings.json` afterwards if you want the
canonical TOML form.
```

```bash
# Preview what would be written (nothing is changed)
caliban config migrate --dry-run

# Run the migration
caliban config migrate
```

After migration the per-feature files are no longer read (caliban checks for the unified key first). You can safely delete them, or leave them in place — caliban will ignore them once the corresponding key exists in the unified settings file.

```admonish tip title="When to migrate"
Run `caliban config migrate` once after upgrading to a version that shipped ADR 0026. It is safe to run multiple times — the command is idempotent.
```

## `caliban config import-router`

Migrates a legacy standalone `caliban.toml` router config into the project-scope
`.caliban/settings.toml` as a `[router]` section, relocating top-level
`[provider.X]` blocks to `[router.provider.X]` on the way. Router config is no
longer auto-discovered, so this is how an existing `caliban.toml` keeps working
([ADR 0060](../adr/0060-router-config-through-settings.md)).

```bash
# Preview the merge (nothing is written)
caliban config import-router --dry-run

# Migrate the nearest caliban.toml
caliban config import-router

# Migrate a specific file
caliban config import-router --from ./config/router.toml
```

`--from` defaults to the nearest `caliban.toml` found by walking up from the
current directory, and the command errors if there is none. Settings keys other
than `[router]` are preserved, and the result is validated before being written.

See [The Model Router](../providers/router.md) for the resulting section's shape.

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

Caliban's settings writers emit TOML (the one exception is `config migrate`, above). JSON files at any scope path are accepted on **read** as a legacy or import path, but caliban logs a `WARN` and recommends running `caliban settings import` to migrate.

When both `settings.toml` and `settings.json` exist in the same scope directory, TOML wins and the JSON file is ignored (with a `WARN`).

```admonish warning title="Do not hand-edit JSON if you also have TOML"
If caliban finds both `settings.toml` and `settings.json` in the same scope directory it will silently ignore the `.json` file. Keep one format per scope directory.
```

---

## Applying changed settings

Settings are read **once at startup**, so a change to any settings file takes
effect the next time you launch `caliban` — there is no live reload today. See
[Applying changed settings](./settings-layering.md#applying-changed-settings) for
why (the file watcher is scaffolded but not wired into the binary).
