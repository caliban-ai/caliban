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
| PR-T1-B | 1 | Shared `reqwest::Client` factory | **+12** (30 added / 18 deleted at consumer sites) | +158 (new `caliban-common::http` is ~146 LOC incl. ~52 LOC of tests) | 9 / 9 (7 provider transports + vertex `list_client` + `web_fetch_client`) | +6 net new (all in `caliban-common::http`) | n/a | n/a |
| PR-T2-A | 2 | Split `caliban/src/tui.rs` (3970 LOC) | **+101** (4071 lines across 5 files / 3970 original; tests stay at root) | n/a | n/a (file split, no dup consolidation) | 0 net (existing tests unmoved) | n/a | n/a |
| PR-T2-B | 2 | Split `caliban/src/main.rs` (1844 LOC) | **+298** (2142 total / 1844 before; pure file split — no consumer-site delta) | n/a (no new crate) | n/a (split, no dup consolidation) | +0 (no new tests) | n/a | n/a |
| PR-T2-C | 2 | Split `caliban-agent-core/src/stream.rs` (1219 LOC) | **+52** (4 files, 1271 LOC total vs. 1219 single-file) | — | — | ±0 (3 turn-timing tests carried over) | n/a | n/a |
| PR-T2-D | 2 | Split `caliban-model-router/src/lib.rs` (1499 LOC) | **+64** (lib.rs 1499 → 342; new builder.rs 102, dispatch.rs 280, provider_impl.rs 60, tests.rs 779) | — | — | 0 (existing 64 tests relocated to `tests.rs` unchanged; all green) | n/a | n/a |
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

## PR-T2-C notes

- `crates/caliban-agent-core/src/stream.rs` (1219 LOC) carved into a
  module directory:
  - `stream/mod.rs` (911 LOC) — imports, all public types (`TurnEvent`,
    `TurnOutcome`, `RunOutcome`, `StopCondition`, `TurnEventStream`,
    `RunSettings`), `impl Agent { stream_until_done* }` with the
    `try_stream!` macro body intact, and sub-module declarations.
  - `stream/turn.rs` (191 LOC) — `TurnTiming` (+TTFT/TBT tests),
    `ActiveBlock`, `MessageAccumulator`.
  - `stream/parallel.rs` (33 LOC) — `DispatchPlan` enum bridging the
    serial-plan and parallel-dispatch phases.
  - `stream/hook_dispatch.rs` (136 LOC) — `dispatch_tool` free async
    helper (single-tool fan-out through `before_tool`/invoke/
    `after_tool` including `UpdatedInput` threading).
- Pure file split: no semantic changes. All existing tests
  (203 in `caliban-agent-core` lib, plus crate-integration tests) pass
  unchanged. No new tests added.

### Deviations from PR-T2-C brief

- Target LOC for `stream/mod.rs` was 200-300; achieved 911 LOC.
  The brief's targets assumed extracting the per-turn loop body and
  parallel dispatch into helper functions, but doing so requires
  restructuring around `async_stream::try_stream!`'s `yield` (which only
  works inside the macro). The conservative path — keep the macro body
  intact in `mod.rs` and move only standalone items to submodules —
  preserves "no semantic changes" exactly; a future PR can revisit the
  loop-body split using nested `try_stream!` sub-streams (event ordering
  is preservable but the refactor is non-trivial).
- `parallel.rs` and `hook_dispatch.rs` likewise come in under the
  spec-suggested LOC (33 / 136 vs. 300 / 200-300) because no free
  helper functions outside `DispatchPlan` and `dispatch_tool` could be
  extracted without that same refactor.
- Public API surface unchanged. Consumers continue to import from
  `caliban_agent_core::stream::...`; the re-exports from
  `caliban-agent-core/src/lib.rs` need no edits.

## PR-T2-B notes

- Original `caliban/src/main.rs` was 1836 LOC, mixing clap parsing,
  startup orchestration, and subcommand dispatch.
