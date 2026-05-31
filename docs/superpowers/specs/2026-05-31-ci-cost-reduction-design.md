---
title: CI cost reduction — drop dual triggers, move ci-cloud to manual + weekly cron
date: 2026-05-31
status: Proposed
author: john.ford2002@gmail.com
---

# CI cost reduction — Design

**Date:** 2026-05-31
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness

## Goal

Cut GitHub Actions spend by roughly 75–90% on a typical PR without
losing the safety the current setup provides. Triggered by hitting the
billing cap; aggressive tradeoffs are acceptable.

## Non-goals

- **No Cargo workspace restructure.** The bedrock / vertex / azure
  features stay on the existing provider crates exactly as they are.
  Cloud-feature compilation is already gated by Cargo features; the
  change here is purely *when CI builds them*, not *how they're
  organized in the source tree*.
- **No code deletion.** Cloud transport code stays. Anyone (the author)
  can still build and test it via `cargo build --features ...` or by
  firing the manual workflow.
- **No new CI provider, no self-hosted runners.** Stay on GitHub-hosted
  ubuntu-latest.
- **No matrix-build experiments.** A feature matrix would multiply runs
  before it shares cache; it doesn't serve the goal.

## Context

Two workflows currently run for the project:

1. **`ci.yml` — "fmt · clippy · build · test (default features)"**
   - Triggers: `push: ["**"]` AND `pull_request: ["**"]`
   - Job: cargo fmt + clippy + build + test on default features
   - Runtime: ~5 min, `timeout-minutes: 20`
2. **`ci-cloud.yml` — "build + test (bedrock + vertex + azure features)"**
   - Triggers: `push: ["**"]` AND `pull_request: ["**"]`, path-filtered
     to provider transport files + provider config files + the entire
     `caliban/src/**` tree + workspace manifests + the workflow file
   - Job: free-disk-space prep step, then cargo build + clippy + test
     with the cloud feature set enabled
   - Runtime: ~5–7 min, `timeout-minutes: 30`

Both workflows declare `concurrency: { group: ci-${{ github.ref }},
cancel-in-progress: true }`, but `github.ref` differs between `push`
(`refs/heads/<branch>`) and `pull_request` (`refs/pull/<N>/merge`), so
the two events live in *different* concurrency groups and both run to
completion for the same PR push.

Empirically, a typical PR (last six PRs, all touching `caliban/src/**`)
fires four runs per push: two for `ci.yml` (push event + pull_request
event) and two for `ci-cloud.yml` (same). Per-push compute is
~20–25 minutes.

The `ci-cloud` workflow's `paths:` list catches almost every PR because
`caliban/src/**` is a load-bearing entry — added per
PR #80's comment trail to unblock PRs that touched binary wiring
without touching a transport file. The intent was correct (avoid PRs
hanging on a required check that never fires), but the side effect is
that the heavy build runs nearly every time, even when the cloud
transports themselves are untouched.

## Decisions

### Decision 1: `ci.yml` triggers on PR + main only

Drop the `push: ["**"]` trigger. Replace with:

```yaml
on:
  pull_request:
    branches: ["**"]
  push:
    branches: [main]
  workflow_dispatch:
```

**Rationale.** The `pull_request` event already covers every push to a
PR branch (via the `synchronize` activity type). The `push: ["**"]`
trigger duplicates that coverage for any branch with an open PR; for
branches without a PR, the coverage isn't load-bearing — operators
push frequently as a backup and rarely need CI before opening a PR.
Keeping `push: main` preserves post-merge sanity. Adding
`workflow_dispatch` is free and convenient for manual reruns.

**Cost saved.** Eliminates the duplicate run per PR push (~50% of
total CI runs).

**What we lose.** Branches pushed without an open PR get no CI. The
workaround is `gh pr create --draft`; this is a deliberate friction.

### Decision 2: `ci-cloud.yml` becomes manual + weekly cron

Replace the existing trigger block with:

```yaml
on:
  workflow_dispatch:
  schedule:
    - cron: "0 13 * * 1"   # Mondays 13:00 UTC
```

The workflow body (free-disk-space step, cargo build/clippy/test with
the cloud feature set) is unchanged.

