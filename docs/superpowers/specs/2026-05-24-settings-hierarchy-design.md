# Layered settings + `/config` editor — Design

**Date:** 2026-05-24
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** `adrs/0026-settings-layering.md`

## Goal

Replace caliban's ad-hoc per-feature TOML files (`permissions.toml`,
`mcp.toml`, the eventual `hooks.toml`) with a single layered
configuration surface: `settings.json` at four canonical scopes
(managed > user > project > local) with documented merge semantics,
JSON-Schema validation, live reload via file-watcher, an interactive
`/config` TUI editor, and dynamic auth via `apiKeyHelper`. JSON is the
primary format for parity with Claude Code's schema; TOML is honored
at the same paths (`.toml` extension) for users who prefer it.

Existing per-crate config (`mcp.toml`, `permissions.toml`,
`hooks.toml`) continues to load *if the unified `settings.json` does
not define the matching top-level key*. The two coexist for a
deprecation window; new features only land in the unified format.

## Non-goals

- **Managed-settings delivery channels.** Reading Windows registry
  (`HKLM\SOFTWARE\Policies\Caliban`) and macOS plists is a sub-project;
  v1 supports file-based managed settings only
  (`/etc/caliban/managed-settings.json` and `managed-settings.d/`).
- **Cloud-pushed settings.** Claude.ai admin-pushed config is
  Anthropic-specific; we don't try to mirror it.
- **`apiKeyHelper` for non-API-key auth.** OAuth tokens (MCP) have
  their own keyring story (ADR 0023). `apiKeyHelper` is scoped to
  provider API keys.
- **Per-key live-reload semantics.** We document which keys reload
  cleanly and which require restart; we don't attempt to hot-swap
  things like the model router's provider list mid-turn.
- **Settings UI in a web browser.** `/config` is a TUI editor only.
- **Schema-driven autocomplete in a real editor (LSP).** The schema is
  published at `https://caliban.dev/schemas/settings.json` for editors
  to consume, but we don't ship an LSP.

## Architecture

```
                    ┌─────────────────────────────────────┐
                    │ ScopeLoader  (one per scope)        │
                    │   - canonical paths (OS-aware)      │
                    │   - both .json and .toml accepted   │
                    │   - returns Option<Settings>        │
                    └─────────────────────────────────────┘
                                    │
              ┌────────┬────────────┼────────────┬────────┐
              ▼        ▼            ▼            ▼        ▼
          Managed     User       Project       Local    CLI
        (org policy) (~/...)   (./.caliban) (.local)  (--settings)
              │        │            │            │        │
              └────────┴──────┬─────┴────────────┴────────┘
                              ▼
                       SettingsMerger
                  (scalars: highest-wins; arrays: documented)
                              ▼
                       Settings (Arc, ArcSwap-able)
                              ▼
              ┌───────────────┼────────────────┐
              ▼               ▼                ▼
          AgentBuilder    HookRouter      McpClientManager
          PermissionsHook ModelRouter     ApiKeyHelper
              ▲               ▲                ▲
              │               │                │
              └─────  notify(ConfigChange)  ────┘
                              ▲
                       SettingsWatcher
                  (notify crate; debounce 250ms)
```

Five building blocks:

1. **ScopeLoader** — reads canonical paths for one scope, supports
   both JSON and TOML, returns a parsed `Settings` value (or None).
2. **SettingsMerger** — merges the scope chain into a single
   `Settings` per the documented rules (scalars: highest-wins;
   arrays: see "Merge rules" below).
3. **Settings** — strongly-typed Rust struct (serde-derived) wrapped
   in `Arc<ArcSwap<Settings>>` so live-reload swaps atomically.
4. **SettingsWatcher** — `notify` filesystem watcher on each scope
   path; debounces 250 ms; re-runs the loader; emits a `ConfigChange`
   hook event (ADR 0024) with the diff.
5. **`/config` editor** — TUI overlay with one tab per top-level key
   group; edits write to the *project* scope by default (configurable).

## Crate structure (deltas only)

