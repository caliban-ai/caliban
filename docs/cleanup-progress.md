# Cleanup & performance sprint — Progress ledger

Tracks per-PR LOC delta, duplication sites consolidated, test-suite changes,
and (once the perf tier lands) binary-size + test-runtime deltas.

Baselines (TBD — captured by PR-T4-0):

- Release binary size: TBD
- Workspace test-suite runtime: TBD
- Headless cold-start time: TBD

## PR ledger

| PR | Tier | Title | LOC delta (consumer sites) | LOC delta (incl. new common crate) | Dup sites consolidated | Tests +/− | Binary size Δ | Test runtime Δ |
|---|---|---|---:|---:|---:|---:|---:|---:|
| PR-T1-A | 1 | `caliban-common` foundation crate | **−305** (544 deleted / 239 added at consumer sites) | +629 (new `caliban-common` module is ~934 LOC incl. ~350 LOC of tests) | 7 / 8 (env-expand ×2; atomic-write ×5; sanitize_cwd ×2; walk_up ×2; matches_glob+first_arg ×1; tracing targets 61/88 sites; XDG paths consolidated into helpers awaiting next consumers — sessions/supervisor/oauth) | +34 net new (39 new in `caliban-common`; carried-over tests removed from `agent-core`/`memory`) | n/a | n/a |
| PR-T1-B | 1 | Shared `reqwest::Client` factory | — | — | — | — | — |
| PR-T2-A | 2 | Split `caliban/src/tui.rs` (3970 LOC) | — | — | — | — | — |
| PR-T2-B | 2 | Split `caliban/src/main.rs` (1844 LOC) | — | — | — | — | — |
| PR-T2-C | 2 | Split `caliban-agent-core/src/stream.rs` (1219 LOC) | — | — | — | — | — |
| PR-T2-D | 2 | Split `caliban-model-router/src/lib.rs` (1499 LOC) | — | — | — | — | — |
| PR-T3-A | 3 | Group `caliban-tools-builtin` modules | — | — | — | — | — |
| PR-T3-B | 3 | Settings as canonical config root | — | — | — | — | — |
| PR-T4-0 | 4 | Baseline measurement | — | — | — | — | — |
| PR-T4-A | 4 | Hot-path tracing audit | — | — | — | — | — |
| PR-T4-B | 4 | Session persist debouncing | — | — | — | — | — |
| PR-T4-C | 4 | Cargo dep audit + release profile | — | — | — | — | — |
| PR-T4-D | 4 | `CompositeHooks` short-circuit | — | — | — | — | — |
| PR-T5-A | 5 | App builder | — | — | — | — | — |
| PR-T5-B | 5 | `CalibanError` centralization | — | — | — | — | — |

## PR-T1-A notes

- Repurposed the 13-LOC `caliban-core` crate as `caliban-common` (workspace
  rename; `crates/caliban-core/` → `crates/caliban-common/`).
- New modules and the duplication they replace:
  - `paths::{xdg_config_home, xdg_data_home, xdg_runtime_home,
    sanitize_cwd_for_path, walk_up_for_file}` — folds the per-crate
    sanitize / walk-up impls (memory + model-router) and adds first-class
    XDG helpers for the next consumers (sessions, supervisor, oauth).
  - `expand::{expand_vars, ExpandContext, ExpandError, MissingPolicy,
    expand_vars_from_env}` — one canonical impl. Replaces the two ad-hoc
    impls in `caliban-mcp-client::config::expand_value` and
    `caliban-plugins::expand`. Settings has no local impl; it now uses the
    same module by virtue of consuming MCP-client config.
  - `fs::{write_atomic, write_atomic_with_mode}` — replaces five ad-hoc
    tmp+rename recipes across `caliban-checkpoint::store` (×2),
    `caliban-checkpoint::restore`, `caliban-tools-builtin::notebook_edit`,
    `caliban-memory::auto` (×3 — write + index update + index removal),
    and `caliban-memory::project_imports`.
  - `glob_match::{matches_glob, first_arg}` — moved from
    `caliban-agent-core::permissions` (re-exported there with
    `#[deprecated]` for back-compat).
  - `tracing_targets::*` — 25 `const &str` covering the existing target
    namespace. 61 of 88 call sites migrated in this PR; the remaining 27
    sites — all in dev-only doc/comment text or single one-offs — can be
    swept in a follow-up.

### Notes on deviations from the original PR-T1-A brief

- The brief lists "3 distinct env-expand impls"; only 2 actually existed in
  source. `caliban-settings` had no local expand impl — it inherits MCP's
  via `caliban_settings::compat` re-mapping. Recorded as 2/3 in the dup
  table.
- A fifth `write_atomic`-style site was found in
  `caliban-checkpoint::restore::atomic_overwrite` and also migrated (mode
  variant). The brief named four; the fifth is documented above.
- Glob-matcher tests live in `caliban-common::glob_match::tests` (carried
  over); the dual copies in `caliban-agent-core::permissions::tests` were
  removed to avoid duplicate coverage.
- Net LOC delta is **+629 incl. `caliban-common`** vs. the
  brief-aspirational net-negative. The new common crate (~934 LOC, of
  which ~350 LOC are tests) is the foundation the rest of the sprint
  builds on — the consumer-site delta is −305 lines and the headline
  net-negative goal is sprint-level, not PR-level.
