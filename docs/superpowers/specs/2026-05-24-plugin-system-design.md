# Plugin system — Design (extension packaging)

**Date:** 2026-05-24
**Status:** Proposed
**Author:** john.ford2002@gmail.com
**Sub-project of:** caliban Rust agent harness
**ADR:** `docs/adr/0030-plugin-packaging.md`
**Depends on:** `docs/superpowers/specs/2026-05-24-hooks-expansion-design.md`
(the unified hooks taxonomy must land first so plugin-bundled hooks have a
config schema to plug into).

## Goal

Let operators install a *plugin* — a single directory that bundles any
combination of skills, hooks, sub-agents, output-styles, and MCP servers —
and have caliban discover all five surfaces with one manifest. Plugins are
namespaced (`plugin-name:skill-name`, `plugin-name:agent-name`), namespaced
items shadow user-level items but lose to project-level overrides, and a
single `caliban plugin install <name>@<marketplace>` CLI brings new plugins
in from a remote JSON index. This is caliban's parity track for Claude Code's
plugins surface (`/plugin`, `claude plugin`, `--plugin-dir`, `--plugin-url`,
`enabledPlugins`, marketplaces).

## Non-goals

- **Signature verification.** Plugins are trusted by location, not signature.
  A first-install trust gate (covered below) is the v1 affordance; cosign /
  minisign verification is a v2 follow-up.
- **Hot-reload of plugin contents.** Edits inside an installed plugin
  require a restart (or `/plugins reload`). Adding/removing whole plugins
  via `enabled_plugins` is similarly restart-bounded; live editing is a
  v2 concern handled with the `ConfigChange` hook.
- **Private marketplaces with auth.** v1 supports public marketplace
  JSON only; authenticated indices (`Authorization: Bearer`) are a
  v2 follow-up.
- **Plugin dependency resolution.** A plugin cannot declare it depends
  on another plugin. Operators install the dependency graph manually.
- **Built-in plugin marketplace.** caliban ships *no* default marketplace
  URL; the operator configures one in settings.

## Architecture

```
caliban binary
  build_runtime()
    PluginManager::load(roots, settings) ──► Vec<LoadedPlugin>
    │
    ├── skills:  fold each plugin's skills/ root into SkillLoader's discovery list
    │             (namespaced: plugin items become "<plugin>:<skill>")
    ├── hooks:   each plugin's hooks/hooks.json is merged into the hooks config
    │             (with ${CALIBAN_PLUGIN_ROOT} expansion on command/args)
    ├── agents:  each plugin's agents/<name>.md is loaded into the AgentRegistry
    │             (namespaced: "<plugin>:<agent>")
    ├── styles:  each plugin's output-styles/<name>.md feeds the OutputStyleRegistry
    │             (see specs/2026-05-24-output-styles-design.md)
    └── mcp:     each plugin's plugin.json `mcpServers` block and/or
                 `.mcp.json` is merged into McpClientManager's config
                 (with ${CALIBAN_PLUGIN_ROOT} expansion)
       │
       ▼
caliban-plugins  (NEW crate)
  PluginManifest        ── serde model of plugin.json
  LoadedPlugin          ── { manifest, root_dir, namespace, components }
  PluginManager         ── discovery + load + namespacing
  Marketplace           ── JSON index parser
  MarketplaceClient     ── fetch/cache marketplace index over reqwest
  TrustStore            ── first-install acknowledgments file
  PluginCli             ── `caliban plugin {install,list,enable,disable,remove}` impl
```

`caliban-plugins` is a thin orchestrator: it does not embed skill, hook,
agent, style, or MCP logic itself. It returns *paths* and *namespaced
identifiers* to the existing loaders. Each downstream crate stays
ignorant of plugins beyond accepting an extra discovery root.

## Plugin directory layout

A plugin is a single directory rooted at the plugin name:

```
my-plugin/
├── plugin.json              # required: manifest (see schema below)
├── skills/
│   └── <skill-name>/SKILL.md
├── hooks/
│   └── hooks.json           # merged into the hooks config tree
├── agents/
│   └── <agent-name>.md      # sub-agent definitions, same format as ~/.caliban/agents/
├── output-styles/
│   └── <style-name>.md      # frontmatter + body, see output-styles spec
├── mcp/
│   └── .mcp.json            # OR inline `mcpServers` in plugin.json
├── commands/
│   └── <cmd-name>.md        # optional: legacy slash-command files
└── LICENSE                  # recommended; surfaced in /plugins details
```

None of the subdirectories are required individually; a plugin that only
ships skills omits `hooks/`, `agents/`, `output-styles/`, and `mcp/`.

## Manifest schema (`plugin.json`)

JSON is the format (matching Claude Code; rejected: TOML, YAML — we already
have YAML for skills, JSON keeps the plugin surface uniform with hooks and
MCP configs which are also JSON).

```json
{
  "$schema": "https://caliban.dev/schemas/plugin.v1.json",
  "name": "superpowers",
  "version": "1.4.2",
  "description": "Curated skills for code review, debugging, and TDD.",
  "author": "john.ford2002@gmail.com",
  "license": "MIT",
  "homepage": "https://github.com/example/superpowers",
  "components": {
    "skills":       ["skills/brainstorming", "skills/test-driven-development"],
    "hooks":        "hooks/hooks.json",
    "agents":       ["agents/reviewer.md"],
    "output_styles":["output-styles/learning.md"],
    "mcp_servers":  "mcp/.mcp.json",
    "commands":     ["commands/recap.md"]
  },
  "mcpServers": {
    "fixtures": {
      "command": "${CALIBAN_PLUGIN_ROOT}/bin/fixtures-server",
      "transport": "stdio"
    }
  },
  "caliban": {
    "min_version": "0.5.0",
    "platforms": ["macos", "linux"]
  }
}
```

### Field semantics

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | yes | Must match the parent directory name. Same `[a-z0-9_-]+` grammar as MCP server names. |
| `version` | semver string | yes | Used by marketplace install for upgrade detection. |
| `description` | string | yes | Surfaced in `/plugins` overlay and on first-install trust prompt. |
| `author` | string | recommended | Free-form; usually `Name <email>` or URL. |
| `license` | SPDX id | recommended | E.g. `MIT`, `Apache-2.0`. |
| `homepage` | URL | optional | Surfaced in `/plugins` details. |
| `components.skills` | array of paths | optional | Subdirectories of skills relative to plugin root. If omitted, all `skills/*/SKILL.md` discovered. |
| `components.hooks` | path | optional | Defaults to `hooks/hooks.json` if file exists. |
| `components.agents` | array of paths | optional | Defaults to all `agents/*.md`. |
| `components.output_styles` | array of paths | optional | Defaults to all `output-styles/*.md`. |
| `components.mcp_servers` | path | optional | Defaults to `mcp/.mcp.json`. Mutually exclusive with the inline `mcpServers` field at manifest top level (if both present, inline wins and a warning is logged). |
| `components.commands` | array of paths | optional | Legacy slash-command markdown files. |
| `mcpServers` | object | optional | Inline alternative to `components.mcp_servers`; same schema as `mcp.toml`'s `[server.*]` blocks (JSON form). |
| `caliban.min_version` | semver string | optional | If set and current caliban is older, plugin is skipped with a warning. |
| `caliban.platforms` | array of strings | optional | Filter: skip plugin on unlisted platforms. Accepts `macos`, `linux`, `windows`. |

Unknown top-level keys are preserved (parsed into a `serde_json::Map`) so
forward-compat fields don't fail load.

## Discovery roots

Three roots, walked in priority order:

1. **Project plugins:** `<workspace_root>/.caliban/plugins/<plugin-name>/`
2. **User plugins:** `$XDG_DATA_HOME/caliban/plugins/<plugin-name>/`
   (defaults to `~/.local/share/caliban/plugins/<plugin-name>/` on Linux,
   `~/Library/Application Support/caliban/plugins/<plugin-name>/` on macOS)
