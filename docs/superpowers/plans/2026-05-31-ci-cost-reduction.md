# CI Cost Reduction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut GitHub Actions spend ~75-85% per PR by dropping the duplicate `push` trigger on the default CI workflow, moving the heavy cloud-features workflow to manual + weekly cron, and skipping CI on docs-only changes.

**Architecture:** Three YAML/Markdown edits in a single PR, no source code touched. The cloud feature gates and provider crate boundaries are unchanged; the only thing changing is *when* CI builds them. Branch protection has already been updated to drop the cloud check from required-status.

**Tech Stack:** GitHub Actions YAML, repository docs. Validation via `python3 -c "import yaml; ..."` (actionlint is not installed locally; GitHub will reject malformed YAML on push as the second line of defense).

**Source spec:** `docs/superpowers/specs/2026-05-31-ci-cost-reduction-design.md`

---

## File Structure

Three files modified, no creates, no source code:

- Modify: `.github/workflows/ci.yml` — replace `on:` block, lower timeout to 15 min
- Modify: `.github/workflows/ci-cloud.yml` — replace `on:` block with `workflow_dispatch` + weekly cron, drop the now-unused `paths:` lists
- Modify: `README.md` — add a short "CI" section (or extend an existing one) noting that cloud features aren't built in PR CI

---

### Task 1: Rewrite `ci.yml` trigger block + tighten timeout + add paths-ignore

**Files:**
- Modify: `.github/workflows/ci.yml` (lines 1–17)

- [ ] **Step 1: Read the current file to confirm baseline**

Run: `cat .github/workflows/ci.yml`

Expected: the file matches what's in the spec — `on:` block has both `push: ["**"]` and `pull_request: ["**"]`, `timeout-minutes: 20`, no `paths-ignore`.

- [ ] **Step 2: Replace the `on:` block and `timeout-minutes`**

Use the Edit tool to replace this exact block at the top of the file:

```yaml
name: ci

on:
  push:
    branches: ["**"]
  pull_request:
    branches: ["**"]

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

jobs:
  check:
    name: fmt · clippy · build · test (default features)
    runs-on: ubuntu-latest
    timeout-minutes: 20
```

with:

```yaml
name: ci

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
  workflow_dispatch:

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

jobs:
  check:
    name: fmt · clippy · build · test (default features)
    runs-on: ubuntu-latest
    timeout-minutes: 15
```

- [ ] **Step 3: Validate YAML syntax**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" && echo OK`

Expected: prints `OK`. No exception traceback.

- [ ] **Step 4: Confirm visual diff matches intent**

Run: `git diff .github/workflows/ci.yml`

Expected: hunks show (a) `push: branches: ["**"]` → `push: branches: [main]`, (b) `paths-ignore` added under both `pull_request` and `push`, (c) `workflow_dispatch:` added at end of `on:` block, (d) `timeout-minutes: 20` → `15`. No other changes.

---

### Task 2: Rewrite `ci-cloud.yml` trigger block; remove obsolete `paths:` lists

**Files:**
- Modify: `.github/workflows/ci-cloud.yml` (lines 1–51)

- [ ] **Step 1: Read the current file to confirm baseline**

Run: `cat .github/workflows/ci-cloud.yml`

Expected: the file matches the spec — `on:` block has `push`/`pull_request` triggers with long `paths:` lists, plus `workflow_dispatch`.

- [ ] **Step 2: Replace the `on:` block**

Use the Edit tool to replace this exact block:

```yaml
name: ci-cloud

on:
  push:
    branches: ["**"]
    # NOTE: the `paths:` list is intentionally duplicated under `push`
    # and `pull_request` below. GitHub Actions' historical YAML-anchor
    # support has been spotty enough that duplication is safer than
    # the DRY win. Keep the two lists in sync.
    paths:
      # Provider internals that directly compile under cloud features.
      - 'crates/caliban-provider-anthropic/src/transport/bedrock.rs'
      - 'crates/caliban-provider-anthropic/src/transport/vertex.rs'
      - 'crates/caliban-provider-google/src/transport/vertex.rs'
      - 'crates/caliban-provider-openai/src/transport/azure.rs'
      - 'crates/caliban-provider-anthropic/src/config.rs'
      - 'crates/caliban-provider-google/src/config.rs'
      - 'crates/caliban-provider-openai/src/config.rs'
      # Binary crate — provider construction wiring + anything in the
      # caliban binary's source can affect what the cloud-features
      # build sees. Branch protection requires this check, so PRs
      # that touched the binary but none of the provider-internals
      # above used to BLOCK forever (the workflow never triggered).
      # Covering `caliban/src/**` + `caliban/Cargo.toml` (+ workspace
      # `Cargo.toml`) unblocks them.
      - 'caliban/src/**'
      - 'caliban/Cargo.toml'
      - 'Cargo.toml'
      - 'Cargo.lock'
      - '.github/workflows/ci-cloud.yml'
  pull_request:
    branches: ["**"]
    paths:
      - 'crates/caliban-provider-anthropic/src/transport/bedrock.rs'
      - 'crates/caliban-provider-anthropic/src/transport/vertex.rs'
      - 'crates/caliban-provider-google/src/transport/vertex.rs'
      - 'crates/caliban-provider-openai/src/transport/azure.rs'
      - 'crates/caliban-provider-anthropic/src/config.rs'
      - 'crates/caliban-provider-google/src/config.rs'
      - 'crates/caliban-provider-openai/src/config.rs'
      - 'caliban/src/**'
      - 'caliban/Cargo.toml'
      - 'Cargo.toml'
      - 'Cargo.lock'
      - '.github/workflows/ci-cloud.yml'
  # Manual re-trigger — useful for ad-hoc reruns against the current
  # branch tip even when the path filter didn't match. Note:
  # workflow_dispatch runs satisfy the *workflow name* but NOT a
  # required-status check on a PR (which binds to pull_request event
  # type); use a path-filter touch to retrigger the required check.
  workflow_dispatch:
```

with:

```yaml
name: ci-cloud

# Cloud features (bedrock / vertex / azure) are not built in PR CI.
# Drift on `main` is caught by the weekly cron; explicit verification
# before merging cloud-transport changes is via `workflow_dispatch`
# from the Actions tab. Branch protection no longer requires this
# check. Rationale: docs/superpowers/specs/2026-05-31-ci-cost-reduction-design.md
on:
  workflow_dispatch:
  schedule:
    - cron: "0 13 * * 1"   # Mondays 13:00 UTC
```

The rest of the file (`concurrency` block + `jobs.check-cloud` body) stays exactly as it is.

- [ ] **Step 3: Validate YAML syntax**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci-cloud.yml'))" && echo OK`

Expected: prints `OK`.

- [ ] **Step 4: Confirm visual diff matches intent**

Run: `git diff .github/workflows/ci-cloud.yml`

Expected: the entire long `on:` block (including both `paths:` lists and the lead-in `NOTE:` comment) is removed and replaced with the new 5-line block (workflow_dispatch + cron, plus the rationale comment). Nothing else in the file changes — `concurrency:`, `jobs:`, `runs-on:`, `timeout-minutes: 30`, the `free-disk-space` step, the `cargo build/clippy/test` step all unchanged.

---

### Task 3: Add a "CI" note to `README.md`

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Read the current README to pick an insertion point**

Run: `cat README.md | head -100`

Look for an existing "Development", "Contributing", or "CI" section. If one exists, append the note there. Otherwise add a new top-level `## CI` section near the end of the file (before any "License" section if present).

- [ ] **Step 2: Insert the CI note**

Append (or add to the chosen section) this exact text:

```markdown
## CI

Pull-request CI runs `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, and `cargo test --workspace` against the default feature set. Docs-only changes (`**.md`, `docs/**`, `LICENSE`, `.github/ISSUE_TEMPLATE/**`) skip CI entirely.

Cloud transports (`caliban-provider-anthropic/{bedrock,vertex}`, `caliban-provider-openai/azure`, `caliban-provider-google/vertex`) are **not** built in PR CI. They are exercised by:

- A weekly cron (Mondays 13:00 UTC) that runs the full cloud-features build against `main`.
- Manual dispatch of the `ci-cloud` workflow from the Actions tab when a PR touches cloud transport code.

To verify cloud changes locally:

```bash
cargo build --workspace \
  --features caliban-provider-anthropic/bedrock,caliban-provider-anthropic/vertex,caliban-provider-openai/azure,caliban-provider-google/vertex
```
```

(If a `## License` section exists at the end of the README, insert the new `## CI` section immediately before it.)

- [ ] **Step 3: Verify the README still renders as expected**

Run: `git diff README.md | head -60`

Expected: an added `## CI` section with the text above, no removed lines elsewhere, no broken markdown (matching open/close fences for the code block).

---

### Task 4: Commit the three edits as one logical change

**Files:**
- Modify: previously staged in Tasks 1–3

- [ ] **Step 1: Stage exactly the three modified files**

Run:
```bash
git add .github/workflows/ci.yml .github/workflows/ci-cloud.yml README.md
git status -s
```

Expected: `git status -s` shows three `M ` lines for the three files. No other files in the staging area.

- [ ] **Step 2: Commit with a self-contained message**

Run:
```bash
git commit -m "$(cat <<'EOF'
ci: drop dual triggers, move ci-cloud to manual+weekly cron

Pull-request CI was firing both workflows twice per push (push event
+ pull_request event, separate concurrency groups), and the cloud
workflow path-matched almost every PR via `caliban/src/**`. With four
runs per push, a typical PR cost ~20-25 compute-minutes.

This change:

- ci.yml: triggers on `pull_request` + `push: main` + `workflow_dispatch`
  only. Adds `paths-ignore` for `**.md`, `docs/**`, `LICENSE`,
  `.github/ISSUE_TEMPLATE/**`. Lowers `timeout-minutes` from 20 to 15
  as a runaway-cost circuit breaker.
- ci-cloud.yml: triggers reduced to `workflow_dispatch` + a weekly
  Monday-13:00-UTC cron against `main`. The body (free-disk-space
  step, cargo build/clippy/test with bedrock+vertex+azure features)
  is unchanged. The `paths:` lists are removed — they no longer apply.
- README.md: adds a `## CI` section documenting the new shape and
  how to verify cloud features locally.

Branch protection has already been updated to drop the cloud check
from required-status. Spec:
docs/superpowers/specs/2026-05-31-ci-cost-reduction-design.md

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3: Verify the commit landed cleanly**

Run: `git log -1 --stat`

Expected: one commit, three files changed, roughly +30/-80 lines (the `ci-cloud.yml` `paths:` removal dominates the delete count).

---

### Task 5: Push, open PR, watch CI

**Files:** none (git/remote operations only)

- [ ] **Step 1: Push the branch**

Run: `git push -u origin "$(git branch --show-current)"`

Expected: a new remote branch is created; the output mentions a "Create a pull request" URL.

- [ ] **Step 2: Open the PR via `gh`**

Run:
```bash
gh pr create --title "ci: drop dual triggers, move ci-cloud to manual+weekly cron" --body "$(cat <<'EOF'
## Summary

Cuts GitHub Actions spend ~75–85% per PR by dropping the duplicate `push` trigger on `ci.yml`, moving `ci-cloud.yml` to manual + weekly cron, and skipping CI on docs-only changes.

Three edits, no source code touched. Branch protection has already been updated to drop the cloud check from required-status.

Spec: `docs/superpowers/specs/2026-05-31-ci-cost-reduction-design.md`
Plan: `docs/superpowers/plans/2026-05-31-ci-cost-reduction.md`

## Test plan

- [ ] This PR itself: confirm only one `ci.yml` run fires per push (no duplicate push+pull_request runs), and zero `ci-cloud.yml` runs.
- [ ] After merge: open a no-op PR (touch a code comment) and verify the same one-run-per-push pattern.
- [ ] After merge: merge that no-op PR; verify one `ci.yml` run on `push: main`.
- [ ] After merge: open a docs-only PR (`.md` edit); verify zero workflow runs.
- [ ] After merge: manually fire `ci-cloud` from the Actions tab against `main`; verify the workflow still runs end-to-end.
- [ ] After merge: wait for the first Monday cron at 13:00 UTC; verify it fires.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Expected: a PR URL is printed.

- [ ] **Step 3: Verify CI behavior on THIS PR — the first verification step**

Run: `gh pr checks "$(gh pr view --json number --jq .number)" --watch --interval 30`

Expected:
- Exactly **one** `fmt · clippy · build · test (default features)` check appears (previously: two — push + pull_request). This itself is partial proof the trigger change worked.
- **Zero** `build + test (bedrock + vertex + azure features)` checks appear. (The PR touches only `.github/workflows/**` and `README.md`; the new `ci-cloud.yml` has no `pull_request` trigger, so it cannot fire.)
- The `ci.yml` check passes.

If a second `ci.yml` check appears or any `ci-cloud.yml` check appears, the trigger blocks are wrong — stop, investigate via `gh pr checks --json` and the workflow file, fix, re-push.

- [ ] **Step 4: Merge**

Once the single check is green:
```bash
gh pr merge "$(gh pr view --json number --jq .number)" --squash --delete-branch
```

(Expected: the local cleanup step may error with `fatal: 'main' is already used by worktree at ...`. The server-side merge will still have succeeded. Confirm with `gh pr view --json state --jq .state` — should print `MERGED`.)

- [ ] **Step 5: Sync local main and clean up the worktree**

Run from inside the main checkout — NOT the worktree:

```bash
git -C /Users/johnford2002/dev/personal/caliban pull --ff-only origin main
```

Then `ExitWorktree` with `action: "remove"` and `discard_changes: true` (the squash-merge leaves the source branch's commit "unmerged" from git's POV, same as PR #84 and #85).

---

### Task 6: Post-merge verification — confirm the trigger and skip behavior

These checks happen ON the merged repo, not within this branch's PR.

**Files:** none (verification only)

- [ ] **Step 1: Open a no-op verification PR**

From the main checkout:

```bash
git checkout -b verify/ci-trigger-shape
# Touch any code file with a comment-only change. Example:
echo "" >> README.md   # add a blank line
git add README.md
git commit -m "chore: no-op to verify post-merge CI trigger shape"
git push -u origin verify/ci-trigger-shape
gh pr create --title "chore: verify CI trigger shape" --body "No-op PR to verify post-merge that exactly one ci.yml run fires per push and zero ci-cloud runs fire. Will be closed without merging."
```

Expected: PR opens.

- [ ] **Step 2: Confirm trigger behavior**

Run: `gh pr checks $(gh pr view --json number --jq .number)`

Expected:
- One `fmt · clippy · build · test (default features)` check.
- Zero `build + test (bedrock + vertex + azure features)` checks.

Push a follow-up no-op commit (`git commit --allow-empty -m "noop" && git push`) and re-check:

Expected: one additional `ci.yml` check appears (from the `pull_request: synchronize` event); still zero `ci-cloud` checks.

- [ ] **Step 3: Verify docs-only paths-ignore**

```bash
git checkout -b verify/docs-only-skip
echo "" >> docs/superpowers/specs/2026-05-31-ci-cost-reduction-design.md
git add docs/
git commit -m "chore: docs-only edit to verify paths-ignore"
git push -u origin verify/docs-only-skip
gh pr create --title "chore: verify docs-only skip" --body "No-op docs-only PR. Should fire zero workflow runs."
```

Then: `gh pr checks $(gh pr view --json number --jq .number)`

Expected: **No checks listed at all.** A docs-only PR should trigger zero workflows.

- [ ] **Step 4: Manually trigger `ci-cloud` against main**

Run: `gh workflow run ci-cloud.yml --ref main`

Then wait for it and confirm:

```bash
gh run list --workflow=ci-cloud.yml --limit 1
gh run watch "$(gh run list --workflow=ci-cloud.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
```

Expected: the workflow runs end-to-end against `main` HEAD and exits 0.

- [ ] **Step 5: Close the verification PRs without merging**

```bash
gh pr close verify/ci-trigger-shape --delete-branch
gh pr close verify/docs-only-skip --delete-branch
```

- [ ] **Step 6: (Deferred) Weekly cron**

The first scheduled cron run will fire on the next Monday at 13:00 UTC. Check `gh run list --workflow=ci-cloud.yml` afterwards to confirm it fired and passed. No action required this session.

---

## Self-Review notes

- **Spec coverage:** Decision 1 (trigger change) → Task 1. Decision 2 (ci-cloud manual+cron) → Task 2. Decision 3 (timeout + paths-ignore) → Task 1. README note → Task 3. Verification plan steps → Task 6 (covers steps 1–4 of the spec's verification plan; the Monday cron step is deferred per Task 6 Step 6).
- **Placeholder scan:** No "TBD"/"TODO". All YAML is shown verbatim. All commands are concrete.
- **Type consistency:** YAML key names verified against the existing files in Tasks 1 and 2.
- **Ambiguity:** The cron expression `"0 13 * * 1"` is unambiguous (Monday 13:00 UTC). The `paths-ignore` lists are identical in spec, plan, and `ci.yml` edit.
