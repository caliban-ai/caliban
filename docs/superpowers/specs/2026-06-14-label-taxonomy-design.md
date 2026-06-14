# Shared label taxonomy across caliban / gonzalo / prospero

- **Issue:** caliban-ai/caliban#109
- **Date:** 2026-06-14
- **Status:** Approved design, pending implementation plan

## Problem

The three caliban-ai sibling repos share one org Kanban (Projects v2 #1) but have
drifted on label taxonomy, so board filters and cross-repo triage are inconsistent.

The original ticket described a larger gap than what exists today. Current **actual**
state (verified live, 2026-06-14):

| Group | caliban | gonzalo | prospero |
|---|---|---|---|
| `kind/*`, `priority/*`, `triage/*`, `lifecycle/*`, `good first issue`, `help wanted` | yes | yes | yes |
| `.github/labels.yml` + `label-sync.yml` governance | yes | yes | yes |
| Self-id marker | `caliban` | **missing** | **missing** |
| `area/*` | 12 (subsystem areas) | 3 (integration, security, performance) | 1 (test) |

So the **common core is already reconciled** and **all three already use the synced
`labels.yml` governance model**. The remaining drift is exactly two things:

1. gonzalo and prospero have no self-identification marker (caliban has `caliban`).
2. The `area/*` sets are nearly disjoint, so a board-level `area/*` filter only works
   for caliban.

## Goals

- Every repo carries a bare self-id marker matching the existing `caliban` convention.
- A shared cross-cutting `area/*` core that all three repos define, so board-level
  `area/*` filtering works consistently across repos.
- Each repo additionally exposes `area/*` labels for its real subsystems.
- Zero label removals; this is purely additive plus a cosmetic color standardization.

## Non-goals

- No single-source-of-truth label tooling. The per-repo `labels.yml` + `label-sync`
  model already works and the three core blocks already agree; a vendored/synced
  source-of-truth file is unnecessary complexity (YAGNI).
- No reconciliation of historical migration aliases (`type:*`, `area:*` on caliban vs
  `bug`/`enhancement`/`tech-debt` on gonzalo/prospero). Aliases only remap old labels
  during sync; the drift is harmless.
- No renaming of caliban's existing `caliban` marker or its existing `area/*` labels.

## Governance model

Each repo keeps its own `.github/labels.yml`, synced to GitHub by `label-sync.yml`
(`EndBug/label-sync@v2.3.3`, `delete-other-labels: true`, triggered on push to `main`
touching `.github/labels.yml`, plus `workflow_dispatch`). This model already exists in
all three repos.

The **common core** blocks (`kind/*`, `priority/*`, `triage/*`, `lifecycle/*`,
`good first issue`, `help wanted`) are kept byte-identical across the three files **by
convention**. Any change to the core must be applied to all three `labels.yml` files in
the same change set.

## Label classes

### 1. Self-id marker (bare name, one per repo)

| repo | label | color | action |
|---|---|---|---|
| caliban | `caliban` | `#5319e7` | keep as-is |
| gonzalo | `gonzalo` | `#006b75` | **add** |
| prospero | `prospero` | `#b60205` | **add** |

Description text: `"<Repo> sub-project."` to mirror the existing
`caliban` -> `"Caliban harness sub-project."` style (e.g. gonzalo ->
`"Gonzalo sub-project."`, prospero -> `"Prospero sub-project."`).

### 2. Shared cross-cutting `area/*` core (all three repos)

All carry these five, color `#1d76db`, so board-level `area/*` filtering works
cross-repo:

- `area/docs` — "Area: docs"
- `area/ci-cd` — "Area: ci-cd"
- `area/test` — "Area: testing"
- `area/security` — "Area: security"
- `area/performance` — "Area: performance"

### 3. Per-repo subsystem `area/*` (color `#1d76db`)

Derived from each repo's actual crate layout.

- **caliban** (existing, unchanged): `area/slash-commands`, `area/tui`,
  `area/observability`, `area/providers`, `area/tools`, `area/model-router`,
  `area/permissions`, `area/memory`, `area/sub-agents`, `area/integrations`
- **gonzalo** (add): `area/core`, `area/store`, `area/server`, `area/proto`,
  `area/domain`, `area/vector`, `area/graph`, `area/cli`; keep its existing
  `area/integration` ("cross-repo / caliban adoption")
- **prospero** (add): `area/api`, `area/cli`, `area/core`, `area/daemon`; keep its
  existing `area/test` (now part of the shared core)

Note: the ticket suggested "substrates" for gonzalo; the real crate family is
`gonzalo-store-*`, so the label is `area/store`.

Note: gonzalo's `area/integration` (singular, cross-repo adoption) and caliban's
`area/integrations` (plural, external integrations) are distinct subsystem concerns and
both are retained as-is; they are not merged.

## Concrete per-repo deltas (all additions, zero removals)

### caliban
- Add `area/test`, `area/security`, `area/performance` (shared core gaps).
- Everything else unchanged.

### gonzalo
- Add `gonzalo` self-id marker.
- Add shared-core gaps: `area/docs`, `area/ci-cd`, `area/test`.
- Add subsystem areas: `area/core`, `area/store`, `area/server`, `area/proto`,
  `area/domain`, `area/vector`, `area/graph`, `area/cli`.
- Keep `area/security`, `area/performance`, `area/integration`.
- Standardize `area/security` and `area/performance` color to `#1d76db`.

### prospero
- Add `prospero` self-id marker.
- Add shared-core gaps: `area/docs`, `area/ci-cd`, `area/security`, `area/performance`.
- Add subsystem areas: `area/api`, `area/cli`, `area/core`, `area/daemon`.
- Keep `area/test`.

### Color standardization
All `area/*` labels use `#1d76db` (caliban reference). gonzalo currently colors
`area/security` (`#d93f0b`) and `area/performance` (`#fbca04`) differently; these are
normalized to the area blue. Cosmetic only.

## Rollout

Three independent PRs, one per repo, each editing `.github/labels.yml`:

1. **caliban** — via the existing worktree `issue-109-label-taxonomy`.
2. **gonzalo** — branch in the gonzalo repo.
3. **prospero** — branch in the prospero repo.

On merge to each repo's `main`, `label-sync.yml` runs and applies the labels
(`delete-other-labels: true` means the YAML is authoritative). Because the change is
additive (plus a color tweak), no existing labels are deleted from any repo.

## Acceptance criteria

- [ ] Documented canonical taxonomy exists (this spec).
- [ ] Governance model recorded (per-repo `labels.yml` + `label-sync`, shared core by
      convention).
- [ ] `area/*` shared cross-cutting core defined and present in all three `labels.yml`.
- [ ] Per-repo subsystem `area/*` cover each repo's actual crates.
- [ ] gonzalo and prospero each gain a bare self-id marker.
- [ ] All three `labels.yml` files updated; PRs open per repo.
- [ ] After sync, board `area/*` / `kind/*` filters behave consistently across repos.

## Verification

After each repo's PR merges and `label-sync` completes:

```bash
for r in caliban gonzalo prospero; do
  echo "== $r =="; gh label list --repo caliban-ai/$r --limit 200 | sort
done
```

Confirm: each repo lists its bare self-id marker; all three list the five shared-core
`area/*` labels; each repo lists its subsystem areas.