3. **Managed plugins:** `/etc/caliban/plugins/<plugin-name>/` (Linux),
   `/Library/Application Support/Caliban/plugins/<plugin-name>/` (macOS).
   Managed plugins cannot be disabled by `enabled_plugins` (they're
   policy-enforced).

A plugin with the same `name` in an earlier root *replaces* the later one
(no merging — manifests don't overlay). This mirrors how skills are
shadowed.

### Item-level collision rules

After all plugins load, the namespaced items are merged into the global
registries:

| Source | Skill name | Wins over |
|---|---|---|
| Project skill at `<ws>/.caliban/skills/foo` | `foo` | everything |
| Plugin skill from project plugin | `pluginA:foo` | user-level items |
| Plugin skill from user plugin | `pluginA:foo` | user-level items at same name |
| User skill at `~/.config/caliban/skills/foo` | `foo` | plugin items at the *bare* name |

In short: **project > plugin > user**. Plugin items always carry the
`<plugin>:<item>` prefix and cannot collide on bare names. Hooks merge
additively (multiple plugins can register `before_tool` handlers); agents
namespace-prefix like skills; output styles namespace-prefix unless a
plugin sets `force_for_plugin: true` (see the output-styles spec).

## Settings keys

```toml
# ~/.config/caliban/settings.toml or .caliban/settings.toml

[plugins]
# Enable plugins by name; defaults to empty (no plugins enabled even
# if installed). This matches Claude Code's `enabledPlugins`.
enabled = ["superpowers", "code-review-pro"]

# When true, only customizations (skills, hooks, agents, styles)
# delivered via plugins are honored — bare-file customizations under
# ~/.caliban/skills/* etc. are ignored. Matches Claude Code's
# `strictPluginOnlyCustomization`.
strict_plugin_only_customization = false

# Shown once on first install of each plugin (covered in Trust gating).
trust_message = "Plugins are arbitrary code. Only install from sources you trust."

[plugins.marketplaces]
# Strictly the listed marketplaces may be queried via `caliban plugin install`.
# Empty list disables marketplace installs entirely (sideload only).
strict_known = ["https://plugins.caliban.dev/index.json"]

# Marketplace URLs that are explicitly blocked, even if added later.
blocked = []
```

The settings file format (TOML vs JSON) is the broader settings-hierarchy
work; this spec assumes the keys above land in whichever shape that work
chooses, and shows TOML for readability.

## Trust gating on first install

Sideloading (copying a plugin directory in by hand) is not gated — the
operator already had filesystem access. Marketplace installs *are* gated:

1. `caliban plugin install foo@<marketplace>` fetches the marketplace
   index, verifies `<marketplace>` is in `plugins.marketplaces.strict_known`
   and not in `blocked`.
2. The CLI prints `plugins.trust_message`, the plugin's `name`, `version`,
   `description`, `author`, `license`, and the manifest hash; prompts for
   `[y/N]`.
3. On `y`, the plugin tarball is downloaded, extracted under
   `$XDG_DATA_HOME/caliban/plugins/<name>/`, and a record is appended to
   `$XDG_DATA_HOME/caliban/trust/plugins.json`:

   ```json
   {
     "superpowers": {
       "version": "1.4.2",
       "marketplace": "https://plugins.caliban.dev/index.json",
       "manifest_sha256": "ab12…",
       "installed_at": "2026-05-24T12:00:00Z"
     }
   }
   ```

4. Subsequent installs of the *same* `name` from the *same* marketplace
   with the *same* manifest hash skip the prompt. Version bumps reprompt
   with the diff in description.

Signature verification (cosign / minisign) is out of scope for v1; the
trust file leaves room for `signature` / `pubkey_id` fields for v2.

## `${CALIBAN_PLUGIN_ROOT}` expansion

Inside a plugin, paths to bundled binaries and scripts need to resolve at
runtime regardless of where the plugin lives. `${CALIBAN_PLUGIN_ROOT}` is
expanded by caliban *before* the value reaches the consumer:

| Context | Expansion site |
|---|---|
| `plugin.json` → `mcpServers.*.command` | `caliban-plugins` (before passing to `caliban-mcp-client`) |
| `plugin.json` → `mcpServers.*.args[*]` | same |
| `mcp/.mcp.json` | same |
| `hooks/hooks.json` → `handlers[*].command` / `args[*]` | same (before passing to hooks loader) |
| `agents/<name>.md` frontmatter `command` (rare) | `caliban-plugins` |

The expansion happens once at load time; downstream loaders see absolute
paths. `${CLAUDE_PLUGIN_ROOT}` is honored as an alias for parity with
existing Claude Code plugins. Any other `${VAR}` is passed through to the
downstream loader (which has its own env-var expansion rules).

## Marketplace concept

A marketplace is a single HTTP(S) endpoint serving a JSON index:

```json
{
  "$schema": "https://caliban.dev/schemas/marketplace.v1.json",
  "name": "Caliban Plugins",
  "url":  "https://plugins.caliban.dev/index.json",
  "plugins": [
    {
      "name": "superpowers",
      "description": "Curated skills for code review, debugging, TDD.",
      "versions": [
        {
          "version": "1.4.2",
          "tarball": "https://plugins.caliban.dev/superpowers/1.4.2.tar.gz",
          "sha256": "ab12…",
          "min_caliban": "0.5.0"
        }
      ]
    }
  ]
}
```

The index is fetched on `caliban plugin install` / `caliban plugin list
--remote` and cached at `$XDG_CACHE_HOME/caliban/marketplaces/<host>.json`
for one hour (configurable). The cache is invalidated on `caliban plugin
update`.

## `/plugins` slash command

```
┌─ Plugins ─────────────────────────────────────────────────────────────┐
│ ● superpowers         1.4.2  user      6 skills · 2 hooks · 1 style   │
│ ● code-review-pro     0.3.0  project   1 agent · 2 hooks              │
│ ○ flaky-tools         2.0.1  user      DISABLED                       │
│ ○ ancient             0.1.0  user      manifest invalid: missing name │
└───────────────────────────────────────────────────────────────────────┘
[esc] close   [enter] details   [e] enable   [d] disable
[i] install (marketplace)   [r] remove
```

Keys:

- `enter` opens a details pane showing manifest, components, manifest hash,
  install source (sideload / marketplace URL).
- `e` adds to `plugins.enabled`; `d` removes. Both write to the appropriate
  settings file (user or project depending on Shift modifier).
- `i` prompts for `<name>@<marketplace>` and runs the install flow inline.
- `r` removes the plugin from disk (with confirmation) and clears its
  trust record.

## CLI surface

```
caliban plugin install <name>[@<marketplace>]   # install from marketplace
caliban plugin install --dir <path>             # sideload a local directory
caliban plugin install --url <tarball-url>      # sideload a remote tarball
caliban plugin list [--remote <marketplace>]    # list installed (or remote)
caliban plugin enable  <name>                   # add to plugins.enabled
caliban plugin disable <name>                   # remove from plugins.enabled
caliban plugin remove  <name>                   # delete from disk + trust
caliban plugin info    <name>                   # show manifest details
caliban plugin update  [<name>]                 # refresh marketplace, upgrade
```

`--scope project|user` (default `user`) controls where `enable`/`disable`/
`install --dir` writes. `--yes` skips the trust prompt (useful for CI;
emits a warning to stderr).

## Public API sketches

```rust
// caliban-plugins/src/lib.rs

pub use manager::{PluginManager, PluginRoots};
pub use manifest::{PluginManifest, ComponentSpec};
pub use loaded::{LoadedPlugin, NamespacedItem};
pub use marketplace::{Marketplace, MarketplaceClient, MarketplaceEntry};
pub use trust::TrustStore;
pub use error::PluginError;

// caliban-plugins/src/manager.rs

pub struct PluginManager {
    plugins: Vec<LoadedPlugin>,
}

impl PluginManager {
    /// Walks the three discovery roots, loads each plugin.json, validates
    /// `caliban.min_version` and `caliban.platforms`, filters by
    /// `settings.plugins.enabled` (unless managed), and returns a manager.
    pub fn load(roots: &PluginRoots, settings: &PluginSettings) -> Result<Self, PluginError>;

    pub fn loaded(&self) -> &[LoadedPlugin];

    /// Returns the union of skill discovery roots contributed by all enabled plugins.
    pub fn skill_roots(&self) -> Vec<PathBuf>;
    pub fn agent_roots(&self) -> Vec<PathBuf>;
    pub fn output_style_roots(&self) -> Vec<PathBuf>;

    /// Returns merged hook configs with ${CALIBAN_PLUGIN_ROOT} expanded.
    pub fn hooks_config(&self) -> serde_json::Value;

    /// Returns merged MCP server configs.
    pub fn mcp_servers(&self) -> Vec<(String /*namespaced name*/, McpServerConfig)>;
}

// caliban-plugins/src/loaded.rs

pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub root_dir: PathBuf,
    pub namespace: String,           // == manifest.name
    pub source: PluginSource,        // Project | User | Managed
}

pub enum PluginSource { Project, User, Managed }

// caliban-plugins/src/marketplace.rs

pub struct MarketplaceClient { /* reqwest::Client, cache dir */ }

impl MarketplaceClient {
    pub async fn fetch_index(&self, url: &Url) -> Result<Marketplace, PluginError>;
    pub async fn install(
        &self,
        plugin: &str,
        marketplace: &Url,
        dest_root: &Path,
        trust: &mut TrustStore,
    ) -> Result<LoadedPlugin, PluginError>;
}
```

## Crate structure (delta)

```
crates/caliban-plugins/                # NEW
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── manifest.rs        # PluginManifest + serde
    ├── loaded.rs          # LoadedPlugin, NamespacedItem, PluginSource
    ├── manager.rs         # discovery + load + namespacing
    ├── marketplace.rs     # Marketplace, MarketplaceClient
    ├── trust.rs           # TrustStore (plugins.json read/write)
    ├── expand.rs          # ${CALIBAN_PLUGIN_ROOT} + ${CLAUDE_PLUGIN_ROOT} aliases
    ├── cli.rs             # `caliban plugin {…}` subcommand impl
    └── error.rs           # PluginError

crates/caliban-skills/src/loader.rs    # accept extra namespaced roots
crates/caliban-mcp-client/src/manager.rs  # accept extra (name, config) tuples
caliban/src/main.rs                    # construct PluginManager early; pass roots
caliban/src/tui_overlay_plugins.rs     # NEW: /plugins overlay
```

Dependencies (new): `tar`, `flate2`, `sha2`, `semver`, `reqwest` (workspace).

## Tests

1. **`manifest_parses_minimal_manifest`** — only `name`, `version`,
   `description` set; defaults filled in.
2. **`manifest_rejects_name_mismatching_directory`** — `name: foo` in
   `bar/plugin.json` errors.
3. **`manifest_rejects_invalid_semver_in_min_version`** —
   `caliban.min_version: "not-a-version"` errors.
4. **`manager_skips_plugins_below_min_version`** — emits a warning, plugin
   not in `loaded()`.
5. **`manager_skips_plugins_for_other_platforms`** — `platforms:
   ["windows"]` on a macos test host is skipped.
6. **`manager_filters_by_enabled_setting`** — installed plugin not in
   `enabled` does not load.
7. **`manager_loads_managed_plugin_even_if_not_enabled`** — managed root
   ignores `enabled` filter.
8. **`skill_roots_returns_plugin_skill_dirs_with_namespace_prefix`** —
   namespaced skill names propagate to the skill loader.
9. **`hooks_config_expands_caliban_plugin_root`** —
   `${CALIBAN_PLUGIN_ROOT}/bin/foo` resolves to the absolute plugin path.
10. **`hooks_config_expands_claude_plugin_root_alias`** — alias works for
    ported plugins.
11. **`mcp_servers_merge_inline_and_external`** — inline `mcpServers` and
    external `.mcp.json` both contribute; inline wins on collision.
12. **`marketplace_index_parses_minimal_entry`** —
    `name`/`description`/`versions[0]` round-trip.
13. **`marketplace_install_rejects_unknown_marketplace`** — when
    `strict_known` is set, install from an unlisted URL errors.
14. **`marketplace_install_writes_trust_record`** — after install, the
    `plugins.json` trust record has matching sha256 and version.
15. **`marketplace_install_skips_prompt_on_reinstall_same_hash`** —
    re-install of identical manifest hash doesn't re-prompt.
16. **`marketplace_install_reprompts_on_version_bump`** — different version
    triggers re-prompt.
17. **`trust_store_persists_to_disk`** — round-trip read/write of
    `plugins.json`.
18. **`cli_install_dir_sideload`** — `caliban plugin install --dir <path>`
    copies into the user root and writes the trust record with
    `marketplace: "sideload"`.
19. **`cli_disable_writes_settings_file`** — `caliban plugin disable foo`
    removes `foo` from `plugins.enabled` in the project settings file.
20. **`overlay_renders_invalid_manifest_row`** — a plugin with a missing
    `name` field shows up in `/plugins` with the parse error.

## Risks

- **Manifest schema churn.** v1's `plugin.json` lacks fields Claude Code may
  add (e.g. `requiredPermissions`, `statusLine`). Mitigation: unknown
  fields are preserved as a `serde_json::Map`; we can ship validators
  later without breaking old plugins.
- **Trust gate UX.** A wall-of-text first-install prompt trains operators
  to mash `y`. Mitigation: bold the unique manifest hash, include the
  install URL, log every accept to `~/.local/state/caliban/audit.log`.
- **Hooks merge ordering.** Multiple plugins registering `before_tool`
  handlers run in plugin-load order, which is alphabetical. Operators
  may expect priority. Mitigation: document the rule; add a per-plugin
  `priority` field in v2 if requested.
- **Marketplace MITM.** Sha256 in the index is only as trustworthy as the
  TLS channel and the marketplace operator. v1 documents this; v2 adds
  signature verification.
- **Plugin removes their MCP server while it's connected.** `/plugins
  remove` must shut down the MCP server before unlinking the binary.
  Covered in `caliban-mcp-client::McpClientManager::shutdown_server`.
- **Cross-spec dependency on hooks.** Plugin-bundled hooks integrate via
  the unified hooks taxonomy from
  `docs/superpowers/specs/2026-05-24-hooks-expansion-design.md`. If that
  spec slips, this spec ships with hooks merging as a no-op (manifest
  loads, hooks config is built but discarded) and ticks the parity row
  back to 🟡.

## Acceptance criteria

- `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D
  warnings`, and `cargo fmt --all -- --check` clean.
- ≥20 tests passing in `caliban-plugins`, plus 3 end-to-end fixtures
  (sideloaded plugin with a skill + hook + agent loads end-to-end).
- caliban binary discovers plugins from all three roots and respects
  `plugins.enabled`.
- `/plugins` overlay renders the loaded plugin list and supports
  enable/disable inline.
- `caliban plugin {install,list,enable,disable,remove,info}` all
  implemented; `caliban plugin install --dir` round-trips through trust
  store.
- Matrix B "Plugin packages" row 🔴 → ✅. Matrix M rows for `/plugin` /
  `/plugins` 🔴 → ✅.
- ADR 0030 in `accepted` status.