```
crates/caliban-core/
├── src/
│   ├── settings/                  # NEW module — root of the typed config
│   │   ├── mod.rs                 # Settings struct (#[derive(Deserialize, JsonSchema)])
│   │   ├── scope.rs               # Scope enum + canonical paths
│   │   ├── loader.rs              # ScopeLoader; json+toml dispatch
│   │   ├── merge.rs               # SettingsMerger; per-key merge rules
│   │   ├── watcher.rs             # SettingsWatcher (notify)
│   │   ├── schema.rs              # JSON-Schema generation + emission
│   │   ├── api_key_helper.rs      # apiKeyHelper invocation + caching
│   │   └── compat.rs              # legacy mcp.toml / permissions.toml fallback
│   └── lib.rs                     # re-export Settings + Scope
crates/caliban-agent-core/
├── src/
│   └── hooks_router/handler.rs    # ConfigChange handler dispatch
caliban/
├── src/
│   ├── tui/                       # /config overlay
│   │   └── config_overlay.rs      # NEW
│   └── main.rs                    # --settings / --setting-sources flags
```

New workspace deps:

```toml
notify         = "6"             # filesystem watcher (debounced)
arc-swap       = "1"             # atomic Arc<Settings> swap
schemars       = "0.8"           # JSON Schema generation
jsonschema     = { workspace = true }  # already in workspace
serde_json     = { workspace = true }
toml           = { workspace = true }
```

## Canonical paths (per OS)

| Scope     | Linux                                          | macOS                                                          | Windows                                                           |
|-----------|------------------------------------------------|----------------------------------------------------------------|-------------------------------------------------------------------|
| Managed   | `/etc/caliban/managed-settings.json` + `managed-settings.d/` | `/Library/Application Support/Caliban/managed-settings.json` + `managed-settings.d/` | `C:\ProgramData\Caliban\managed-settings.json` + `managed-settings.d\` |
| User      | `$XDG_CONFIG_HOME/caliban/settings.json` (default `~/.config/caliban/settings.json`) | `~/Library/Application Support/Caliban/settings.json` (also reads `~/.config/caliban/settings.json` for XDG fans) | `%APPDATA%\Caliban\settings.json` |
| Project   | `<cwd>/.caliban/settings.json`                 | same                                                           | same                                                              |
| Local     | `<cwd>/.caliban/settings.local.json`           | same                                                           | same                                                              |

Each path also accepts `.toml` (`settings.toml`,
`settings.local.toml`). If both exist in the same scope, `.json` wins
and a WARN is logged.

CLI overrides:

- `--settings <FILE>` (path) **or** `--settings '<INLINE-JSON>'`
  injects a virtual scope **above local** (below CLI).
- `--setting-sources <CSV>` restricts which scopes are read.
  E.g. `--setting-sources user,project` skips local and managed. Used
  by CI to pin a known-good base.

## Schema (top-level keys)

A pragmatic subset of Claude Code's documented keys plus caliban-
specific ones. The full Rust struct is `Settings` in
`crates/caliban-core/src/settings/mod.rs`; key groups:

```rust
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    // model / agent
    pub agent:        Option<String>,
    pub model:        Option<ModelSelector>,    // { provider, name } OR plain string
    pub available_models: Vec<ModelEntry>,
    pub effort_level: Option<EffortLevel>,
    pub fallback_model: Option<ModelSelector>,

    // permissions (replaces permissions.toml)
    pub permissions: Permissions,
    pub disable_bypass_permissions_mode: bool,

    // hooks (replaces hooks.toml; see ADR 0024)
    pub hooks: HooksConfig,
    pub disable_all_hooks: bool,
    pub allow_managed_hooks_only: bool,
    pub allowed_http_hook_urls: Vec<String>,
    pub http_hook_allowed_env_vars: Vec<String>,

    // MCP (replaces mcp.toml; see ADR 0023)
    pub mcp: McpConfig,
    pub allowed_mcp_servers: Vec<String>,
    pub denied_mcp_servers: Vec<String>,
    pub allow_managed_mcp_servers_only: bool,

    // memory (see ADR 0018)
    pub auto_memory_enabled: Option<bool>,
    pub auto_memory_directory: Option<PathBuf>,
    pub claude_md: Option<PathBuf>,             // managed-only
    pub claude_md_excludes: Vec<String>,

    // auth
    pub api_key_helper: Option<ApiKeyHelperSpec>,

    // UI / UX
    pub editor_mode: Option<EditorMode>,
    pub tui_mode:    Option<TuiMode>,
    pub language:    Option<String>,

    // observability
    pub feedback_survey_rate: Option<f32>,
    pub status_line: Option<StatusLineSpec>,

    // headless
    pub no_session_persistence: bool,
    pub cleanup_period_days: Option<u32>,

    // misc
    pub env: BTreeMap<String, String>,
    pub additional_directories: Vec<PathBuf>,

    // escape hatch — passthrough for forward-compat
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
```

`Permissions`, `HooksConfig`, `McpConfig` are the same structs the
existing per-crate loaders produce — exactly so that we can fall back
to `permissions.toml`/`mcp.toml`/`hooks.toml` when the unified
`settings.json` doesn't define the top-level key.

JSON Schema is generated at build time from the `JsonSchema` derives
and emitted to `target/schemas/caliban-settings.json`. CI uploads it
to `https://caliban.dev/schemas/settings.json` so editors (VS Code,
JetBrains) can autocomplete via the `$schema` URL.