- Carved into four siblings:
  - `args.rs` (452 LOC) — `Args` struct + `CalibanCommand`/`AgentsCommand`/
    `DaemonCommand`/`RouterCommand` enums + `ProviderKind` + CLI helpers
    (`read_prompt`, `summarize`, `summarize_blocks`, `default_model_for`,
    `provider_name`).
  - `startup.rs` (1242 LOC) — every state-assembly helper:
    `init_debug_tracing`, `build_provider`, `build_registry`,
    `load_layered_settings`, `auto_memory_disabled`, `web_fetch_client`,
    `load_plugin_manager`, `start_mcp`, `install_sub_agent`,
    `load_hooks_config`, `build_permissions` (returning
    `PermissionsSetup`), `build_agent`, `resolve_session`,
    `resolve_system_prompt`, `fire_session_start` / `fire_session_end`,
    `run_and_render`, `run_headless`, `run_single_prompt`.
  - `subcommands.rs` (91 LOC) — top-level dispatchers: `run_plugin_cli`,
    `run_router_debug`, `run_supervisor_command` (handles
    `agents`/`daemon`/`attach`/`logs`/`stop`/`kill`/`respawn`/`rm`),
    `run_bg_shortcut`.
  - `main.rs` (357 LOC) — `#[tokio::main] async fn main()`: argv routing
    (plugin proxy → router-debug → supervisor → `--bg`), debug tracing
    init, then a linear pipeline of `startup::*` calls and the three
    driver dispatches (headless / TUI / single-prompt).
- The original CLI types remain reachable at `crate::Args`,
  `crate::ProviderKind`, `crate::AgentsCommand`, etc. via a
  `pub(crate) use crate::args::{…}` re-export in `main.rs` — keeps the
  existing references from `tui.rs`, `agents_cli.rs`, and the
  `headless`/`system_prompt` modules working without a global rename.
- `tui::ask` was bumped from `mod` to `pub(crate) mod` so
  `startup::build_permissions` can name `tui::ask::AskRequest` in the
  return type of `PermissionsSetup`.
- Pure file split — no semantic changes, no new tests. All 225
  `caliban`-binary tests pass unchanged; full workspace
  `cargo build/clippy/fmt/test` is clean (the lone occasional failure
  in `caliban-memory` / `caliban-common` env-var tests under parallel
  workspace `cargo test` is pre-existing flakiness unrelated to this
  change — both crates pass cleanly in isolation).
- `main.rs` is 357 LOC, modestly above the ~200-LOC brief target. The
  remainder is the three driver dispatches (each takes 10–14 state
  arguments) plus the `settings_sources_view` mapping for the TUI
  driver — both stay inline because extracting them would only push
  large tuples around without simplifying the code shape.
- `startup.rs` ended up at 1242 LOC vs. the ~600-800 LOC brief
  estimate. The extra weight is the `run_headless` driver (~270 LOC)
  and `install_sub_agent` (~110 LOC); both are atomic units of work
  whose internals weren't split further to keep the refactor purely
  mechanical.

## PR-T1-B notes

- New `caliban-common::http` module with three constructors:
  - `default_client_builder()` — yields a `reqwest::ClientBuilder`
    pre-configured with the shared User-Agent
    (`caliban/<CARGO_PKG_VERSION>`), HTTP/2 adaptive window, hickory-DNS
    resolver, rustls TLS backend, redirect limit of 10, and a 30s default
    timeout. Provider transports layer their custom timeout on top via
    `.timeout(config.timeout)`.
  - `default_client()` — convenience helper that calls `.build()` on the
    above; panics on TLS / DNS init failure (matches the expectations of
    every existing call site that did `.expect(...)` on its builder).
  - `no_redirect_client()` — same defaults but with `Policy::none()`, for
    `web_fetch`'s manual same-host redirect enforcement.
