# ADR 0026 · Layered settings.json + `/config` editor

- **Status:** accepted
- **Date:** 2026-05-24
- **Spec:** `docs/superpowers/specs/2026-05-24-settings-hierarchy-design.md`

## Context

caliban today has three ad-hoc TOML files (`permissions.toml`,
`mcp.toml`, the upcoming `hooks.toml` per ADR 0024) each loaded by its
own crate with no shared scope hierarchy, no schema, no merge rules,
no live reload, no interactive editor, and no dynamic auth surface.
Claude Code consolidates all of this into one layered `settings.json`
with documented managed > user > project > local merge semantics, a
JSON Schema at `https://json.schemastore.org/claude-code-settings.json`,
a tabbed `/config` editor, and `apiKeyHelper` for dynamic API-key
refresh. Closing that gap is Tier-1 foundation work because plugins
(eventual ADR 0030), observability, headless mode, and downstream
tooling all want a single configuration story. Full spec at
`docs/superpowers/specs/2026-05-24-settings-hierarchy-design.md`; this
ADR records the architectural commitments only.

## Decision

### JSON is the primary format; TOML is honored at the same path

`settings.json` is the canonical filename at each scope. The same path
with a `.toml` extension is parsed identically (`settings.toml`,
`settings.local.toml`). Rationale for JSON-primary: parity with Claude
Code's documented schema URL, JSON-Schema editor support out-of-the-box,
and serde supports both with no extra work. If both exist in the same
scope, JSON wins with a WARN logged.

### Four scopes with a documented merge order

In priority order, **CLI** > **Local** > **Project** > **User** >
**Managed** (default). Managed sits at the bottom by default so
operators can augment org defaults, but moves to the top when the
managed setting sets `parentSettingsBehavior: "block"` — mirrors
Claude Code's escape hatch. `--settings <FILE|JSON>` injects a virtual
scope above local; `--setting-sources <CSV>` restricts which scopes
are read (e.g. `user,project` for known-good CI base).

### Merge rules: scalars highest-wins, arrays mostly concatenate

Per-key rules are documented in the spec. The headline:

- Permission arrays (`allow`/`ask`/`deny`), hook arrays
  (`hooks.<Event>`), MCP allow/deny lists, `available_models`,
  `additional_directories`, `claude_md_excludes` all **concatenate in
  priority order** with dedup where meaningful.
- `mcp.servers.<name>` and `env` **deep-merge**.
- Every other scalar is **highest-wins**.

The `/config Effective` tab annotates each value with the scope it
came from.

### Strongly-typed `Settings` struct with `deny_unknown_fields`

`Settings` is a serde-derived struct in `caliban-core::settings`. Top-
level keys are typed; unknown top-level keys fail loudly. A
`#[serde(flatten)] extra: BTreeMap<String, Value>` escape hatch
captures forward-compat keys without forcing a release for every new
Claude Code field. JSON Schema is generated from `schemars` derives at
build time and published at `https://caliban.dev/schemas/settings.json`.

### Per-feature TOML files remain a compat fallback for one deprecation window

`permissions.toml` / `mcp.toml` / `hooks.toml` continue to load *only*
when the unified `settings.json` does not define the matching top-level
key. `caliban config migrate` round-trips them into a single
`settings.json`. After one minor release the compat path logs
DEPRECATED; after two it errors.

### Live reload via `notify` + `arc-swap` + `ConfigChange` hook

A `SettingsWatcher` watches each scope's path, debounces 250 ms,
re-loads + re-merges, and atomically swaps the `Arc<Settings>` via
`arc-swap`. A `ConfigChange` hook event (ADR 0024) fires with the diff
so external observers and in-process subscribers can react. Live-
reloadable keys are documented (`permissions.*`, `hooks.*`,
`api_key_helper.*`, UI keys, `env`, etc.). Restart-required keys
(`model`, `mcp.servers.*`, `auto_memory_*`) log WARN on change and
take effect on next launch; `/config` shows a "restart required" badge.

### `apiKeyHelper` is shell-out with caching + per-provider routing

A configurable script that emits the provider API key on stdout. Two
shapes:

- Single helper with `provider: "*"` as fallback for all providers.
- Array of helpers keyed by provider.

Cached `refreshIntervalMs` (default 5 min) or until a provider returns
401, whichever comes first. Refresh is inline against a
`slowHelperWarningMs` (default 10 s); env var
`CALIBAN_API_KEY_HELPER_TTL_MS` mirrors Claude Code's contract. The
helper is `execv`'d without a shell to avoid argv injection.

Auth precedence chain (per provider): per-provider helper → wildcard
helper → env var → keyring → anonymous (local providers).

### `/config` is a tabbed TUI overlay; edits write to project scope by default

Tabs per top-level key group (Model, Permissions, Hooks, MCP, Memory,
UI, Auth, Effective). Each row carries a `[scope]` chip showing which
scope contributed the effective value. `s` cycles the write-scope;
`w` flushes pending edits via atomic temp-file + rename. The Effective
tab is read-only and mirrors `caliban config print`.

The file-watcher picks up `/config`'s own writes automatically, so the
running process refreshes via the same code path external edits hit —
no extra plumbing.

## Consequences

- **Positive:** Closes all five rows under "D. Configuration /
  settings" in `docs/parity-gap-matrix.md` plus the `/config` row in
  section M. Establishes the single configuration story plugins,
  hooks, MCP, model router, and headless mode all consume.
  `apiKeyHelper` unlocks short-lived-credential workflows (AWS STS,
  GCP IAM, internal vault systems) caliban can't currently
  participate in. Live reload makes hook/permission iteration cycle-
  fast.
- **Negative:** `Settings` struct gains ~30 top-level keys — a real
  surface area to keep typed and tested. Merge rules are intricate
  (8-row table); operator confusion is real, mitigated by the
  Effective tab. Live reload introduces "settings changed mid-turn"
  semantics that subtle bugs can hide in (e.g. a permission allowed
  at turn start gets revoked mid-turn — we honor the rule at
  *dispatch time*, but documenting and testing that boundary takes
  care). One-release compat window for legacy TOMLs adds short-term
  parser surface. `apiKeyHelper` is shell-out; a managed-scope
  malicious script would be a privesc vector (mitigated by managed
  paths being root-owned by convention).
- **Revisit if:** Settings struct grows beyond ~50 top-level keys
  (refactor into named sub-modules per group). If live-reload
  semantics prove too surprising for operators, move to a
  reload-on-`/config-w` model. If managed delivery channels (Windows
  registry, macOS plist) become a real ask, add a `ScopeLoader`
  backend per channel. If `apiKeyHelper`'s 5-minute cache proves
  wrong for short-TTL credentials, expose `refreshIntervalMs` per
  provider.
