# Cleanup & performance sprint — Design

**Date:** 2026-05-25
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** *(no ADR; this is a multi-PR refactor program, not a new architectural decision)*

## Goal

After the parity sweep landed 22 PRs and ~77 k LOC across 24 crates, the
workspace has visible smells: duplicated helpers across crates, god-files
(`caliban/src/tui.rs` is ~3970 LOC), and unmeasured performance hot spots.
This sprint does a focused, simplification-first pass — extract shared
infrastructure, split oversized files, tighten crate boundaries, and
follow up with measurable perf wins where they don't muddy the code.

Bias is explicit: **simplification first**, performance follows where it
doesn't fight readability. No new features.

## Non-goals

- **No new ADRs.** This sprint moves code; it doesn't make new decisions.
- **No feature additions** (the 18 ADRs already shipped cover the parity
  set).
- **No backward-incompatible API breaks** outside of internal crates. The
  binary CLI surface stays the same; any internal trait signature changes
  ship with migrated call sites in the same PR.
- **No partial migrations.** Each PR is fully merged or fully reverted —
  no `legacy_*` modules lingering across PRs (deprecation markers are OK
  for one release cycle).
- **No unsafe-code expansion** beyond what already exists in
  `caliban-tools-builtin/src/bash.rs` for process-group cleanup.

## Diagnostic summary (what we found)

### Duplication

| Concern | Sites | Notes |
|---|---|---|
| `${VAR}` / `${CLAUDE_PROJECT_DIR}` expansion | 3 distinct impls | `mcp-client/config.rs`, `plugins/expand.rs`, `settings/loader.rs` |
| Atomic file write (tmp + rename) | 4 ad-hoc | `checkpoint/store.rs`, `notebook_edit`, `memory/auto.rs`, `memory/project_imports.rs` |
| XDG path helpers | 6+ sites | `sessions`, `supervisor`, `mcp-client/oauth`, `memory`, `agent-core/permissions`, `output-styles` |
| `sanitize_cwd` for filesystem-safe names | 5 sites | `memory/sanitize`, `checkpoint/store`, `supervisor/store`, `sessions`, `tui/reverse_history` |
| `walk_up_for_file` | 2 impls | `memory/walk.rs` + `model-router/discovery.rs` |
| `glob_matcher` / `first_arg` | only in `agent-core/permissions` | Other call sites would benefit (e.g. `caliban-sandbox::shim` glob bypass list) |
| `reqwest::Client::builder` boilerplate | 8 transports | Anthropic / OpenAI / Google / Vertex / Ollama / Bedrock / Vertex / sandbox-direct / web_fetch / web_search |
| `tracing` target string literals | dozens | `"caliban::tools"`, `"caliban::cache"`, `"caliban::mcp"`, etc. as bare strings |

### Oversized files (>1000 LOC)

| File | LOC | Smell |
|---|---:|---|
| `caliban/src/tui.rs` | 3970 | Mixes state, render, input dispatch, overlay logic |
| `caliban/src/main.rs` | 1844 | CLI parsing + startup + subcommand dispatch |
| `caliban-model-router/src/lib.rs` | 1499 | Provider impl + builder + breaker + hedge dispatch |
| `caliban-agent-core/src/stream.rs` | 1219 | Turn loop + hook chain + parallel dispatcher |
| `caliban-tools-builtin/src/web_fetch.rs` | 1182 | Single tool but complex (htmd, redirects, charset detection) — okay-ish |
| `caliban-mcp-client/src/oauth.rs` | 1142 | OAuth flow + token store + refresh + discovery — splittable |

### Crate-boundary smells

- `caliban-core` is **13 LOC** (a single `mod` reexport). Either delete or repurpose.
- `caliban-tools-builtin` is **7800 LOC** with 12 tool files flat in `src/`.
- `caliban-settings` wraps `McpConfig` / `HooksConfig` / `permissions::Rule[]` but their owning crates still expose ad-hoc TOML loaders that drift from the unified hierarchy.