`deny_unknown_fields` is on by default; a top-level `"extra": {…}`
table is the escape hatch for forward-compat. Unknown nested fields
under known top-level keys fail loudly.

## Merge rules

Highest-priority scope wins for scalars; arrays merge per-key per the
table below. Order from highest to lowest priority: **CLI** >
**Local** > **Project** > **User** > **Managed-with-precedence-flag** >
**Managed** (default).

Why managed isn't unconditionally top: org policy frequently wants to
*ban* something a user might unset; but in practice operators want to
*augment* defaults, not lock everything. So managed sits at the
bottom **unless** it sets `parentSettingsBehavior: "block"` (then it
moves to the top, overriding everything). Mirrors Claude Code's
documented escape hatch.

```
priority: CLI > Local > Project > User > Managed*
* Managed jumps to the top when parentSettingsBehavior = "block"
```

| Key                                | Merge rule                                          |
|------------------------------------|-----------------------------------------------------|
| `permissions.allow` / `ask` / `deny` | **concatenate** in priority order (highest first; first-match-wins) |
| `hooks.<Event>` arrays             | **concatenate** in priority order; managed handlers tagged for `allow_managed_hooks_only` |
| `allowed_http_hook_urls`           | **concatenate**, then dedupe                        |
| `http_hook_allowed_env_vars`       | **concatenate**, then dedupe                        |
| `allowed_mcp_servers`              | **concatenate**, then dedupe                        |
| `denied_mcp_servers`               | **concatenate**, then dedupe                        |
| `mcp.servers.<name>`               | **deep-merge** within a name; full override if `disabled = true` at higher scope |
| `available_models`                 | **concatenate**, dedupe by `{provider, name}`       |
| `env`                              | **deep-merge** key-by-key; highest-wins per key     |
| `additional_directories`           | **concatenate**, dedupe                             |
| `claude_md_excludes`               | **concatenate**, dedupe                             |
| every other scalar                 | **highest-wins**                                    |

Operators can inspect the effective merge with `caliban config print`
or the `/config` overlay's "Effective" tab.

## Live reload

`SettingsWatcher` watches every scope path and the parent directory
(so file-creation events fire). On change:

1. Debounce 250 ms.
2. Re-run the loader for the affected scope.
3. Re-merge; build the new `Settings`.
4. Compute a diff of changed keys.
5. `arc_swap.store(new_settings)`.
6. Fire `ConfigChange { changed_keys, new_settings_summary }` via the
   hook router.
7. Components subscribe to a `tokio::sync::watch::Receiver<Arc<Settings>>`
   and refresh their internal state.

Live-reloadable keys (no restart required):

- `permissions.*`, `hooks.*`, `disable_all_hooks`,
  `allow_managed_hooks_only`, `allowed_http_hook_urls`,
  `http_hook_allowed_env_vars`
- `api_key_helper.*` (next call refreshes the token)
- `additional_directories`, `claude_md_excludes`
- UI keys: `editor_mode`, `tui_mode`, `language`, `status_line`
- `env` (re-applied to *new* subprocesses; existing children unaffected)
- `feedback_survey_rate`, `cleanup_period_days`

Restart-required keys (logged WARN on change, applied on next launch):

- `model`, `fallback_model`, `available_models`
- `agent`
- `mcp.servers.*` (changing transports / commands mid-flight is
  surgically hostile; users can re-spawn via `/mcp` overlay's `[r]`
  per-server)
- `auto_memory_enabled`, `auto_memory_directory`

A `ConfigChange` hook handler (e.g. a webhook to a monitoring
endpoint) can be configured to alert operators that a restart-only key
changed.

## `apiKeyHelper`

A dynamic provider-API-key supplier. The setting is either a shell
command string or a path to a script:

```json
{
  "apiKeyHelper": {
    "command": "/usr/local/bin/get-anthropic-key.sh",
    "provider": "anthropic",
    "refreshIntervalMs": 300000,
    "ttlMs": 3600000
  }
}
```

Contract:

1. caliban runs the command (no shell; argv-style) with env vars
   `CALIBAN_PROVIDER=anthropic`, `CALIBAN_SESSION_ID=…`,
   `CALIBAN_API_KEY_HELPER_TTL_MS=3600000`.
2. The command writes the API key to **stdout** (no trailing newline
   required) and exits 0. Anything on stderr is logged WARN.
3. Caliban caches the key in memory for `refreshIntervalMs` (default
   300 s) or until a provider call returns 401, whichever comes
   first.
4. Refresh is inline (blocks the next provider call) up to a
   `slowHelperWarningMs` deadline (default 10 s); WARN is logged
   when the helper takes longer.

Per-provider keys are supported — either a single `apiKeyHelper` with
`provider: "*"` (used as fallback), or an array of helpers keyed by
provider:

```json
{
  "apiKeyHelper": [
    { "provider": "anthropic", "command": "..." },
    { "provider": "openai",    "command": "..." }
  ]
}
```

The auth precedence chain (per provider):

1. `apiKeyHelper` for this provider (if configured)
2. `apiKeyHelper` with `provider: "*"`
3. Environment variable (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc.)
4. Per-provider keyring entry (caliban-provider crates)
5. Anonymous (local providers like Ollama)

## `/config` slash command

TUI overlay opens a tabbed editor:

```
┌─ /config ─────────────────────────────────────────────────────────────┐
│ [Model] [Permissions] [Hooks] [MCP] [Memory] [UI] [Auth] [Effective] │
├──────────────────────────────────────────────────────────────────────┤
│ model.provider     anthropic    [project]                            │
│ model.name         claude-sonnet-4-7  [user]                         │
│ fallback_model     claude-haiku-4-7   [project]                      │
│ effort_level       medium        [user]                              │
│ available_models                 [user]                              │
│   - anthropic/claude-sonnet-4-7                                       │
│   - anthropic/claude-haiku-4-7                                        │
│   - openai/gpt-5                                                      │
├──────────────────────────────────────────────────────────────────────┤
│ [esc] close   [tab] next pane   [enter] edit   [s] scope  [w] write  │
└──────────────────────────────────────────────────────────────────────┘
```

Per-row:

- A small `[scope]` chip shows which scope contributed the effective
  value.
- `Enter` opens an inline editor for the row's value (typed against the
  schema; invalid input rejected before save).
- `s` cycles the write-scope (default: project).
- `w` flushes pending edits to disk.

The **Effective** tab is read-only and shows the fully-merged config
JSON with per-key scope annotations — exactly what
`caliban config print` emits.

Edits use atomic file replace: write to `settings.json.tmp`, fsync,
rename. The file-watcher fires `ConfigChange` after the rename so the
running process picks up its own edits without extra plumbing.

## Public API sketches

```rust
// crates/caliban-core/src/settings/mod.rs

#[derive(Clone)]
pub struct SettingsHandle(Arc<ArcSwap<Settings>>);

impl SettingsHandle {
    pub fn load(opts: LoadOptions) -> Result<Self> { … }
    pub fn current(&self) -> Arc<Settings> { self.0.load_full() }
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<Arc<Settings>>;
    pub fn watch_paths(&self) -> Vec<PathBuf>;
}

pub struct LoadOptions {
    pub cwd: PathBuf,
    pub setting_sources: Option<Vec<Scope>>,   // None = all
    pub cli_overlay: Option<Settings>,         // --settings <FILE|JSON>
    pub schema_validate: bool,                  // default true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Managed,
    User,
    Project,
    Local,
    Cli,
}
```

```rust
// crates/caliban-core/src/settings/api_key_helper.rs

pub struct ApiKeyHelperPool { … }

impl ApiKeyHelperPool {
    pub fn from_settings(s: &Settings) -> Self;
    pub async fn key_for(&self, provider: &str) -> Result<SecretString>;
    pub async fn invalidate(&self, provider: &str);  // on 401
}
```

## Compatibility / migration

For one minor release after this lands, legacy per-feature files keep
working when the unified `settings.json` doesn't define the matching
top-level key:

- `permissions.toml` → loaded only if `settings.json.permissions` is
  unset
- `mcp.toml`         → loaded only if `settings.json.mcp` is unset
- `hooks.toml`       → loaded only if `settings.json.hooks` is unset