- Migrated 9 call sites: 8 provider transports
  (`anthropic::{direct,vertex}`, `openai::{direct,azure}`,
  `google::{ai_studio,vertex}`, `ollama::direct`, `vertex::list_client`)
  plus `caliban/src/main.rs::web_fetch_client`.
- The brief described "5–10 LOC shrink per migrated site"; in practice
  each provider site had only `.timeout(config.timeout)` set explicitly
  before this PR — the savings show up as *absorbed* shared defaults
  (User-Agent, HTTP/2, hickory-DNS, rustls, redirect cap) that each site
  would otherwise need to repeat to reach parity. The `web_fetch_client`
  site does shrink by 8 LOC (boilerplate → one-line delegation).
- Behaviour preserved: provider transports keep their explicit
  `.timeout(config.timeout)` override (some configs set this to multi-
  minute values). The Anthropic Vertex variant inherits its
  `anthropic-version` header from existing `auth_headers()` logic
  unchanged. `web_fetch` keeps `Policy::none()` for its manual same-host
  redirect handling.
- `WebSearchTool::new` and `WebFetchTool::new` keep their injectable
  `reqwest::Client` parameter for test ergonomics (wiremock-friendly
  clients without TLS); docstrings now direct production callers at
  `caliban_common::http::{default_client, no_redirect_client}`.

## PR-T2-A notes

Pure file split of `caliban/src/tui.rs` (3970 LOC) into focused submodules.
No semantic changes; no new tests; existing 1307-test suite passes
unchanged.

LOC after split (per `wc -l`):

| File | LOC |
|---|---:|
| `caliban/src/tui.rs` (root + tests) | 1129 |
| `caliban/src/tui/app.rs` | 391 |
| `caliban/src/tui/render.rs` | 538 |
| `caliban/src/tui/events.rs` | 1323 |
| `caliban/src/tui/overlay.rs` | 690 |
| **total** | **4071** |

The +101 LOC delta vs. the 3970-line original is the cost of adding
module-level docs, `use` declarations, and the test module's re-imports of
private helpers via their new submodule paths. No source file exceeds
1500 LOC.

The root `tui.rs` is 1129 LOC because the existing `mod tests` block
(~830 LOC) stays at the root. The non-test prelude (module decls,
`TerminalGuard`, `run()`, re-exports) is ~295 LOC, very close to the
spec's 200-LOC target. Moving tests into the new submodules would have
required either making more symbols `pub(crate)` than necessary or
duplicating fixtures across modules; keeping them centralized matches
the "no test changes" constraint.

### Symbol assignment

- `tui.rs` root: `mod` decls + `is_esc_chord` + `ESC_ESC_WINDOW_MS` +
  `TerminalGuard` (RAII) + `run()` entry + `pub(crate) use` re-exports
  for cross-module / cross-crate callers (`App`, `TranscriptLine`,
  `Overlay`, `ViewState`, `TuiAskHandler`, `render_usage_lines`,
  `render_context_lines`, `handle_compact_command`). Test module
  unchanged in place; it imports private helpers via
  `super::{app,render,events,overlay}::*`.
- `app.rs`: `TranscriptLine`, `RunningTurn`, `Activity` + impl,
  `spinner_frame`, `App` struct + `App::new`, `App::with_checkpoint_store`,
  `App::cwd_display`.
- `render.rs`: `render`, `render_input_menu`, `format_tool_input`,
  `wrap_lines_to_width`, `render_transcript`, `format_bytes`,
  `format_cache_suffix`, `render_status`.
- `events.rs`: agent-event handlers (`handle_agent_event`,
  `handle_agent_error`), slash dispatch (`handle_slash_command`,
  `apply_slash_outcome`), `/usage` / `/context` / `/compact` helpers,
  the full keyboard/mouse dispatch (`handle_event`, `handle_key`,
  `handle_mouse`, per-overlay handlers, `cycle_permission_mode`,
  `handoff_to_supervisor`, `handle_ctrl_g`, `open_reverse_history`,
  `dispatch_shell_escape`, `refresh_at_menu`, `MOUSE_WHEEL_ROWS`).