### Performance suspects (unmeasured)

- Hot-path `tracing::debug!` in `caliban::tools` and `caliban::stream` targets — fires per tool call / per turn even when telemetry is off.
- Session persist on every turn end writes full JSON file synchronously.
- `CompositeHooks` `for`-loop + `await` per event, even when all members are no-ops.
- `Cargo.toml` workspace dep closure — `image` and `arboard` may carry unused features.
- Release binary is 25 MB unstripped, no LTO.

---

## Architecture

```
                       ┌──────────────────────────────────────────┐
                       │  caliban-common  (NEW; repurposed core)  │
                       │  ─────────────────────────────────────── │
                       │  expand_vars · write_atomic · xdg paths  │
                       │  sanitize_cwd · walk_up · glob_match     │
                       │  http::default_client · tracing_targets  │
                       │  errors::CalibanError                    │
                       └────────┬─────────────────────────────────┘
                                │ used by
                                ▼
  ┌───────────┬─────────────┬─────────────┬─────────────┬─────────────┐
  │ providers │ mcp-client  │ plugins     │ settings    │ tools-…     │
  │ memory    │ supervisor  │ checkpoint  │ output-styl │ images      │
  │ telemetry │ skills      │ sandbox     │ sessions    │ worktrees   │
  └───────────┴─────────────┴─────────────┴─────────────┴─────────────┘
                                │
                                ▼
                        caliban (binary)
                        ─────────────────
                        startup · args · subcommands
                        tui {app, render, events, overlay}
                        headless · system_prompt
```

`caliban-common` is the single home for cross-crate plumbing. Nothing
fancy lives there — no traits with multiple implementations, no
generic frameworks. Just pure functions and one or two opinionated
constructors (the `reqwest::Client` factory, `CalibanError`).

---

## Tier-by-tier plan

Each tier is a phase of PRs. **Tier 1 serializes** (foundation crate
must land before anything depends on it). **Tier 2 splits parallelize**
(each touches one file). **Tiers 3–5 parallelize subject to non-overlap**.

### Tier 1 — Foundation (~2 PRs)

#### PR-T1-A · `caliban-common` + first wave of migrations

- Repurpose `caliban-core` as `caliban-common` (the 13-LOC crate has
  one re-export; preserve it under the new name).
