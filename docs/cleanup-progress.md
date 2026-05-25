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
| PR-T2-A | 2 | Split `caliban/src/tui.rs` (3970 LOC) | — | — | — | — | — |
| PR-T2-B | 2 | Split `caliban/src/main.rs` (1844 LOC) | — | — | — | — | — |
| PR-T2-C | 2 | Split `caliban-agent-core/src/stream.rs` (1219 LOC) | **+52** (4 files, 1271 LOC total vs. 1219 single-file) | — | — | ±0 (3 turn-timing tests carried over) | n/a | n/a |
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
