# ADR 0050 · XDG-first path locations on all platforms

- **Status:** accepted
- **Date:** 2026-07-03

## Context

caliban resolves its per-user config, data, cache, and state directories through
helpers in `caliban-common::paths`. Until now those helpers deferred to the
OS-native locations from the `dirs` crate when no `XDG_*` override was set:
`dirs::config_dir()` → `~/.config` on Linux but **`~/Library/Application Support`
on macOS**, and `dirs::data_local_dir()` similarly. For a **terminal-first
developer tool**, `~/Library/Application Support` is the wrong convention: it is
a GUI-app store, hidden by Finder, awkward to `cd` into, and not where CLI users
look. The tools caliban lives among (`git`, `nvim`, `gh`, `kubectl`, and the
Claude Code it replaces) use `~/.config` / `~/.local/share` or a home dotdir —
never Application Support.

The surface had also drifted into several schemes at once:

- Some call sites used the XDG-aware helper; others called **bare
  `dirs::config_dir()` / `dirs::data_local_dir()`**, which ignore
  `$XDG_CONFIG_HOME` / `$XDG_DATA_HOME` on macOS/Windows — so an operator's XDG
  override moved *settings* but not *permissions*, *hooks*, or the *router* config.
- A `~/.caliban/` home dotdir was used for checkpoints, memory rules, and the
  imports-allowlist — inconsistent with everything else.
- `mcp.toml` was read from both `~/.config` and the native path, but
  `settings.toml` was native-only on macOS, so an MCP `[mcp_servers]` block in
  `~/.config/caliban/settings.toml` was silently ignored while an `mcp.toml`
  server in the same directory worked.
- Outliers: the managed-settings path used capital `Caliban` on macOS, and
  stream-overflow used `directories::ProjectDirs` → `dev.caliban.caliban/`.

Notably, most path-related ADRs (0011, 0018, 0019, 0020, 0023, 0030, 0031, 0032,
0036, 0045) already documented **XDG** locations (`~/.config/caliban`,
`$XDG_DATA_HOME`, `$XDG_STATE_HOME`). The *implementation* drifted to
macOS-native, not the ADRs. The lone ADR that baked in the divergence is
**0017**, which described `mcp.toml` as "XDG-aware on Linux, cache_dir on macOS."

Options weighed: **(A)** XDG-first on all platforms; **(B)** a `~/.caliban/` home
dotdir for everything (à la Claude Code's `~/.claude`); **(C)** keep the
OS-native default. (C) is the status quo we are rejecting. (B) is simple and
discoverable but abandons the config/data/cache/state separation and pollutes
`$HOME`. (A) is uniform across OSes, matches the CLI ecosystem, realigns the
implementation with the ADRs above, and closes the settings/mcp split.

## Decision

We will make caliban **XDG-first on every platform** (Linux, macOS, Windows).

- `caliban-common::paths` exposes `platform_config_dir` / `platform_data_dir` /
  `platform_state_dir` / `platform_cache_dir`. Each honors its `XDG_*_HOME`
  override if set and non-empty, otherwise falls back to the XDG home layout —
  `~/.config`, `~/.local/share`, `~/.local/state`, `~/.cache` — on **all**
  platforms. We do **not** defer to `dirs::config_dir()` /
  `dirs::data_local_dir()` (i.e. never `~/Library/Application Support`).
- **Every** production call site for caliban's own config/data/cache/state goes
  through these helpers. Bare `dirs::config_dir()` / `data_dir()` /
  `data_local_dir()` / `state_dir()` / `cache_dir()` and
  `directories::ProjectDirs` are banned outside `paths.rs`; a `cargo test` guard
  (`caliban-common/tests/no_bare_platform_dirs.rs`) enforces this.
  `dirs::home_dir()` remains allowed — it is the base the XDG helpers build on.
- The `~/.caliban/` home dotdir is retired: checkpoints and TUI reverse-history
  move to `<data>/caliban/projects/`, memory rules to `<config>/caliban/rules/`,
  and the imports-allowlist to `<state>/caliban/imports-allowlist.json`.
- Managed/system settings unify on `/etc/caliban` across Unix (dropping the
  macOS `/Library/Application Support/Caliban`); Windows stays
  `C:\ProgramData\Caliban`.
- **No Library fallback and no migration.** Old locations are abandoned, not
  read; users start fresh. The only inbound migration is the existing manual
  `caliban settings import --from …` / `caliban perms import --from …` importer
  (Claude Code / Codex JSON → caliban TOML), which stays **manual** — there is no
  automatic first-run auto-seed.

This ADR **codifies** the location policy that ADRs 0011, 0018, 0019, 0020,
0023, 0030, 0031, 0032, 0036, and 0045 already assumed, and **amends
[ADR 0017](0017-mcp-client-architecture.md)**,
whose "cache_dir on macOS" note for `mcp.toml` is superseded by the XDG-first
rule here. Because `mcp.toml` and the unified `settings.toml` now both resolve
to `~/.config/caliban`, the macOS split that orphaned `[mcp_servers]` blocks in
`settings.toml` is closed.

## Consequences

- **Positive:** one uniform path scheme across Linux/macOS/Windows — a single
  story for docs, code, and support; CLI-discoverable locations; `$XDG_*`
  overrides work everywhere; the settings/mcp split-brain and the four drift
  items (helper bypass, `~/.caliban` dotdir, capital `Caliban`, `ProjectDirs`)
  are all resolved; the guard test prevents regression.
- **Negative:** a **breaking change with no migration** — existing macOS users'
  sessions, checkpoints, permissions, and memory under
  `~/Library/Application Support/caliban` (and `~/.caliban`) are abandoned and
  must be re-created or hand-imported. macOS purists may object that a CLI tool
  writing to `~/.config` is "non-native"; we accept that trade for uniformity.
- **Revisit if:** caliban ever ships a native macOS GUI surface (where
  Application Support would be idiomatic), or if a packaging/OS constraint
  requires the platform-native dirs — at which point the single `xdg_base`
  chokepoint makes reversal a one-file change.