- `overlay.rs`: `ViewState`, `Overlay` + `title`/`short_name`,
  `centered_rect`, `clone_lines`, `render_overlay`, per-overlay line
  builders (`reverse_history_lines`, `ask_modal_lines`,
  `slash_help_lines`, `config_lines`, `mcp_lines`, `skills_lines`,
  `system_lines`, `rewind_lines`).

Existing siblings (`tui/slash.rs`, `tui/slash/`, `tui/ask.rs`,
`tui/attach.rs`, `tui/external_editor.rs`, `tui/input.rs`,
`tui/reverse_history.rs`, `tui/shell_escape.rs`,
`tui/transcript_viewer.rs`, `tui/completer.rs`, `tui/toast.rs`) are
untouched.

`crate::tui::{App,TranscriptLine,Overlay,…}` import paths preserved at
the root so external callers (`main.rs`, `tui/slash/*`) compile
unchanged.

## PR-T2-D notes

- Pure file split, no semantic changes, no new tests. `lib.rs` slimmed
  from 1499 LOC to 342 LOC; existing 64 router-level tests relocated
  verbatim from the inline `#[cfg(test)] mod tests { ... }` block to a
  sibling `tests.rs` declared via `#[cfg(test)] mod tests;`.
- New modules:
  - `builder.rs` (102 LOC) — `ModelRouterBuilder` + its `Debug` /
    fluent methods.
  - `provider_impl.rs` (60 LOC) — `impl Provider for ModelRouter`
    delegating `complete`/`stream` to `dispatch.rs` and supplying the
    `capabilities` / `list_models` / `name` glue.
  - `dispatch.rs` (280 LOC) — candidate-resolution dispatch as
    additional inherent `pub(crate)` methods on `ModelRouter`
    (`dispatch_complete`, `dispatch_stream`, `rewrite_for_route`).
    Reusing inherent methods lets the dispatch loop touch private
    fields directly (no new public seams).
- Deviations from spec target sizes:
  - Spec targeted `lib.rs ~200`, `builder.rs ~250`, `provider_impl.rs ~400`,
    `dispatch.rs ~650` (total ~1500). Actual is `lib.rs 342`, `builder.rs
    102`, `provider_impl.rs 60`, `dispatch.rs 280`, plus `tests.rs 779`.
  - `lib.rs` is +142 over target because the public `RouteUsage` /
    `RouterStatsSnapshot` types, the `pub(crate) StatsHandle`, and the
    `render_diagnostics` free function all stay there (these are the
    natural public surface of the crate and don't fit any of the
    sibling modules cleanly).
  - `builder.rs` / `provider_impl.rs` are well *under* target because
    each is doing exactly one thing — the dispatch loop's heaviness
    moved fully into `dispatch.rs`, not into `provider_impl.rs`.
  - `dispatch.rs` is under target (280 vs ~650) because it's a single
    `impl ModelRouter { ... }` block of the existing `complete`/`stream`
    bodies — no helper extraction (the spec said *pure file split*).
- Crate field visibility tightened from `private` to `pub(crate)` on
  `ModelRouter.{default_purpose, routes, providers, breakers, stats}`
  so the new `dispatch.rs` and `provider_impl.rs` modules can reach
  them directly. `StatsHandle` and `StatsInner` likewise widened to
  `pub(crate)`. Nothing escapes the crate boundary.
- `lib.rs` retains the public re-export hub (`pub use builder::...`,
  `pub use breaker::...`, etc.); `ModelRouter`, `ModelRouterBuilder`,
  `RouterConfig`, `RouteEntry`, `render_diagnostics`, `Result`,
  `RouterError`, and the rest of the public API remain reachable from
  `caliban_model_router::*` unchanged.
