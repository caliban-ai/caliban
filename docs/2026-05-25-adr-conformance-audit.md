# ADR conformance audit (2026-05-25)

> **What this is.** A documentation-only review of all 40 ADRs in
> `adrs/`. For each ADR it answers two questions:
>
> 1. **Conformance** — does the current code (`main`-ish, branch
>    `jf/docs/adr-conformance-audit`) match what the ADR commits to?
> 2. **Merit** — independently of conformance, is the decision a good
>    one?
>
> **What it isn't.** It is not a replacement for
> `docs/parity-gap-matrix.md` (feature parity vs Claude Code) or the
> spec set under `docs/superpowers/specs/`. Where those documents are
> authoritative I link them and avoid re-stating their content.
>
> **Scope.** ADRs 0001-0040; conformance checked against the workspace
> as of branch HEAD. Source-of-truth references (file paths, crate
> names, env vars) reflect spot-checks done during the audit.

## TL;DR

- **Code conformance is generally strong.** Every "accepted" decision
  has a code home; nearly every "proposed" decision in ADRs 0019-0040
  already has shipped code in the workspace.
- **ADR status hygiene is poor.** The README index lists 22 ADRs as
  `proposed` that are either `accepted` in their own body or whose
  features are marked ✅ in `parity-gap-matrix.md`. See
  [Finding 1](#finding-1-adr-status-drift).
- **Two ADRs deviate from the "new crate" commitment** without
  amending the ADR: `caliban-tui-ask` (ADR 0027) and `caliban-auto-mode`
  (ADR 0029) live as sub-modules in existing crates instead. See
  [Finding 2](#finding-2-two-crate-boundaries-collapsed-without-an-adr-update).
- **Two decisions deserve early revisits.** The 8 KiB memory budget
  (ADR 0018) is conservative against 2026-era context windows; the
  50 ms TUI redraw tick (ADR 0014) is explicitly described in its own
  ADR as masking a root cause rather than fixing it. See
  [Finding 3](#finding-3-aging-defaults-worth-revisiting) and
  [Finding 4](#finding-4-tui-tick-papers-over-a-root-cause).
- **Technical merit is sound across the board.** No ADR rises to
  "questionable decision." Several have known trade-offs the ADR
  itself documents — those are surfaced per-row below, not relitigated.

## Cross-cutting findings

### Finding 1 — ADR status drift

The `Status:` line inside the ADR file disagrees with `adrs/README.md`
for ten ADRs (body says `accepted`, index says `proposed`):

`0025, 0026, 0027, 0029, 0030, 0035, 0036, 0037, 0038, 0040`

A further eleven ADRs say `proposed` in both places but
`docs/parity-gap-matrix.md` marks their corresponding rows ✅:

`0019, 0020, 0021, 0023, 0024, 0028, 0031, 0032, 0033, 0034, 0039`

In other words, 21 of the 40 ADRs are misstated in the index. New
contributors reading `adrs/README.md` will misjudge which decisions
are in effect.

**Action.** A single PR can reconcile the README, fold `accepted`
status into the body where it lags, and add a short "implemented in"
crate pointer to each ADR.

### Finding 2 — Two crate boundaries collapsed without an ADR update

| ADR | Commitment | Reality |
|-----|------------|---------|
| 0027 | "The Ask modal lives in a new `caliban-tui-ask` crate" | `caliban/src/tui/ask.rs` (202 LOC) inside the binary |
| 0029 | "A new Layer-3 crate `caliban-auto-mode`" | `crates/caliban-agent-core/src/{auto_mode,mode_filter,permission_mode}.rs` (~1,750 LOC) inside `caliban-agent-core` |

Both decisions may be correct in retrospect — Ask handling is tiny and
binary-specific; auto-mode is tightly coupled to the permission
pipeline that already lives in agent-core. But the ADR text still
promises a separate crate. Either the ADR should be updated with a
"Revised" note explaining the consolidation, or the code should be
extracted. Drift between ADRs and physical layout makes the workspace
harder to map.

### Finding 3 — Aging defaults worth revisiting

- **8 KiB combined memory prefix (ADR 0018).** Defensible in 2024;
  conservative against 2026 context windows (1M tokens on Sonnet 4.6,
  200K standard on most providers). Truncation-first behavior risks
  dropping the auto-memory index — exactly the tier that grows over
  time. A `cap_tokens` setting is cheap; raising the default to ~32
  KiB is cheaper.
- **5 MiB image pre-base64 cap (ADR 0039).** Mirrors Anthropic's
  documented limit; OpenAI and Google both accept larger payloads.
  Acceptable as a parity choice; flag for a future "per-provider cap"
  knob in `[images]`.
- **256-entry LRU for auto-mode classifier (ADR 0029).** Per-session
  cache means a fresh run starts cold; consider a persisted cache when
  the verdict-input space stabilizes.

### Finding 4 — TUI tick papers over a root cause

ADR 0014 explicitly says of its 50 ms redraw tick: *"If the
underlying cause is something deeper (e.g., a missing waker in
`async_stream::try_stream!`), these fixes mask the symptom rather
than addressing the root cause."* The debug log is the diagnostic.
Two years on, the redraw tick is still in place; no follow-up ADR
records whether the root cause was identified. Worth a short
follow-up — either a clean bill of health, or a real fix.

### Finding 5 — `is_parallel_safe()` deferral is reasonable but rotting

ADR 0016 deferred a per-tool concurrency-safety flag, observing that
no current built-in has write contention. That observation is still
true for `Bash`/`Read`/`Grep`/`Glob`. It is *less* true for the new
`Edit`/`Write`/`NotebookEdit`/`MultiEdit`/`WriteMemoryTopic` surface
introduced by ADRs 0028 + 0035 — two `Edit`s on the same file in one
turn would interleave non-deterministically. Sub-agent worktree
isolation (ADR 0037) sidesteps this *between* agents but not *within*
one turn. Worth re-examining now that the write surface has tripled.

### Finding 6 — AGPL-3.0-only forecloses library-shaped reuse

ADR 0003 is intentional and the rationale is sound for caliban as an
end product. But the workspace now contains crates that *look like*
reusable libraries (`caliban-provider`, `caliban-mcp-client`,
`caliban-model-router`, `caliban-images`, `caliban-telemetry`). Two
options for the future, neither blocking:

- Stay AGPL across the board; document that reuse means consumers
  AGPL too. This is the current de-facto policy.
- Carve out a few "protocol" crates (the IR + transport surface in
  particular) under a permissive license, allowing third-party
  adapters without infecting the rest. Would need a CLA framework.

No action recommended; flag so the choice is conscious if the question
arises.

### Finding 7 — No ADR for the bin/CLI split itself

The workspace declares one binary (`caliban`) and one daemon
(`caliband`) under `crates/caliban-supervisor/src/bin/`. ADR 0037
introduces the daemon obliquely. There is no ADR establishing
"caliband is a separate binary in the same workspace" — operationally
this is the right call, but it's worth one paragraph in the next ADR
sweep so future contributors don't relitigate it.

---

## Per-ADR audit

The table below is a one-line summary. Per-ADR details follow.

| # | Title | Body status | Conformance | Merit | Notes |
|---|-------|-------------|-------------|-------|-------|
| 0001 | Async runtime → `tokio` | accepted | ✅ | Sound | — |
| 0002 | Error model → `thiserror`/`anyhow` | accepted | ✅ | Sound | — |
| 0003 | License → `AGPL-3.0-only` | accepted | ✅ | Sound w/ caveats | See [Finding 6](#finding-6-agpl-30-only-forecloses-library-shaped-reuse) |
| 0004 | Naming → `caliban-*` libs, `caliban` bin | accepted | ✅ | Sound | — |
| 0005 | Workspace layout | accepted | ✅ | Sound | 25 members; grouping subdirs may be warranted soon |
| 0006 | Message schema → provider-neutral IR | accepted | ✅ | Sound | Anthropic-shaped IR — see row notes |
| 0007 | Transport trait per schema family | accepted | ✅ | Sound | — |
| 0008 | `Role::System` positional | accepted | ✅ | Sound | — |
| 0009 | Agent-core design | accepted (partial supersede) | ✅ | Sound | Sequential-tools clause superseded by 0016 |
| 0010 | WorkspaceRoot + restricted mode | accepted | ✅ | Sound | — |
| 0011 | JSON sessions + REPL | accepted | ✅ | Sound | REPL replaced by TUI in 0012 |
| 0012 | TUI via ratatui | accepted | ✅ | Sound | — |
| 0013 | TUI overlays + layout v2 | accepted | ✅ | Sound | — |
| 0014 | System prompt + stall fix + debug log | accepted | ✅ | Sound w/ caveats | See [Finding 4](#finding-4-tui-tick-papers-over-a-root-cause) |
| 0015 | Context + tilde expansion | accepted | ✅ | Sound | — |
| 0016 | Parallel tool dispatch | accepted | ✅ | Sound w/ caveats | See [Finding 5](#finding-5-is_parallel_safe-deferral-is-reasonable-but-rotting) |
| 0017 | MCP stdio v1 | accepted | ✅ | Sound | Superseded in scope by 0023 |
| 0018 | Memory tier model | accepted | ✅ | Sound w/ caveats | See [Finding 3](#finding-3-aging-defaults-worth-revisiting) |
| 0019 | Skills loading | proposed | ✅ | Sound | Status drift — see [Finding 1](#finding-1-adr-status-drift) |
| 0020 | Permission rules | proposed | ✅ | Sound | Status drift |
| 0021 | Sub-agent primitive | proposed | ✅ | Sound | Status drift; v1 sync; async-Task is 0037 |
| 0022 | Model routing v1 | accepted | ✅ | Sound | — |
| 0023 | MCP v2 | proposed | ✅ | Sound | Status drift; all three phases shipped |
| 0024 | Hook event taxonomy | proposed | ✅ | Sound | Status drift; events implemented |
| 0025 | Headless / print mode | accepted | ✅ | Sound | README index lags |
| 0026 | Settings hierarchy | accepted | ✅ | Sound w/ caveats | README index lags; intricate merge rules |
| 0027 | TUI ergonomics | accepted | 🟡 | Sound | **Crate boundary deviates** — see [Finding 2](#finding-2-two-crate-boundaries-collapsed-without-an-adr-update) |
| 0028 | Checkpointing + `/rewind` | proposed | ✅ | Sound | Status drift |
| 0029 | Permission modes + auto-mode | accepted | 🟡 | Sound | **Crate boundary deviates** — see [Finding 2](#finding-2-two-crate-boundaries-collapsed-without-an-adr-update) |
| 0030 | Plugin packaging | accepted | ✅ | Sound w/ caveats | No signature verification in v1 |
| 0031 | Output styles | proposed | ✅ | Sound | Status drift |
| 0032 | OS sandbox | proposed | ✅ | Sound w/ caveats | Seatbelt deprecated by Apple; no built-in proxy |
| 0033 | OTel + cost | proposed | ✅ | Sound | Status drift; rate-card maintenance is hand-driven |
| 0034 | Bedrock + Vertex providers | proposed | ✅ | Sound | Status drift |
| 0035 | Auto-memory | accepted | ✅ | Sound w/ caveats | README index lags; no automatic pruning |
| 0036 | CLAUDE.md ancestry + imports | accepted | ✅ | Sound | README index lags |
| 0037 | Subagent isolation + fleet | accepted | ✅ | Sound | README index lags |
| 0038 | Model router v2 | accepted | ✅ | Sound | README index lags |
| 0039 | Image / vision input | proposed | ✅ | Sound | Status drift |
| 0040 | Slash command registry | accepted | ✅ | Sound | README index lags |

Legend: ✅ full conformance · 🟡 conforms in substance, deviates from a
specific commitment · 🔴 not implemented (no rows currently).

### 0001 — Async runtime → `tokio`

- **Conformance:** ✅. Workspace pins `tokio` with `features =
  ["full"]`; member crates use `tokio.workspace = true`.
- **Merit:** Sound. The "no nested runtimes" rule is the most
  load-bearing clause and is observed. No 2026 reason to revisit.

### 0002 — Error model → `thiserror`/`anyhow`

- **Conformance:** ✅. Each `caliban-*` crate exposes a local `Error`
  enum; the binary uses `anyhow`.
- **Merit:** Sound. Per-crate error enums have stayed local; no
  "uber error" crate emerged. The forward-looking risk the ADR called
  out (shared `Cancelled`/`Timeout`) is now real in spots — checkpoint,
  supervisor, and headless all model cancellation locally — but the
  duplication remains tractable.

### 0003 — License → `AGPL-3.0-only`

- **Conformance:** ✅. `LICENSE` is AGPL-3.0; every Cargo.toml uses
  `license.workspace = true`.
- **Merit:** Sound w/ caveats. The choice is correct for caliban as
  an end product. See [Finding 6](#finding-6-agpl-30-only-forecloses-library-shaped-reuse)
  for the library-shaped-reuse consideration.

### 0004 — Naming → `caliban-*` libs, `caliban` bin

- **Conformance:** ✅. All 24 library crates use the `caliban-` prefix;
  the binary is `caliban`.
- **Merit:** Sound. Module-path terseness convention is observed
  consistently.

### 0005 — Workspace layout

- **Conformance:** ✅. Libraries live under `crates/`; the `caliban`
  binary sits at the workspace root. The `caliband` binary is nested
  under `crates/caliban-supervisor/src/bin/` (consistent with the
  "binaries at root" rule's intent — `caliband` is *secondary*).
- **Merit:** Sound. The ADR itself flagged ">25 crates" as the
  threshold for grouping subdirectories — we're at 24 + 2 binaries.
  Worth thinking about `crates/layer-N/` grouping ahead of the next
  major addition (e.g., a hypothetical `caliban-orchestrator/`).

### 0006 — Message schema → provider-neutral IR

- **Conformance:** ✅. `caliban-provider` exports the IR; each adapter
  translates at its boundary.
- **Merit:** Sound. The "intentionally close to Anthropic shape"
  trade-off carries some risk as OpenAI and Google ship new modalities
  (audio input, structured tool-result attachments) that don't map
  cleanly. The IR has held up so far; the `Image` variant from ADR
  0039 was clean. No action.

### 0007 — Transport trait per schema family

- **Conformance:** ✅. Each schema-family crate exposes its own
  `Transport` trait; Bedrock/Vertex transports are feature-gated in
  `caliban-provider-anthropic` and re-wrapped by the dedicated
  provider crates from ADR 0034.
- **Merit:** Sound. The per-family scope explicitly avoids a leaky
  cross-family transport abstraction; the trade-off is acknowledged
  in the ADR.

### 0008 — `Role::System` positional

- **Conformance:** ✅. Single canonical representation enforced;
  adapters serialize per their family.
- **Merit:** Sound.

### 0009 — Agent-core design

- **Conformance:** ✅. `stream_until_done` is the single source of
  truth; `NoopCompactor` is default; retry classifier matches the
  closed list. Sequential-tool clause is explicitly superseded by 0016.
- **Merit:** Sound. The retry classifier's static membership has held
  up; the only friction is ADR 0038's "fatal-for-route" list (a
  superset of the retryable list), which lives in
  `caliban-model-router`. That's the right layer for it.

### 0010 — WorkspaceRoot + restricted mode

- **Conformance:** ✅. `WorkspaceRoot::resolve` does `~` expansion
  (per ADR 0015) and the permissive/restricted split is in place.
- **Merit:** Sound. The default-permissive choice is correct for the
  personal-use context.

### 0011 — JSON sessions + REPL

- **Conformance:** ✅. JSON sessions live under `caliban-sessions`;
  the rustyline REPL was replaced by the TUI in ADR 0012 — the
  decision is partly historical now. Session format is unchanged.
- **Merit:** Sound. The SQLite-as-future-option escape hatch is
  documented; with image blob storage from ADR 0039 already needing a
  side-table-style `<session>/blobs/` directory, a future move makes
  more sense than it did in 2024.

### 0012 — TUI via ratatui

- **Conformance:** ✅. `caliban/src/tui.rs` + `caliban/src/tui/`.
- **Merit:** Sound.

### 0013 — TUI overlays + layout v2

- **Conformance:** ✅. Overlay system is in place; `/help`/`/config`/
  `/mcp`/`/skills` overlays are present plus many more added since
  (rewind, transcript, ask, plugin, etc.).
- **Merit:** Sound. The "static read-only v1" constraint has been
  selectively relaxed (config overlay shows scope provenance, etc.) —
  consistent with the ADR's "revisit if" clause.

### 0014 — System prompt + stall fix + debug log

- **Conformance:** ✅ for the system prompt path
  (`caliban/src/system_prompt.rs`); ✅ for the debug log; ✅ for the
  50 ms tick.
- **Merit:** Sound w/ caveats. See
  [Finding 4](#finding-4-tui-tick-papers-over-a-root-cause).

### 0015 — Context + tilde expansion

- **Conformance:** ✅.
- **Merit:** Sound. The "two-copies" risk (App + session) the ADR
  flagged hasn't bitten — but with ADR 0028 now persisting per-prompt
  manifests *also* keyed by message index, the long-term refactor the
  ADR sketched (App holds `Arc<RwLock<Session>>`) is worth
  re-evaluating.

### 0016 — Parallel tool dispatch

- **Conformance:** ✅. `parallel_tools` field on `AgentBuilder`, the
  semaphore, the serial `before_tool` gate, and completion-order
  emission all match the ADR.
- **Merit:** Sound w/ caveats. See
  [Finding 5](#finding-5-is_parallel_safe-deferral-is-reasonable-but-rotting).

### 0017 — MCP stdio v1

- **Conformance:** ✅ in spirit. The crate now ships stdio + HTTP +
  SSE per ADR 0023, so 0017's "stdio only" clause is explicitly
  superseded.
- **Merit:** Sound. v1 shipping stdio-only kept the dep footprint
  small until real demand for HTTP transports landed.

### 0018 — Memory tier model

- **Conformance:** ✅. `caliban-memory` owns tier discovery,
  sanitization, splicing, and budget enforcement; agent-core does not
  depend on it. ADR 0036 extended the project tier; ADR 0035 made
  auto-memory writable.
- **Merit:** Sound w/ caveats. The 8 KiB combined cap is the most
  pressing default to revisit — see
  [Finding 3](#finding-3-aging-defaults-worth-revisiting).

### 0019 — Skills loading

- **Conformance:** ✅. `caliban-skills` exists with `SkillTool` +
  loader; the auto-memory skill body (ADR 0035) is bundled here.
- **Merit:** Sound. The "description-list bloat" risk grows with
  skill count; not yet acute. README status is stale.

### 0020 — Permission rules

- **Conformance:** ✅. `caliban-agent-core::permissions` has the rule
  grammar; the `PermissionsHook` is in place; mode composition lands
  via 0029.
- **Merit:** Sound. The "first-arg prefix glob" surprise risk is
  acknowledged and the TUI surfaces the matched rule.

### 0021 — Sub-agent primitive

- **Conformance:** ✅. `AgentTool` is in `caliban-tools-builtin`;
  recursion guard, allowlist semantics, ~5000-char truncation present.
- **Merit:** Sound. The synchronous-only constraint is now relaxed
  via ADR 0037's `bg = true`, exactly along the "revisit if" path.

### 0022 — Model routing v1

- **Conformance:** ✅. `caliban-model-router` is in place; routes are
  matched by `RequestPurpose`; the router is itself a `Provider`.
- **Merit:** Sound. The "operator-defined policy" stance is the
  signature differentiator from Claude Code; the v2 work (ADR 0038)
  consciously preserved it.

### 0023 — MCP v2 — transports, OAuth, elicitation, resources

- **Conformance:** ✅. `oauth.rs`, `elicitation.rs`, `resource.rs`,
  `client.rs` (with `Transport::Http`/`Sse` arms) all present. Phase
  C is shipped per `parity-gap-matrix.md`.
- **Merit:** Sound. The "loopback OAuth assumes a browser" trade-off
  is acknowledged; a paste-back fallback for hardened workstations
  remains in the "revisit if" bucket.

### 0024 — Hook event taxonomy

- **Conformance:** ✅. All first-class events from the ADR
  (`session_start`, `session_end`, `user_prompt_submit`,
  `pre_compact`/`post_compact`, `config_change`, `cwd_changed`,
  `file_changed`, `subagent_start`/`stop`, `permission_request`/
  `permission_denied`) exist as `Hooks` trait methods.
- **Merit:** Sound. URL allowlist + managed-only-mode are good
  safety knobs given the "shell hooks are arbitrary code execution"
  reality.

### 0025 — Headless / print mode + JSON output

- **Conformance:** ✅. `caliban/src/headless/` exists with
  `cli.rs`/`mod.rs`/`events.rs`/`schema.rs`/`hooks_sink.rs`/`budget.rs`.
  Stream-json formats and exit-code table match.
- **Merit:** Sound. Pricing-table staleness is the documented risk;
  monthly refresh discipline is the unavoidable cost.

### 0026 — Settings hierarchy + `/config`

- **Conformance:** ✅. `caliban-settings` exposes `scope.rs`,
  `loader.rs`, `merge.rs`, `overlay.rs`, `watcher.rs`,
  `api_key_helper.rs`, `compat.rs` (legacy per-feature TOMLs),
  `schema.json`/`schema.rs`. README index lags.
- **Merit:** Sound w/ caveats. The merge-rule complexity (8-row
  table) is intricate; the Effective tab surfaces provenance per key,
  which is the right mitigation.

### 0027 — TUI ergonomics

- **Conformance:** 🟡. Every behavior (shell escape, external editor,
  Ask modal, transcript viewer, reverse history, file-suggestion
  trait) is in place. **The Ask modal does not live in a separate
  `caliban-tui-ask` crate** as the ADR commits to — it lives in
  `caliban/src/tui/ask.rs` (202 LOC) inside the binary. See
  [Finding 2](#finding-2-two-crate-boundaries-collapsed-without-an-adr-update).
- **Merit:** Sound. The InputMode-fattening pattern has held up
  under the additions from ADRs 0028 (rewind) and 0039 (image paste).

### 0028 — Checkpointing + `/rewind`

- **Conformance:** ✅. `caliban-checkpoint` ships `manifest.rs`,
  `recorder.rs`, `restore.rs`, `prune.rs`, `store.rs`, `hook.rs`.
  Storage layout under `<root>/projects/<sanitized-cwd>/checkpoints/`
  matches Claude Code; override env `CALIBAN_CHECKPOINT_ROOT`.
  `before_run`/`after_run` hooks exist.
- **Merit:** Sound. Bash exclusion is intractable to fix; the rewind
  menu calling it out is the right UX.

### 0029 — Permission modes + auto-mode classifier

- **Conformance:** 🟡 in shape. `PermissionMode`,
  `SharedPermissionMode`, `ModeFilter` exist; the
  `AutoModeClassifier` exists, dispatches via
  `RequestPurpose::FastClassifier`, caches via sha256-keyed LRU, and
  routes `soft_deny` to the Ask modal. **The implementation lives
  inside `caliban-agent-core`, not in a dedicated `caliban-auto-mode`
  crate** as the ADR commits to (~1,750 LOC across `auto_mode.rs`,
  `mode_filter.rs`, `permission_mode.rs`). See
  [Finding 2](#finding-2-two-crate-boundaries-collapsed-without-an-adr-update).
- **Merit:** Sound. Operator-defined classifier model is signature
  differentiation; the static rule pre-pass keeps the median call
  off the network.

### 0030 — Plugin packaging

- **Conformance:** ✅. `caliban-plugins` ships `cli.rs`, `manager.rs`,
  `manifest.rs`, `expand.rs`, `marketplace.rs`, `trust.rs`,
  `overlay.rs`, `loaded.rs`. `${CALIBAN_PLUGIN_ROOT}` and
  `${CLAUDE_PLUGIN_ROOT}` aliases honored.
- **Merit:** Sound w/ caveats. Signature verification is explicitly
  deferred — `manifest_sha256` + URL trust is the v1 trust model.
  Promoting cosign/minisign should land before community marketplaces
  proliferate.

### 0031 — Output styles

- **Conformance:** ✅. `caliban-output-styles` exists with built-ins
  `default.md`, `proactive.md`, `explanatory.md`, `learning.md`;
  splice composes with `MemoryPrefix`.
- **Merit:** Sound. Frontmatter parsing duplicated with
  `caliban-skills` is acknowledged — extract a `frontmatter` helper
  in `caliban-common` when a third consumer appears (ADR 0030's
  plugin manifest is JSON, not frontmatter, so it doesn't count).

### 0032 — OS sandbox

- **Conformance:** ✅. `caliban-sandbox` exists; macOS + Linux/WSL
  backends ship per the matrix; Windows is documented-deferred.
- **Merit:** Sound w/ caveats. Seatbelt's Apple-deprecation status is
  the real long-term risk; the ADR's mitigation (re-evaluate when
  Apple removes it) is the only realistic plan. The lack of an
  in-tree network-egress proxy is documented as a v1.1 follow-up; no
  external proxy is bundled.

### 0033 — OTel export + cost accounting

- **Conformance:** ✅. `caliban-telemetry` ships with
  `CALIBAN_ENABLE_TELEMETRY` master switch, `OTEL_*` env contract,
  `rust_decimal` cost math, and the `caliban.*` metric names listed
  in the matrix. Rates YAML vendored at
  `crates/caliban-telemetry/rates.yaml`.
- **Merit:** Sound. Rate-card maintenance is the only ongoing cost.

### 0034 — Bedrock + Vertex providers

- **Conformance:** ✅. `caliban-provider-bedrock` and
  `caliban-provider-vertex` each ship `auth.rs`, `config.rs`,
  `models.rs`, `lib.rs`, `error.rs`. Per the ADR, both wrap
  `AnthropicProvider<XTransport>` rather than forking the IR adapter.
- **Merit:** Sound. The "don't extend `caliban-provider-anthropic`"
  rationale (control-plane creep) has held up.

### 0035 — Auto-memory

- **Conformance:** ✅. `ReadMemoryTopicTool` + `WriteMemoryTopicTool`
  in `caliban-tools-builtin`; on-disk layout `~/.caliban/projects/<sanitized-cwd>/memory/`;
  atomic writes via tempfile + rename. README index lags.
- **Merit:** Sound. No automatic pruning is a known trade-off the
  ADR documents. The 200-line / 25 KB index cap is the most likely
  near-term re-tune.

### 0036 — CLAUDE.md ancestor walk + `@`-imports

- **Conformance:** ✅. `caliban-memory::loader::walk_ancestors`
  exists; `claude_md_excludes`; rules; nested-on-demand all
  documented in code. README index lags.
- **Merit:** Sound. Monotone prompt-growth is the most subtle
  trade-off; interacts with [Finding 3](#finding-3-aging-defaults-worth-revisiting)'s
  budget concern.

### 0037 — Subagent worktree isolation + background fleet

- **Conformance:** ✅. `caliban-worktrees` (`manager.rs`, `sparse.rs`,
  `symlinks.rs`); `caliban-supervisor` (`bin/`, `client.rs`,
  `server.rs`, `proto.rs`, `registry.rs`, `runtime.rs`, `store.rs`);
  `caliban agents` CLI. README index lags.
- **Merit:** Sound. The per-repo daemon model is conservative; the
  ADR's "revisit if cross-repo gets common" clause is the right exit
  ramp.

### 0038 — Model router v2

- **Conformance:** ✅. `caliban-model-router` ships `fallback.rs`,
  `hedging.rs`, `breaker.rs`, `capabilities.rs`, `discovery.rs`,
  `effort.rs`, `resolver.rs`, `dispatch.rs`. README index lags.
- **Merit:** Sound. Hedging-as-opt-in default protects the median
  bill; the cross-route prompt-cache marker clearing is the right
  trade against silent cache thrash.

### 0039 — Image / vision input

- **Conformance:** ✅. `caliban-images` exists; IR has `Image` variant
  (referenced by ADR text); strict-routing fallback present;
  graphics-protocol detection per matrix. Status drift.
- **Merit:** Sound. The MIME allowlist (png/jpeg/gif/webp) is the
  right CVE-reduction default; promotion of avif/heic should follow
  ecosystem readiness.

### 0040 — Slash command registry

- **Conformance:** ✅. `caliban/src/tui/slash/` contains the trait,
  registry, and per-group impls (`basic`, `observe`, `config`,
  `model`, `perms`, `session`, `dx`, `existing`). README index lags.
- **Merit:** Sound. `SlashCtx`-as-god-object is acknowledged; the
  ADR's split threshold (~20 fields) is a sane heuristic.

---

## Recommended follow-up work

In rough priority order. None are blocking.

1. **Reconcile ADR status drift.** Update `adrs/README.md`'s status
   column from the bodies and the parity matrix. Optionally add a
   small "implemented in" pointer to each ADR.
2. **Decide on the two crate-boundary deviations** ([Finding 2](#finding-2-two-crate-boundaries-collapsed-without-an-adr-update)).
   Either amend ADRs 0027 + 0029 with a "Revised" note explaining
   why the crate wasn't created, or extract `caliban-tui-ask` and
   `caliban-auto-mode` for real.
3. **Revisit the 8 KiB memory budget** ([Finding 3](#finding-3-aging-defaults-worth-revisiting)).
   Either bump the default or add a per-scope `cap_tokens` knob.
4. **Close out the 50 ms TUI tick mystery** ([Finding 4](#finding-4-tui-tick-papers-over-a-root-cause)).
   Use the debug log to confirm stalls are gone; either remove the
   tick or document why it stays.
5. **Reconsider `is_parallel_safe()`** ([Finding 5](#finding-5-is_parallel_safe-deferral-is-reasonable-but-rotting))
   now that the write surface includes `Edit`/`Write`/`MultiEdit`/
   `NotebookEdit`/`WriteMemoryTopic`.
6. **Sweep for missing ADRs.** No ADR records (a) the `caliband`
   sibling-binary decision, (b) the choice to use `arc-swap` for
   shared state, (c) the choice to use `rmcp` 1.7-line specifically.
   One paragraph each prevents relitigation.