- Add modules:
  - `paths::{xdg_config_home, xdg_data_home, xdg_runtime_home, sanitize_cwd_for_path, walk_up_for_file}`
  - `expand::{expand_vars, ExpandError}` (one canonical implementation;
    supports `${VAR}`, `${VAR:-default}`, `${CLAUDE_PROJECT_DIR}`,
    `${CALIBAN_PLUGIN_ROOT}` with optional plugin-root alias for
    `${CLAUDE_PLUGIN_ROOT}`).
  - `fs::{write_atomic, write_atomic_with_mode}` (tempfile + rename;
    mode 0600 variant for credential blobs).
  - `glob::{matches_glob, first_arg}` (lift from
    `agent-core/permissions`).
  - `tracing_targets` (const &'static str for every existing target).
- Migrate all known sites to the new helpers in the same PR. Delete
  the duplicates. Each migration is a 5–20-line edit.
- Tests: existing tests carry over; new tests for each helper covering
  the spec'd behaviors.
- **Acceptance**: `cargo build/clippy/fmt/test` clean; LOC delta is
  net-negative (we delete more than we add); no behavioral change.

#### PR-T1-B · Shared `reqwest::Client` factory

- New `caliban-common::http::{default_client, default_client_builder}`.
- Migrate 8 transport call sites in `caliban-provider-{anthropic,openai,
  google,ollama,bedrock,vertex}` + `caliban-tools-builtin::web_{fetch,
  search}`.
- Centralize user-agent, redirect policy, http2 toggle, timeouts.
- **Acceptance**: same.

### Tier 2 — God-file splits (~4 PRs, parallelizable)

#### PR-T2-A · Split `caliban/src/tui.rs` (3970 LOC)

```
caliban/src/tui/
├── mod.rs        # re-exports + run() entry point (~200 LOC)
├── app.rs        # App struct + new() + builder (~600 LOC)
├── render.rs     # frame drawing, status bar, transcript (~900 LOC)
├── events.rs     # key/mouse dispatch, slash handling (~800 LOC)
└── overlay.rs    # Overlay enum + per-overlay render (~600 LOC)
```

Existing sub-modules (`tui/slash.rs`, `tui/attach.rs`, etc.) stay where
they are. No semantic changes — pure file split, all tests pass
unchanged.

#### PR-T2-B · Split `caliban/src/main.rs` (1844 LOC)

```
caliban/src/
├── main.rs           # entry point + argv routing (~200 LOC)
├── args.rs           # clap::Parser struct + flag parsing (~400 LOC)
├── startup.rs        # registry assembly, provider construction (~700 LOC)
└── subcommands.rs    # `caliban {agents,plugin,router,daemon,...}` dispatch (~500 LOC)
```

#### PR-T2-C · Split `caliban-agent-core/src/stream.rs` (1219 LOC)

```
crates/caliban-agent-core/src/stream/
├── mod.rs            # public surface, RunEvent, TurnEventStream
├── turn.rs           # single-turn loop
├── parallel.rs       # FuturesUnordered + Semaphore dispatch
└── hook_dispatch.rs  # CompositeHooks fan-out, UpdatedInput threading
```

#### PR-T2-D · Split `caliban-model-router/src/lib.rs` (1499 LOC)

```
crates/caliban-model-router/src/
├── lib.rs            # public surface, ModelRouter (~200 LOC)
├── builder.rs        # ModelRouterBuilder (~250 LOC)
├── provider_impl.rs  # impl Provider for ModelRouter (~400 LOC)
└── dispatch.rs       # candidate resolution + fallback/hedge loop (~650 LOC)
```

(`resolver`, `breaker`, `hedging`, `cache`, `capabilities`, `config`,
`discovery`, `effort`, `fallback` already live as siblings — this PR
just slims `lib.rs` to a re-export hub.)

### Tier 3 — Crate-boundary cleanup (~2 PRs)

#### PR-T3-A · Group `caliban-tools-builtin` modules

```
crates/caliban-tools-builtin/src/
├── lib.rs
├── fs/        # read, write, edit, multi_edit, notebook_edit
├── shell/     # bash, bash_bg
├── web/       # web_fetch, web_search
├── memory/    # memory.rs (ReadMemoryTopic, WriteMemoryTopic)
├── agent/     # agent_tool, todo_write
└── plan/      # plan_mode_tools, skill
```

#### PR-T3-B · Settings as canonical config root

- Mark `caliban-agent-core::hooks_config::load_hooks_file`,
  `caliban-agent-core::permissions::load_rules`, and
  `caliban-mcp-client::config::load_config` as `#[deprecated(note =
  "load via caliban-settings instead; legacy loaders remove in v0.2")]`.
- Binary calls into `caliban-settings::load_layered_settings` once;
  deprecated entry points stay functional for one release.
- Tests verify Settings → sub-config conversion produces identical
  shapes to the legacy loaders.

### Tier 4 — Performance (~4 PRs)

#### PR-T4-0 · Baseline measurement

- New module `caliban-common::bench` (test-only) with helpers to
  measure workspace test-suite runtime, headless `-p` cold-start time,
  and release-binary size.
- Document baseline in `docs/cleanup-progress.md`.

#### PR-T4-A · Hot-path tracing audit

- Identify per-tool / per-token / per-turn `tracing::debug!` calls.
- Replace bare emissions with `if tracing::enabled!(...) { ... }` guards
  where the argument formatting is non-trivial.
- Verify release-mode regression-free.

#### PR-T4-B · Session persist debouncing

- Replace synchronous write-on-every-turn with debounced (250 ms)
  + on-exit-flush via `tokio::sync::mpsc` + a dedicated writer task.
- Uses `caliban-common::fs::write_atomic`.
- Tests verify crash-safety (writer task drained on `Drop`).

#### PR-T4-C · Cargo dep closure audit + release profile tuning

- Add `[profile.release]` block:
  - `lto = "thin"`
  - `codegen-units = 1`
  - `strip = "symbols"`
- Audit `default-features = false` opportunities (`image`, `arboard`,
  `oauth2`, `reqwest` already trimmed).
- Update `docs/cleanup-progress.md` with binary-size delta.

#### PR-T4-D · `CompositeHooks` short-circuit

- When all chained `Hooks` impls are `NoopHooks`, return early without
  awaiting.
- Verify via the existing `hooks_events.rs` test suite.

### Tier 5 — Quality polish (~2 PRs, optional)

#### PR-T5-A · App builder

- Replace `App::new(<14 args>)` with `AppBuilder`. Removes the
  `#[allow(clippy::too_many_arguments)]` allow.
- Mechanical refactor; tests carry over.

#### PR-T5-B · `caliban-common::errors::CalibanError`

- Centralize the most-duplicated error variants (`NotFound { what,
  path }`, `Io { source }`, `NoHome`).
- Per-crate `thiserror` enums compose via `#[from]` so existing call
  sites stay typed and printed-as-before.

---

## Execution model

- **Per-PR**: subagent in isolated worktree, ≤300 LOC of net new code
  (most PRs delete more than they add), tests + clippy + fmt all clean,
  matrix-style cleanup-progress ledger updated.
- **Cleanup ledger**: `docs/cleanup-progress.md` tracks LOC delta, dup
  sites consolidated, binary-size delta, test-suite runtime — refreshed
  in the same PR that ships each change.
- **Parallelism**: Tier 1 PRs serialize. Tier 2 splits parallelize (4
  concurrent subagents, each touches one file/crate). Tier 3 and 4
  interleave with conflict-aware scheduling. Tier 5 last.
- **Acceptance per PR**:
  - `cargo build --workspace` clean
  - `cargo clippy --workspace --all-targets -- -D warnings` clean
  - `cargo fmt --all -- --check` clean
  - `cargo test --workspace` green
  - CI green on both default and cloud-feature jobs
  - Ledger updated in same commit

## Cross-cutting risks

| Risk | Mitigation |
|---|---|
| Tier 1 migration touches ~10 consumers in one PR | Run full workspace tests locally before pushing; sub-PR phases if it bloats past 1500 LOC |
| Tier 2 splits race each other on `App` struct definitions | T2-A (tui.rs split) lands first; T2-B (main.rs) rebases atop |
| Tier 3-B `#[deprecated]` triggers `-D warnings` builds | Use `#[allow(deprecated)]` at the call sites that still consume legacy loaders during transition |
| Tier 4 perf changes regress existing tests | Each perf PR runs the full suite + the new baseline-comparison check |
| Auto-rebase chains amplify conflicts | Sequence Tier 2 → 3 → 4 → 5 with at most 3 PRs in flight at a time |

## Acceptance criteria (sprint-level)

- ≥10 PRs merged across tiers 1–4.
- `caliban-common` crate exists; all 8 duplication sites (table above)
  consolidated.
- No source file >1500 LOC anywhere in `crates/` or `caliban/`.
- Release binary smaller (target ~30% reduction with LTO+strip).
- Workspace test suite runs in ≤ baseline + 5% (avoid perf regressions).
- `docs/cleanup-progress.md` records every PR's delta.

## Out of scope (parking lot)

These showed up in the survey but aren't worth a sprint PR:

- Migrating to `bytes::BytesMut` for hot stream buffers (premature).
- Replacing `Arc<Mutex<…>>` with lock-free types globally (case-by-case
  judgment, not a sweep).
- Renaming `caliban-agent-core` to something less generic.
- Reorganizing provider crates into a single `caliban-providers` (would
  break public-facing dep names).
