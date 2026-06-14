# ADR 0030 · Plugin packaging

- **Status:** accepted
- **Date:** 2026-05-24
- **Author:** john.ford2002@gmail.com
- **Spec:** `docs/superpowers/specs/2026-05-24-plugin-system-design.md`
- **Depends on:** ADR for hooks-expansion (forthcoming alongside
  `specs/2026-05-24-hooks-expansion-design.md`)

## Context

Skills (ADR 0019), MCP servers (ADRs 0017 / 0023), sub-agents (ADR 0021),
and the forthcoming hooks-expansion + output-styles work each ship as
their own discovery surface. Operators who want to share a package of
related customizations — Claude Code's "plugin" model — currently have
to drop files into half a dozen directories by hand.

Claude Code unifies all five surfaces under a single plugin directory
with one `plugin.json` manifest; settings expose `enabledPlugins`,
marketplace allowlists, and `strictPluginOnlyCustomization`; the
`/plugins` slash command and `claude plugin` CLI manage install /
enable / disable / remove. This ADR records caliban's commitment to the
same shape.

## Decision

### A plugin is a directory with a `plugin.json` manifest

A plugin is `<plugin-name>/plugin.json` plus optional subdirectories
`skills/`, `hooks/`, `agents/`, `output-styles/`, `mcp/`, `commands/`.
The manifest declares `name` (matches directory), `version`,
`description`, `author`, `license`, optional `caliban.min_version` and
`caliban.platforms`, and a `components` object pointing at the bundled
files. JSON (not TOML) so the surface stays uniform with hooks and MCP
configs (also JSON in their canonical forms). Unknown manifest keys are
preserved through serde to leave room for forward-compat fields.

### Three discovery roots, project > user > managed

- Project: `<workspace>/.caliban/plugins/<name>/`
- User: `$XDG_DATA_HOME/caliban/plugins/<name>/`
- Managed: `/etc/caliban/plugins/<name>/` (Linux), platform analogues
  elsewhere. Managed plugins ignore `plugins.enabled` (policy-enforced).

A plugin with the same `name` in an earlier root replaces the later one
— no manifest merging.

### Items are namespaced: `<plugin>:<item>`

Skills, agents, and output styles loaded from a plugin carry the
`<plugin>:<item>` prefix. They cannot collide with bare-named items at
the user level (project-level bare items still shadow them). Hooks
merge additively across plugins. MCP servers are exposed under
`<plugin>:<server>` to avoid colliding with user-configured servers.

Collision priority is **project > plugin > user**. Strict project-only
operators get `strict_plugin_only_customization = true`, which ignores
bare-file customizations under `~/.caliban/skills/*` etc. entirely.

### `${CALIBAN_PLUGIN_ROOT}` expansion at the plugin boundary

Plugin-bundled MCP configs and hook commands need to reach binaries
inside the plugin without hardcoding install paths.
`caliban-plugins` expands `${CALIBAN_PLUGIN_ROOT}` to the plugin's
absolute root directory *before* passing config downstream.
`${CLAUDE_PLUGIN_ROOT}` is an honored alias so existing Claude Code
plugins port verbatim. Any other `${VAR}` is passed through to the
downstream consumer's own expansion (MCP client, hooks loader).

### Marketplaces are public JSON indices fetched on demand

A marketplace is one HTTP(S) URL serving a JSON index of plugins +
versions + tarball URLs + sha256 hashes. `caliban plugin install
<name>@<marketplace>` fetches the index, verifies the marketplace is
in `plugins.marketplaces.strict_known` and not in `blocked`, downloads
and extracts the tarball, and writes a trust record.

Signature verification is *out of scope for v1*. Trust is by source URL
+ manifest hash, surfaced in the install prompt. v2 may add cosign /
minisign.

### Trust gating on first install

Sideloads aren't gated (the operator already had filesystem access).
Marketplace installs prompt with `plugins.trust_message`, the manifest
contents, the manifest sha256, and the install URL. Acknowledged
installs are recorded in `$XDG_DATA_HOME/caliban/trust/plugins.json`;
re-installs of identical manifest hashes skip the prompt; version bumps
re-prompt.

### New crate: `caliban-plugins`

A thin orchestrator: it parses manifests, resolves namespaces, expands
`${CALIBAN_PLUGIN_ROOT}`, and hands paths + configs to the existing
loaders (skills, hooks, MCP, agents, output-styles). It does *not*
duplicate any per-surface logic. The `caliban` binary constructs one
`PluginManager` at startup and wires its outputs into the existing
loaders.

## Consequences

- **Positive:** Closes Matrix row B "Plugin packages" and the
  `/plugins` slash row in one initiative. Existing Claude Code plugins
  port with at most a directory rename (`${CLAUDE_PLUGIN_ROOT}` alias).
  Each downstream loader stays single-purpose — plugins are a
  composition concern, not a per-loader concern. Trust gating gives
  operators a real "I have read this" moment without locking sideloads
  behind ceremony.
- **Negative:** Adds a new crate and a new settings surface (`plugins.*`,
  `plugins.marketplaces.*`). Marketplace install adds three new
  dependencies (`tar`, `flate2`, `sha2`) and an HTTP fetch path
  separate from MCP's. Trust records create a small migration burden
  if we ever move the on-disk format (mitigated by versioning the
  file). The unified hooks taxonomy must land first; this ADR's
  hooks-merging behavior is a no-op until then.
- **Revisit if:** Operators demand signed plugins (move to v2 cosign /
  minisign verification). The bare-vs-namespaced collision rules surprise
  users in practice (consider an explicit per-plugin "alias to bare
  name" affordance). Hot-reload of plugin contents becomes a real need
  (today it requires restart).