A `caliban config migrate` subcommand reads the legacy files, emits a
unified `settings.json` (and deletes the legacy files iff `--delete`),
and prints the diff. After one minor release, the legacy loaders log
DEPRECATED but continue to work; after two minor releases they error.

## Testing strategy

~18 enumerated tests across `caliban-core` unit + integration tests:

1. **ScopeLoader reads JSON from each scope's canonical path.** OS-aware fixture paths.
2. **ScopeLoader reads TOML as fallback.** Settings round-trip.
3. **`.json` wins over `.toml` in the same scope; WARN logged.**
4. **Merge: scalar highest-wins.** `model` set at user and project; project wins.
5. **Merge: `permissions.allow` concatenates in priority order.**
6. **Merge: `mcp.servers.<name>` deep-merges.** User defines stdio command; project overrides `env`.
7. **Merge: managed with `parentSettingsBehavior=block` overrides everything.**
8. **`--settings <FILE>` injects a virtual scope above local.**
9. **`--settings '<INLINE-JSON>'` parses inline JSON.**
10. **`--setting-sources user,project` skips managed and local.**
11. **Schema validation rejects unknown top-level keys (with `deny_unknown_fields`).**
12. **`extra` passthrough captures forward-compat keys without erroring.**
13. **SettingsWatcher fires on file change, debounced 250 ms.**
14. **`ConfigChange` hook fires with the right diff payload.**
15. **Restart-required key change logs WARN and is *not* applied to the live config.**
16. **`apiKeyHelper` invocation caches; second call within `refreshIntervalMs` hits cache.**
17. **`apiKeyHelper` 401 path invalidates cache; next call re-invokes.**
18. **`apiKeyHelper` slow-helper warning fires at `slowHelperWarningMs`.**
19. **Compat loader: legacy `permissions.toml` consumed when unified `settings.json.permissions` is unset.**
20. **`caliban config migrate` round-trips the three legacy files into a single `settings.json` byte-for-byte stable on re-emit.**

Plus 2 TUI snapshot tests for the `/config` overlay (Model tab and
Effective tab).

## Risks

- **Settings sprawl.** ~30 top-level keys is a lot of surface. We
  trade for it because the parity matrix lists ~80 Claude Code keys
  and most operators expect a single `settings.json` to govern
  behavior. Mitigation: documented schema; `/config` editor as
  primary UX; `deny_unknown_fields` keeps typos loud.
- **Live-reload races.** Two processes (e.g. CLI and TUI both running)
  can stomp each other. Mitigation: atomic write + read; `mtime`
  check before write; conflict surfaces in `/config` overlay with a
  "settings changed on disk; reload?" prompt.
- **Merge complexity.** The 8-row merge-rule table is hard to keep
  straight. Mitigation: unit tests pin every documented rule;
  `/config Effective` tab annotates which scope contributed each
  value.
- **`apiKeyHelper` is shell-out.** A user-scope helper running with
  user privileges is fine; managed-scope helpers could be a privilege
  escalation vector if a malicious user script lands at a managed
  path. Mitigation: managed-settings paths are root-owned by
  convention; the helper invocation is `execv` (no shell) so no
  argv-injection.
- **TOML/JSON drift.** A user's `settings.json` and `settings.toml`
  in the same scope could diverge silently. Mitigation: WARN when
  both exist; `caliban doctor` (separate spec) reports them.
- **Hot-reloading `model` is tempting.** Operators may expect that
  editing `model` mid-session takes effect. Mitigation: documented as
  restart-required; `/config` shows a "restart required" badge next
  to those rows.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean; `cargo fmt --all -- --check`
  clean.
- ≥18 new tests under `caliban-core/src/settings/*` and TUI snapshot
  tests; all passing.
- Generated JSON Schema is emitted at
  `target/schemas/caliban-settings.json` during build.
- `caliban config print` emits the effective merged settings with
  scope annotations.
- `caliban config migrate` produces a unified `settings.json` from
  legacy per-feature files.
- `/config` slash command opens the tabbed TUI editor; edits round-trip
  via the file-watcher.
- All five rows under **D. Configuration / settings** in
  `docs/parity-gap-matrix.md` move 🔴 → ✅. Section M's `/config` row
  moves 🔴 → ✅.
- README's "Settings" section documents the scope chain, merge rules,
  and the schema URL.
- ADR 0026 in `accepted` status alongside this implementation.
