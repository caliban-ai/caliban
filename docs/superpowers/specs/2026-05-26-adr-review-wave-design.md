# ADR review wave — Design

**Date:** 2026-05-26
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**Branch:** `jf/docs/adr-conformance-audit`
**ADR:** *(no new architectural decision; this PR amends existing ADRs and writes three small missing ones)*

## Goal

A previous session produced a 579-line conformance + merit audit of all 40
ADRs (`docs/2026-05-25-adr-conformance-audit.md`) and matching resume notes
(`docs/adr-audit-resume-state.md`). Both are untracked. The audit surfaced 7
cross-cutting findings, all non-blocking.

This spec consolidates the audit + its follow-ups into a **single PR** that:

1. Commits the audit docs.
2. Reconciles the 21 ADR-status mismatches in `adrs/README.md`.
3. Amends two ADRs whose committed crate boundary was collapsed into an
   existing crate (0027, 0029).
4. Raises the memory cap default and adds a per-scope `cap_tokens` knob.
5. Re-introduces `parallel_conflict_key()` on the tool trait, gating
   same-target write collisions.
6. Closes out the ADR 0014 50 ms TUI tick — via real root-cause fix if
   findable in a time-box, otherwise via a close-out ADR.
7. Writes three short ADRs covering decisions made in code but never
   recorded (`caliband` sibling-binary placement, `arc-swap`, `rmcp` pin).

The audit itself is unchanged from the previous session beyond fixing two
stale references (branch parent + a side-note about a commit on the prior
parent branch).

## Non-goals

- **No re-running the audit.** The findings stand. Spot-check three of the
  21 "status drift" ADRs before publishing the headline number, but don't
  re-verify the whole 40-ADR pass.
- **No optional "Implemented in:" pointer added to every ADR.** Mechanical,
  ~40 file edits, can be a separate cleanup.
- **No image-cap / classifier-LRU changes** from Finding 3. The audit
  flagged them as "no action recommended"; honor that.
- **No license re-evaluation** from Finding 6. The audit explicitly flagged
  Finding 6 as "no action recommended; flag the choice."
- **No new features.** The code work here (memory cap, conflict key, maybe
  a TUI fix) is each scoped to <1 crate of impact.
- **No parity-matrix expansion.** Tick the relevant rows; don't add new ones.

## Branch + commit strategy

Single branch `jf/docs/adr-conformance-audit`, rebased onto current `main`
(commit `e995267`). Multiple atomic commits so the reviewer can read it
section-by-section. Each commit is independently revertable.

Tentative commit sequence (numbers may shift if Finding 4 produces no fix
commit, only an ADR):

1. `docs: ADR conformance audit (2026-05-25)` — Section 1 (audit + resume
   notes, footer/anchor fixes applied)
2. `docs(adrs): reconcile README status column with bodies and parity matrix`
   — Section 2 (Finding 1)
3. `docs(adr-0027): note Ask modal consolidation in caliban binary`
   — Section 3a (Finding 2a)
4. `docs(adr-0029): note auto-mode consolidation in caliban-agent-core`
   — Section 3b (Finding 2b)
5. `feat(memory): raise default cap to 32 KiB + add per-scope cap_tokens`
   — Section 4 (Finding 3)
6. `feat(tools): per-target parallel_conflict_key gates same-file write collisions`
   — Section 5 (Finding 5)
7. `fix(tui): <root-cause one-liner>` **or**
   `docs(adr-XXXX): close out 50 ms redraw tick` — Section 6 (Finding 4)
8. `docs(adr-XXXX): caliband sibling-binary placement` — Section 7a
9. `docs(adr-XXXX): arc-swap as shared-state primitive` — Section 7b
10. `docs(adr-XXXX): rmcp 1.7 version pin` — Section 7c

ADR numbers assigned at write time, starting from the current next-free
slot (probably 0041 if no concurrent additions; verify before writing).

---

## Section 1 — Audit doc fixes (pre-commit)

Two small edits to the existing untracked files before `git add`:

### 1.1 `docs/adr-audit-resume-state.md`
- Replace `"Branch: jf/docs/adr-conformance-audit (off jf/fix/lmstudio-followups)"`
  with `"Branch: jf/docs/adr-conformance-audit (off main, rebased 2026-05-26)"`.
- Remove the entire "Side note from the previous session" trailer (commit
  `8c05e6f` is no longer relevant; the branch is now off `main` directly).