**Rationale.** The bedrock / vertex / azure transports are not on the
active development path for the author; recent PRs (#82–#85) have
touched zero cloud transport files. Defending against drift in those
files on every unrelated PR is the misalignment we're paying for.
The weekly cron is the safety net — drift on `main` will surface
within seven days, well inside the project's release cadence. Manual
dispatch covers the case where the author actively touches a cloud
transport file and wants confirmation before merging.

**Cost saved.** Drops `ci-cloud` from ~every PR to once a week plus
on-demand. Roughly 30% of total CI spend on top of Decision 1.

**What we lose.** PR-time confirmation that a `#[cfg(feature = "...")]`
gate compiles. Mitigated by: the gates rarely change, the weekly cron
catches drift, and the author can manually fire the workflow with one
click before merging anything that touches transport code. Branch
protection has already been updated to drop the cloud check from
required-status checks.

### Decision 3: Bundle small adjacent tweaks to `ci.yml`

While editing the file:

- Tighten `timeout-minutes: 20` → `15`. The current build comfortably
  finishes in ~5 min; a 15-min ceiling is still 3× the typical budget
  and serves as a runaway-cost circuit breaker.
- Add `paths-ignore` to skip the workflow on docs-only changes:
  ```yaml
  on:
    pull_request:
      branches: ["**"]
      paths-ignore:
        - "**.md"
        - "docs/**"
        - "LICENSE"
        - ".github/ISSUE_TEMPLATE/**"
    push:
      branches: [main]
      paths-ignore:
        - "**.md"
        - "docs/**"
        - "LICENSE"
        - ".github/ISSUE_TEMPLATE/**"
  ```
  Small flat win; primarily an ergonomics improvement (no waiting on
  CI for typo fixes) but also a real money saver over time.

**Rationale.** These are zero-risk, in-the-neighborhood improvements
that compound the main cuts. Bundling avoids a second PR for trivial
related work.

## Concrete changes

The PR consists of three edits, no code changes:

1. `.github/workflows/ci.yml` — replace `on:` block per Decision 1 +
   Decision 3, change `timeout-minutes` per Decision 3.
2. `.github/workflows/ci-cloud.yml` — replace `on:` block per
   Decision 2; remove the now-unused `paths:` lists. Workflow body
   unchanged.
3. Add a short note (target: `README.md`, in the "Development" or
   "Contributing" section if present; otherwise a new top-level
   "## CI" section) documenting that cloud transports aren't built
   in PR CI and how to verify them locally / via manual dispatch.

## Verification plan

Post-merge:

1. Open a trivial no-op PR (touch a code comment). Expect:
   - One `ci.yml` run when the PR opens.
   - One additional `ci.yml` run per `git push` to the branch.
   - Zero `ci-cloud.yml` runs.
2. Merge the no-op PR. Expect:
   - One `ci.yml` run on `push: main`.
   - Zero `ci-cloud.yml` runs.
3. Manually trigger `ci-cloud` from the Actions tab against `main`.
   Expect: the workflow runs end-to-end and passes.
4. Open a docs-only PR (edit a `.md` file). Expect: zero workflow runs.
5. Wait for the first scheduled cron run on the following Monday at
   13:00 UTC. Confirm it fires and passes.

## Rollback

The entire change is a YAML revert. If post-rollout verification
surfaces an unexpected problem (e.g., the docs-ignore matches too
aggressively, or the cron firing time conflicts with other infra),
revert the relevant commit; no code, schema, or dependency state
needs to change.

## Expected impact

Per typical PR (assuming 3 pushes per PR-lifecycle, all touching
`caliban/src/**`):

| Setup | Runs per PR | Approx compute-min per PR |
|---|---|---|
| Current | 12 (3 pushes × 2 events × 2 workflows) | ~70–90 |
| After Decision 1 only | 6 | ~30–40 |
| After Decisions 1 + 2 | 3 | ~15–20 |
| After Decisions 1 + 2 + cron | 3 + ~1/week flat | ~15–20 + ~5–7/week |

**~75–85% reduction on a typical PR**, with the weekly cron adding
back a flat ~30 compute-minutes per month for cloud drift detection.
Docs-only PRs go from ~20 min to zero.
