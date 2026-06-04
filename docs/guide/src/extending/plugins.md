# Plugins

A plugin bundles related customizations — skills, hooks, sub-agent definitions, MCP server configs, and output styles — into a single installable directory. The plugin system (ADR 0030) is a thin orchestrator: it parses a `plugin.json` manifest, namespaces items, expands `${CALIBAN_PLUGIN_ROOT}`, and feeds everything into the same per-surface loaders that project and user files use.

## What a plugin contains

```text
my-plugin/
  plugin.json             # required manifest
  skills/
    my-workflow/
      SKILL.md
  hooks/
    hooks.json
  agents/
    reviewer.md
  output-styles/
    concise.md
  mcp/
    .mcp.json
  commands/
    recap.md
```

All subdirectories are optional. When a `components` entry is omitted from the manifest, the loader scans the conventional subdirectory automatically.

## The manifest (`plugin.json`)

```json
{
  "name": "my-plugin",
  "version": "1.0.0",
  "description": "Short description shown in /plugins and trust prompts",
  "author": "Alice <alice@example.com>",
  "license": "MIT",
  "homepage": "https://example.com/my-plugin",
  "components": {
    "skills": ["skills/my-workflow"],
    "hooks": "hooks/hooks.json",
    "agents": ["agents/reviewer.md"],
    "output_styles": "output-styles/concise.md",
    "mcp_servers": "mcp/.mcp.json",
    "commands": ["commands/recap.md"]
  },
  "caliban": {
    "min_version": "0.5.0",
    "platforms": ["macos", "linux"]
  }
}
```

| Field | Required | Description |
|---|---|---|
| `name` | Yes | Matches the directory name. Must be `[a-z0-9_-]{1,32}`. |
| `version` | Yes | Semver string. |
| `description` | No | One-line description. |
| `author` | No | Free-form author string. |
| `license` | No | SPDX identifier. |
| `homepage` | No | URL. |
| `components` | No | Paths to bundled files (string or array). |
| `caliban.min_version` | No | Skip when the running caliban is older. |
| `caliban.platforms` | No | Limit to `macos`, `linux`, or `windows`. |

For MCP servers bundled as inline config (matching Claude Code's `.mcp.json` shape), use the top-level `mcpServers` key instead of `components.mcp_servers`.

## Discovery roots

Caliban scans three roots at startup. A plugin with the same name in an earlier root replaces later ones — no manifest merging.

| Priority | Root | Scope |
|---|---|---|
| 1 (highest) | `<workspace>/.caliban/plugins/<name>/` | Project |
| 2 | `$XDG_DATA_HOME/caliban/plugins/<name>/` (user install dir) | User |
| 3 | `/etc/caliban/plugins/<name>/` (platform analogues) | Managed (org policy) |

Managed plugins ignore the `plugins.enabled` list — they run regardless of per-user configuration.

## Namespacing

Items loaded from a plugin carry a `<plugin>:<item>` prefix:

- Skills: `my-plugin:my-workflow`
- Output styles: `my-plugin:concise`
- MCP servers: `my-plugin:my-server`

This prevents collisions with bare-named items at the project or user level. Hooks merge additively across plugins.

## The `caliban plugin` command

```bash
# List all installed plugins and their status
caliban plugin list

# Show the manifest of an installed plugin as JSON
caliban plugin info <name>

# Install a plugin from a marketplace
caliban plugin install <name>@<marketplace-url>

# Install a plugin from a local directory
caliban plugin install --dir /path/to/my-plugin

# Update a plugin to the latest marketplace version
caliban plugin update <name>

# Remove a plugin
caliban plugin remove <name>

# Enable / disable a plugin (affects whether it loads at startup)
caliban plugin enable <name>
caliban plugin disable <name>
```

`caliban plugin help` prints the full reference.

## Marketplace trust

First-time marketplace installs display the manifest, its sha256 hash, and the install URL and prompt for acknowledgement. Acknowledged installs are recorded in `$XDG_DATA_HOME/caliban/trust/plugins.json`. Re-installs of the same manifest hash skip the prompt; version bumps re-prompt. Sideloads (local `--dir` installs) skip trust gating because the operator already has filesystem access.

## `${CALIBAN_PLUGIN_ROOT}` expansion

Inside plugin-bundled hook commands and MCP server configs, `${CALIBAN_PLUGIN_ROOT}` expands to the plugin's absolute root directory. `${CLAUDE_PLUGIN_ROOT}` is a supported alias so existing Claude Code plugins port verbatim.

```admonish note
The `--no-plugins` flag (or `CALIBAN_NO_PLUGINS=1`) disables plugin discovery entirely for a single run, treating all plugin roots as empty. This is useful for debugging or for CI environments that should not pick up locally installed plugins.
```

## Related pages

- [Skills](./skills.md)
- [Hooks](./hooks.md)
- [MCP Servers](./mcp.md)
- [Output Styles](./output-styles.md)
- [Slash Command Index](../reference/slash-index.md) — `/plugins` overlay