- Fix dead anchor links: replace
  `[Finding 1](#headline-findings-mirror-of-tldr-in-the-audit)` etc. with
  `[Finding 1](2026-05-25-adr-conformance-audit.md#finding-1-adr-status-drift)`
  (and analogous for findings 2-5).

### 1.2 `docs/2026-05-25-adr-conformance-audit.md`
- Spot-check 3 of the 21 "status drift" ADRs before publishing the headline
  number. Pick one from each cluster: one from the "body says accepted,
  index says proposed" group (e.g., 0025), one from the "proposed
  everywhere but parity ✅" group (e.g., 0019), and one randomly (e.g.,
  0033). If any are wrong, correct the headline and the per-row notes.
- No other changes expected; the audit body is final.

---

## Section 2 — Finding 1: README status reconciliation

Pure docs change in `adrs/README.md`.

### Scope
- For each of the 10 ADRs in the audit's first cluster (`0025, 0026, 0027,
  0029, 0030, 0035, 0036, 0037, 0038, 0040`): set the README status column
  to `accepted` (matching the ADR body).
- For each of the 11 ADRs in the audit's second cluster (`0019, 0020,
  0021, 0023, 0024, 0028, 0031, 0032, 0033, 0034, 0039`): also update
  README **and** flip the body `Status:` line from `proposed` → `accepted`,
  since `parity-gap-matrix.md` shows them as shipped.
- Verification: after the edits, no ADR's body status should disagree with
  the README; and no `accepted` ADR should have an unshipped row in the
  parity matrix.

### What this does NOT touch
- The body of any ADR beyond the `Status:` line.
- The References section of any ADR.
- The parity matrix itself.

---

## Section 3 — Finding 2: Two ADR amendments

Append a `## Revised 2026-05-26` section to ADRs 0027 and 0029. Additive
only — the original "Decision" section stays put so git history of the
original choice remains intact.

### 3.1 `adrs/0027-tui-ergonomics.md` revision body

