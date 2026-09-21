# ADR 0060 · Router config resolves through the settings layer; `caliban.toml` discovery retired

- **Status:** accepted
- **Date:** 2026-09-20
- **Amends:** [0038](0038-model-router-v2.md) (the "`caliban.toml` discovery uses the CLAUDE.md walk algorithm" decision)
- **Author:** john.ford2002@gmail.com

## Context

[ADR 0038](0038-model-router-v2.md) wired the model router from a `caliban.toml`
file that the binary located by an **independent discovery chain**: CLI `--config`
flag → `CALIBAN_ROUTER_CONFIG` env var → walk up from the cwd to the git root or
`$HOME` → `~/.config/caliban/caliban.toml`. This predated caliban's unified
**settings layer** (`.caliban/settings.toml`, ADR 0026), which resolves every
other configuration key through one precedence chain (`Cli > Local > Project >
User > Managed`) with per-key provenance surfaced by `caliban config print`.

The result was two parallel configuration mechanisms that never met: router
config had its own file, its own search algorithm, and no provenance, while
everything else lived in settings. #540 took the first step — it taught the
binary to prefer a settings-layer `[router]` section over discovery — but kept
the discovery chain as a fallback and did **not** mirror per-provider
`[provider.X]` blocks (custom `api_key_env` / `base_url`) into settings, so a
settings-sourced router could not point at a proxy or a keyless local endpoint.

Keeping both mechanisms is a standing tax: two code paths, two places to look, a
walk-up that silently reaches outside the workspace, and router config that is
invisible to the provenance tooling every other key enjoys.

## Decision

**Router config resolves through the settings layer, with an explicit file
override — and nothing else.**

1. **Precedence.** `--config <PATH>` / `CALIBAN_ROUTER_CONFIG` (a standalone
   `caliban.toml`) is the highest-precedence source and, when set, overrides the
   settings `[router]` section. Otherwise the settings-layer `[router]` value is
   used (flowing through the normal settings precedence and provenance).
   Otherwise the binary uses its single-provider fallback.

2. **Discovery removed.** The walk-up and `~/.config/caliban/caliban.toml`
   steps are deleted. A bare repo-root `caliban.toml` is no longer picked up
   implicitly. The router crate's file module keeps only *explicit* loading
   (`load_router_config_file(explicit)`); when no explicit path is given it
   returns `None` rather than searching.

3. **Per-provider blocks in settings.** The settings `[router]` value carries
   optional `[router.provider.X]` blocks, typed binary-side via
   `caliban_model_router::router_and_providers_from_value`. This reaches parity
   with a standalone `caliban.toml`'s top-level `[provider.X]` blocks, so a
   settings-sourced router can set `api_key_env` / `base_url` per provider.

4. **Migration.** `caliban config import-router [--from <PATH>] [--dry-run]`
   reads a legacy `caliban.toml`, relocates its top-level `[provider.X]` blocks
   under `[router.provider.X]`, validates the result exactly as the runtime will
   load it, and merges it into `<workspace>/.caliban/settings.toml` `[router]`
   (preserving other settings keys). At startup, a still-present but now-orphaned
   `caliban.toml` above the cwd triggers a one-time warning pointing at this
   command, so a discovered-yesterday config never vanishes silently.

5. **`caliban router debug`** resolves config the same way a real run does
   (explicit file, else settings `[router]`), not via the deleted discovery path.

The settings crate stays free of a dependency on the router schema: it carries
`router` as an opaque value and the binary types it at wiring time, as
established by #540.

## Consequences

- **Breaking.** A repo-root or `~/.config/caliban/caliban.toml` that was
  auto-discovered before is now ignored unless migrated into settings or passed
  via `--config`. The startup warning and `caliban config import-router` make the
  migration one command; the change is called out in the changelog.
- One configuration model. Router config gains settings provenance
  (`caliban config print` shows its source scope) and drops a bespoke search
  algorithm and its four tests.
- The `--config` flag becomes an explicit, highest-precedence override rather
  than the top of a multi-step search — matching the intuition that an explicit
  CLI flag beats a config file.
- `caliban_common::paths::walk_up_for_file` is still used, now only to *detect*
  an orphaned `caliban.toml` for the deprecation warning, not to load it.

This supersedes ADR 0038's "`caliban.toml` discovery uses the CLAUDE.md walk
algorithm" decision; the rest of ADR 0038 (fallback, hedging, breakers,
capability filtering, effort levels, binary wiring) stands.