> ### Revised 2026-05-26
>
> The original Decision committed the Ask modal to a new `caliban-tui-ask`
> crate. In practice the modal shipped at `caliban/src/tui/ask.rs` (~202
> LOC) inside the binary.
>
> **Why this is the correct outcome.** The modal is binary-coupled (it
> consumes the binary's `App` state, dispatches via the binary's `Action`
> enum, and renders into the binary's overlay system). Extracting it would
> require either threading App/Action/overlay traits through a public
> surface or duplicating them — both costs without payoff. The "extract
> when sharable" trigger from the original Decision never fired.
>
> **Revisit if** another consumer needs the modal (e.g., a hypothetical
> standalone `caliban-tui` library separated from the binary), or LOC
> grows past ~500.

### 3.2 `adrs/0029-permission-modes.md` revision body

> ### Revised 2026-05-26
>
> The original Decision committed `caliban-auto-mode` to be a new Layer-3
> crate. In practice the implementation lives inside `caliban-agent-core`
> across `auto_mode.rs`, `mode_filter.rs`, and `permission_mode.rs`
> (~1,750 LOC combined).
>
> **Why this is the correct outcome.** Auto-mode dispatch is tightly
> coupled to the permission pipeline (`PermissionsHook`,
> `SharedPermissionMode`, the soft-deny → Ask handshake) which already
> lives in agent-core. Extracting auto-mode would either pull most of the
> permission pipeline out with it or introduce a circular dep. The static
> rule pre-pass, the classifier dispatch, and the LRU cache all live next
> to the data they need.
>
> **Revisit if** auto-mode grows a second consumer (e.g., a non-agent
> classifier client), or if the dispatch path becomes a measurable
> compile-time burden on `caliban-agent-core`.

### What this does NOT touch
- No code moves.
- No `Status:` line change (both ADRs already say `accepted` in body
  post-Section 2).

---

## Section 4 — Finding 3: Memory cap + per-scope knob

Real code change in `caliban-memory`.

### 4.1 Pre-work
- Locate the 8 KiB constant (likely `caliban-memory/src/budget.rs` or
  similar — verify at implementation time). Note all call sites.
- If the constant is referenced from >2 sites with no intermediate type,
  introduce a `MemoryBudget` struct to centralize. Otherwise just change
  the value.

### 4.2 Default change
- Bump combined-prefix default from **8 KiB → 32 KiB**.

### 4.3 Settings schema addition
Add to `caliban-settings`' schema (and `schema.json`):

```toml
[memory]
# Per-scope token caps. All optional; missing = use combined default.
cap_tokens_auto       = 16384  # auto-memory tier
cap_tokens_claude_md  = 16384  # CLAUDE.md tier
cap_tokens_combined   = 32768  # hard ceiling across all tiers
```

Settings merge follows existing precedence (project > user > defaults).

### 4.4 Behavior
- Each tier's budget is `min(per-scope cap, combined remaining)`.
- Truncation order is unchanged from current behavior (defined by ADR 0018).
- If `cap_tokens_combined` is set lower than the sum of per-tier caps,
  per-tier caps are scaled down proportionally — no silent dropping.

### 4.5 Tests
- Default value is 32 KiB combined when no settings are present.
- Per-scope override observed (e.g., user sets `cap_tokens_auto = 4096`).
- Combined cap acts as a ceiling: if set below per-tier sum, per-tier
  caps scale proportionally.
- Truncation at the new limit still drops in ADR-0018 order.
- Settings JSON-schema validation accepts the new keys and rejects
  non-integer values.

### 4.6 Doc updates
- Update parity-matrix row(s) for memory if any reference 8 KiB.
- Update ADR 0018 with a `## Revised 2026-05-26` section recording the
  default change + knob addition. (One additional ADR amendment beyond
  Section 3 — fits in the same commit as the code change.)
- Update any spec under `docs/superpowers/specs/` that pins 8 KiB.

---

## Section 5 — Finding 5: per-target conflict keys

Real code change in `caliban-tools-builtin` (the tool trait) and
`caliban-agent-core` (the parallel dispatcher).

### 5.1 Trait extension
Add to the tool trait:

```rust
/// Returns Some(key) if this tool call has a conflict identity that must
/// not run in parallel with another call using the same key. Returns None
/// when the tool is fully parallel-safe (the default).
fn parallel_conflict_key(&self, input: &ToolInput) -> Option<String> {
    None
}
```

Default `None` preserves existing behavior for every tool that doesn't
override.

### 5.2 Per-tool overrides

| Tool | Key |
|---|---|
| `Edit`, `Write`, `MultiEdit` | canonicalized absolute path of the target file |
| `NotebookEdit` | canonicalized absolute path of the notebook |
| `WriteMemoryTopic` | `memory:{scope}:{topic}` (no filesystem path; scope+topic identifies the file) |
| `Read`, `Grep`, `Glob`, `Bash`, all others | `None` (unchanged) |

### 5.3 Path canonicalization
- Use `std::fs::canonicalize` first.
- On failure (e.g., `Write` creating a new file): canonicalize the parent
  directory, then join the file name.
- On Windows: known minor gap — case-insensitive collisions may not key
  identically if neither file exists yet. Documented; not fixed in this PR.

### 5.4 Dispatcher change
In `caliban-agent-core` parallel dispatch:

1. Group tool calls in the batch by `parallel_conflict_key()`.
2. The `None` group runs fully in parallel.
3. Each non-`None` key group runs serially within itself (preserving
   submission order).
4. Groups run in parallel against each other.

The existing `parallel_tools` semaphore is preserved and is acquired
**per individual tool call**, not per group — so the global concurrency
ceiling is unchanged. A serialized key-group still acquires the semaphore
once per step; a parallel `None` group acquires it once per concurrent
call.

This is a minimal change to the existing dispatcher — it adds a grouping
pass before the existing parallel `join` and serializes within each
key-group via sequential `await`.

### 5.5 Tests
- `Edit(a.rs) + Edit(b.rs)` runs in parallel (assert via instrumented
  dispatcher — record start timestamps, confirm overlap).
- `Edit(a.rs) + Edit(a.rs)` runs serially in submission order.
- `Edit(a.rs) + Read(a.rs)` runs in parallel (Read returns `None`; we
  accept the read may observe pre- or post-edit state — same as today).
- Symlink test: `Edit(real_path)` + `Edit(symlink_to_real_path)` runs
  serially (canonicalization collapses).
- `Write` creating new file + existing `Edit` on same path: serial
  (parent-dir canonicalize fallback works).
- `WriteMemoryTopic(user, foo) + WriteMemoryTopic(user, foo)` serial;
  different topics parallel.
- `Edit + WriteMemoryTopic` always parallel (different key namespaces).

### 5.6 ADR amendment
Append `## Revised 2026-05-26` to ADR 0016:

> ### Revised 2026-05-26
>
> The original Decision deferred a per-tool `is_parallel_safe()` flag,
> noting that no built-in had write contention. That observation was true
> in 2024 (Bash/Read/Grep/Glob). It is no longer true: ADRs 0028 + 0035
> introduced Edit/Write/MultiEdit/NotebookEdit/WriteMemoryTopic, all of
> which can collide on the same target within one turn.
>
> **Revised mechanism:** `parallel_conflict_key(&self, input) -> Option<String>`
> on the tool trait. Returns `None` for fully parallel-safe tools (the
> default; matches the original 2024 posture). Returns a conflict-identity
> string for tools whose effect is keyed to a target — typically the
> canonicalized path; for `WriteMemoryTopic` a `memory:{scope}:{topic}`
> string. The dispatcher groups batch calls by key; `None` group
> parallelizes freely, each non-`None` key group runs serially. Different
> keys parallelize against each other.
>
> **What this preserves.** Read/Grep/Glob/Bash continue to behave exactly
> as before (default `None`). Two `Edit`s on different files still
> parallelize. The parallel-tools differentiator from Claude Code is
> intact.

---

## Section 6 — Finding 4: 50 ms TUI tick

Time-boxed investigation, then either a real fix or a close-out ADR.

### 6.1 Investigation procedure (time-box: ~1 hour)
1. Build a debug-log-enabled local binary with the 50 ms tick commented
   out (don't delete yet — easy revert).
2. Run an interactive TUI session against a real provider. Drive 3-4
   streaming completions that historically showed the stall pattern
   (long single-message streams; tool-call follow-ups; thinking-heavy
   turns from a reasoning model).
3. Watch the debug log for the "no event for >100 ms during a streaming
   completion" pattern that ADR 0014 originally diagnosed.
4. If stalls return: hunt for the missing waker in the `async_stream::try_stream!`
   call sites in `caliban-agent-core::stream`. Likely culprit is a
   manually constructed future that doesn't store its waker.
5. If stalls don't return after the time-box: write the close-out ADR
   (Section 6.3).

### 6.2 Best-case outcome: real fix
Commit message: `fix(tui): <root-cause one-liner — e.g. propagate waker through stream parser>`

- Remove the 50 ms tick.
- Add a regression test if the fix is locatable enough to write one
  (probably a streaming-parser unit test asserting that a parked task
  wakes on the next chunk).
- Append `## Revised 2026-05-26` to ADR 0014 noting the tick was removed
  and the root cause fixed.

### 6.3 Fallback outcome: close-out ADR
Write a new ADR (likely 0041): `50 ms redraw tick — close-out`.

Body covers:
- Original 2024 trade-off (per ADR 0014).
- Two years of incident-free operation.
- Investigation summary: what was probed, what wasn't found.
- Decision: tick stays. Re-investigate if a contributor identifies a
  reproducible stall under specific conditions.

### 6.4 Either way
The TUI tick is no longer an open question after this PR.

---

## Section 7 — Finding 7: Three missing ADRs

Three short ADRs (~30-60 lines each), all `Status: accepted`.

### 7.1 `caliband` sibling-binary placement
- **Context:** caliban (TUI/CLI) + caliband (supervisor daemon) are the
  only two binaries. The convention from ADR 0005 puts "primary" binaries
  at the workspace root; caliband lives at
  `crates/caliban-supervisor/src/bin/caliband.rs`.
- **Decision:** caliband stays nested under `caliban-supervisor` as a
  secondary binary. Rationale: it's not directly user-facing (the CLI is
  `caliban agents`, which talks to caliband over a Unix socket); it's
  semantically part of the supervisor crate; nesting it keeps the
  workspace root focused on the primary product surface.
- **Consequences:** clean process boundary; shared crate compilation; no
  startup path conflict; `cargo install` requires `--bin caliband`
  explicitly. Acceptable cost for the conceptual clarity.

### 7.2 `arc-swap` as shared-state primitive
- **Context:** Several read-mostly shared-state surfaces (settings
  overlay, model router routes, plugin registry) use `arc_swap::ArcSwap`
  rather than `tokio::sync::RwLock`.
- **Decision:** Prefer `arc-swap` for read-mostly state where readers
  outnumber writers by >10x and writers can tolerate full Arc replacement.
  Use `tokio::sync::RwLock` for surfaces with frequent partial mutation.
- **Consequences:** lock-free reads, no priority inversion under load,
  slightly higher memory churn on writes (Arc allocation per swap), no
  fairness guarantees between writers (acceptable for our config-reload
  use case).

### 7.3 `rmcp` 1.7 version pin
- **Context:** The Model Context Protocol Rust SDK (`rmcp`) is pinned at
  `1.7.x` rather than tracking the latest minor.
- **Decision:** Pin at 1.7. Bump in a single dedicated PR after manual
  review of the changelog.
- **Consequences:** insulation from breaking changes in MCP transport or
  server APIs; manual maintenance cost; review burden on each bump.
  Acceptable given the size of our MCP surface (`caliban-mcp-client`).

---

## Risks + alternatives

### Risk: PR scope
8-12 hours of work, ~15-20 files touched, 3-4 new ADRs, real code in 2-3
crates. The reviewer is reading very different kinds of changes in one go.

**Mitigation:** atomic commits per section; each commit is independently
revertable. If a section blocks (most likely Finding 4 investigation drags
past the time-box), drop the fix commit and ship the close-out ADR
instead — the PR keeps moving.

### Risk: Finding 4 time-box bleed
If the investigation produces a tantalizing partial fix that isn't ready
to ship, the temptation to keep digging is real.

**Mitigation:** the time-box is firm. If a real fix is identified but
needs more than the time-box to land safely, file it as a follow-up PR
and ship the close-out ADR in this one.

### Risk: Finding 3 schema change
Adding `cap_tokens_*` to the settings schema is a public surface change.
Users running with custom `[memory]` configs need to not break.

**Mitigation:** all new keys are optional with defaults; existing configs
continue to work unchanged. Schema validation rejects only invalid types,
not missing keys.

### Alternative considered: split into doc PR + code PR
Audit + F1 + F2 + F6 + F7 as one doc-only PR; F3 + F5 + maybe F4 as a
second code PR. Cleaner review surface per PR but loses the through-line
(the audit motivates the code work). **Rejected** at brainstorming time
in favor of the single-PR sprint posture.

### Alternative considered: per-tool `is_parallel_safe()` bool (Finding 5)
Simpler trait method but serializes any batch containing a write tool —
including `Edit(a.rs) + Edit(b.rs)`. **Rejected** because the parallel-tools
win is the differentiator and same-file is the only real collision risk.

### Alternative considered: per-path with full conflict resolution
Per-target keys *plus* explicit read-after-write barriers (e.g.,
`Edit(a.rs) + Read(a.rs)` serializes). **Rejected** because it punishes
the common read-then-edit pattern; callers that need read-after-write
semantics shouldn't be batching them.

---

## Testing summary

| Section | Test surface |
|---|---|
| 1 (audit fixes) | manual review of two edits |
| 2 (README status) | manual diff review; grep for `Status: proposed` after the change should show only the 19 ADRs still genuinely proposed (40 − 21) |
| 3 (ADR amendments) | manual review |
| 4 (memory cap) | unit tests in `caliban-memory` per Section 4.5 |
| 5 (conflict key) | unit tests in `caliban-agent-core` per Section 5.5; integration test with the actual `Edit`/`Write`/`WriteMemoryTopic` tools |
| 6 (TUI tick — fix path) | streaming-parser unit test if reachable |
| 6 (TUI tick — close-out path) | no test; this is a doc commit |
| 7 (missing ADRs) | manual review |

Whole-workspace `cargo test --workspace` runs green before each commit.
`cargo clippy --workspace --all-targets` runs clean.

---

## Open questions (resolve at implementation time)

1. **Where is the 8 KiB constant defined?** Verify before writing the
   `MemoryBudget` struct vs bare constant decision.
2. **Next free ADR number?** Verify against `adrs/` directory before
   writing the new ones. Expect 0041 unless a concurrent PR has landed
   one.
3. **Finding 4 outcome.** Determined at investigation time. Both commit
   sequences (fix or close-out) are in this spec.
4. **Should the audit doc's TL;DR headline number (`21 of 40`) be
   re-verified?** Yes — 3-ADR spot check (Section 1.2). If wrong, fix
   before committing.

## Next step

After this spec is approved: invoke the writing-plans skill to produce a
detailed implementation plan with per-step tasks, test gates, and verify
commands.
