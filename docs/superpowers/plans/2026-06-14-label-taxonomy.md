# Shared Label Taxonomy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reconcile labels across caliban/gonzalo/prospero — add per-repo self-id markers and a shared cross-cutting `area/*` core plus per-repo subsystem areas — via three independent PRs.

**Architecture:** Each repo owns a `.github/labels.yml` synced to GitHub by `label-sync.yml` (`delete-other-labels: true`). Changes are additive YAML edits (plus one cosmetic color normalization on gonzalo). On merge to `main`, `label-sync` makes the YAML authoritative. No code, no Rust toolchain involved.

**Tech Stack:** YAML, GitHub Labels, `EndBug/label-sync` GitHub Action, `gh` CLI, `python3 -c` for YAML validation.

**Spec:** `docs/superpowers/specs/2026-06-14-label-taxonomy-design.md`

**Worktrees / branches:**
- caliban: existing worktree `.claude/worktrees/issue-109-label-taxonomy`, branch `worktree-issue-109-label-taxonomy`.
- gonzalo: new branch in `/Users/johnford2002/dev/caliban-ai/gonzalo`.
- prospero: new branch in `/Users/johnford2002/dev/caliban-ai/prospero`.

---

### Task 1: caliban — add shared-core area labels

**Files:**
- Modify: `/Users/johnford2002/dev/caliban-ai/caliban/.claude/worktrees/issue-109-label-taxonomy/.github/labels.yml` (append to the `# area/* (caliban)` section)

- [ ] **Step 1: Append the three shared-core area labels**

In `.github/labels.yml`, after the last existing area entry (`area/docs`), append:

```yaml
- name: area/test
  color: '1d76db'
  description: "Area: testing"
- name: area/security
  color: '1d76db'
  description: "Area: security"
- name: area/performance
  color: '1d76db'
  description: "Area: performance"
```

- [ ] **Step 2: Validate YAML parses and labels are unique**

Run:
```bash
cd /Users/johnford2002/dev/caliban-ai/caliban/.claude/worktrees/issue-109-label-taxonomy
python3 -c "import yaml,sys; d=yaml.safe_load(open('.github/labels.yml')); n=[x['name'] for x in d]; assert len(n)==len(set(n)), 'dup labels'; assert {'area/test','area/security','area/performance'} <= set(n); print(f'{len(n)} labels, unique, additions present')"
```
Expected: `39 labels, unique, additions present`

- [ ] **Step 3: Commit**

```bash
cd /Users/johnford2002/dev/caliban-ai/caliban/.claude/worktrees/issue-109-label-taxonomy
git add .github/labels.yml
git commit -m "feat(labels): add shared cross-cutting area core (#109)

Add area/test, area/security, area/performance to caliban so the
cross-repo area/* board filter set is consistent across siblings.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 4: Push and open PR**

```bash
cd /Users/johnford2002/dev/caliban-ai/caliban/.claude/worktrees/issue-109-label-taxonomy
git push -u origin worktree-issue-109-label-taxonomy
gh pr create --repo caliban-ai/caliban --base main \
  --title "feat(labels): shared cross-cutting area core (#109)" \
  --body "Part of #109. Adds area/test, area/security, area/performance so all three sibling repos share a cross-cutting area/* core. Spec: docs/superpowers/specs/2026-06-14-label-taxonomy-design.md

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```
Expected: PR URL printed.

---

### Task 2: gonzalo — self-id marker, shared core, subsystem areas, color normalization

**Files:**
- Modify: `/Users/johnford2002/dev/caliban-ai/gonzalo/.github/labels.yml`

- [ ] **Step 1: Create a branch**

```bash
cd /Users/johnford2002/dev/caliban-ai/gonzalo
git switch main && git pull --ff-only
git switch -c labels-taxonomy-109
```
Expected: on branch `labels-taxonomy-109`.

- [ ] **Step 2: Insert the self-id marker block**

In `.github/labels.yml`, after the `# contributor` block (the `help wanted` entry) and before the `# area/* (gonzalo)` comment, insert:

```yaml
# component marker
- name: gonzalo
  color: '006b75'
  description: "Gonzalo sub-project."
```

- [ ] **Step 3: Replace the entire area section**

Replace the existing block that starts at `# area/* (gonzalo)` and ends at the last `area/performance` entry with:

```yaml
# area/* shared cross-cutting core
- name: area/docs
  color: '1d76db'
  description: "Area: docs"
- name: area/ci-cd
  color: '1d76db'
  description: "Area: ci-cd"
- name: area/test
  color: '1d76db'
  description: "Area: testing"
- name: area/security
  color: '1d76db'
  description: "Area: security"
  aliases: ['security']
- name: area/performance
  color: '1d76db'
  description: "Area: performance"
  aliases: ['performance']
# area/* (gonzalo subsystems)
- name: area/integration
  color: '1d76db'
  description: "Area: cross-repo / caliban adoption"
  aliases: ['integration']
- name: area/core
  color: '1d76db'
  description: "Area: core"
- name: area/store
  color: '1d76db'
  description: "Area: store"
- name: area/server
  color: '1d76db'
  description: "Area: server"
- name: area/proto
  color: '1d76db'
  description: "Area: proto"
- name: area/domain
  color: '1d76db'
  description: "Area: domain"
- name: area/vector
  color: '1d76db'
  description: "Area: vector"
- name: area/graph
  color: '1d76db'
  description: "Area: graph"
- name: area/cli
  color: '1d76db'
  description: "Area: cli"
```

- [ ] **Step 4: Validate YAML parses, labels unique, additions present**

Run:
```bash
cd /Users/johnford2002/dev/caliban-ai/gonzalo
python3 -c "import yaml; d=yaml.safe_load(open('.github/labels.yml')); n=[x['name'] for x in d]; assert len(n)==len(set(n)),'dup'; need={'gonzalo','area/docs','area/ci-cd','area/test','area/security','area/performance','area/integration','area/core','area/store','area/server','area/proto','area/domain','area/vector','area/graph','area/cli'}; assert need<=set(n), need-set(n); import re; assert all(x['color']=='1d76db' for x in d if x['name'].startswith('area/')), 'area color not normalized'; print(f'{len(n)} labels, unique, additions present, area colors normalized')"
```
Expected: `... labels, unique, additions present, area colors normalized`

- [ ] **Step 5: Commit**

```bash
cd /Users/johnford2002/dev/caliban-ai/gonzalo
git add .github/labels.yml
git commit -m "feat(labels): self-id marker + shared area core + subsystems (#109)

Add 'gonzalo' self-id marker; add shared cross-cutting area core
(docs, ci-cd, test); add subsystem areas (core, store, server, proto,
domain, vector, graph, cli); normalize all area/* to #1d76db.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 6: Push and open PR**

```bash
cd /Users/johnford2002/dev/caliban-ai/gonzalo
git push -u origin labels-taxonomy-109
gh pr create --repo caliban-ai/gonzalo --base main \
  --title "feat(labels): self-id marker + shared area taxonomy (#109)" \
  --body "Part of caliban-ai/caliban#109. Adds the 'gonzalo' self-id marker, the shared cross-cutting area/* core (docs, ci-cd, test), gonzalo subsystem areas (core, store, server, proto, domain, vector, graph, cli), and normalizes area/* colors to #1d76db.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```
Expected: PR URL printed.

---

### Task 3: prospero — self-id marker, shared core, subsystem areas

**Files:**
- Modify: `/Users/johnford2002/dev/caliban-ai/prospero/.github/labels.yml`

- [ ] **Step 1: Create a branch**

```bash
cd /Users/johnford2002/dev/caliban-ai/prospero
git switch main && git pull --ff-only
git switch -c labels-taxonomy-109
```
Expected: on branch `labels-taxonomy-109`.

- [ ] **Step 2: Insert the self-id marker block**

In `.github/labels.yml`, after the `# contributor` block (the `help wanted` entry) and before the `# area/* (prospero)` comment, insert:

```yaml
# component marker
- name: prospero
  color: 'b60205'
  description: "Prospero sub-project."
```

- [ ] **Step 3: Replace the entire area section**

Replace the existing block that starts at `# area/* (prospero)` and ends at the `area/test` entry with:

```yaml
# area/* shared cross-cutting core
- name: area/docs
  color: '1d76db'
  description: "Area: docs"
- name: area/ci-cd
  color: '1d76db'
  description: "Area: ci-cd"
- name: area/test
  color: '1d76db'
  description: "Area: testing"
  aliases: ['test']
- name: area/security
  color: '1d76db'
  description: "Area: security"
- name: area/performance
  color: '1d76db'
  description: "Area: performance"
# area/* (prospero subsystems)
- name: area/api
  color: '1d76db'
  description: "Area: api"
- name: area/cli
  color: '1d76db'
  description: "Area: cli"
- name: area/core
  color: '1d76db'
  description: "Area: core"
- name: area/daemon
  color: '1d76db'
  description: "Area: daemon"
```

- [ ] **Step 4: Validate YAML parses, labels unique, additions present**

Run:
```bash
cd /Users/johnford2002/dev/caliban-ai/prospero
python3 -c "import yaml; d=yaml.safe_load(open('.github/labels.yml')); n=[x['name'] for x in d]; assert len(n)==len(set(n)),'dup'; need={'prospero','area/docs','area/ci-cd','area/test','area/security','area/performance','area/api','area/cli','area/core','area/daemon'}; assert need<=set(n), need-set(n); print(f'{len(n)} labels, unique, additions present')"
```
Expected: `... labels, unique, additions present`

- [ ] **Step 5: Commit**

```bash
cd /Users/johnford2002/dev/caliban-ai/prospero
git add .github/labels.yml
git commit -m "feat(labels): self-id marker + shared area core + subsystems (#109)

Add 'prospero' self-id marker; add shared cross-cutting area core
(docs, ci-cd, security, performance); add subsystem areas (api, cli,
core, daemon).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 6: Push and open PR**

```bash
cd /Users/johnford2002/dev/caliban-ai/prospero
git push -u origin labels-taxonomy-109
gh pr create --repo caliban-ai/prospero --base main \
  --title "feat(labels): self-id marker + shared area taxonomy (#109)" \
  --body "Part of caliban-ai/caliban#109. Adds the 'prospero' self-id marker, the shared cross-cutting area/* core (docs, ci-cd, security, performance), and prospero subsystem areas (api, cli, core, daemon).

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```
Expected: PR URL printed.

---

### Task 4: Post-merge verification

**Do this only after all three PRs are merged and each repo's `label-sync` workflow has run.**

- [ ] **Step 1: Confirm label-sync workflows succeeded**

```bash
for r in caliban gonzalo prospero; do
  echo "== $r =="; gh run list --repo caliban-ai/$r --workflow label-sync.yml --limit 1
done
```
Expected: most recent run `completed / success` for each repo.

- [ ] **Step 2: Verify live labels across all repos**

```bash
for r in caliban gonzalo prospero; do
  echo "== $r =="; gh label list --repo caliban-ai/$r --limit 200 | sort
done
```
Expected: each repo lists its bare self-id marker (`caliban`/`gonzalo`/`prospero`); all three list `area/{docs,ci-cd,test,security,performance}`; each lists its subsystem areas.

- [ ] **Step 3: Sanity-check the board filter**

In GitHub Projects v2 #1, filter by `area/docs` and confirm cards from more than one repo can appear (i.e. the label now exists in multiple repos). Document the result on issue #109.

- [ ] **Step 4: Close out the ticket**

```bash
gh issue comment 109 --repo caliban-ai/caliban --body "All three PRs merged; label-sync applied. Self-id markers (caliban/gonzalo/prospero) present, shared area/* core (docs, ci-cd, test, security, performance) present in all repos, subsystem areas added. Board area/* filtering now consistent cross-repo."
```
Then move the board card to Done (status option id `98236657`) and close the issue once the linked PRs are merged.

---

## Notes for the executor

- These are three **independent** PRs in three **separate** repos. They can be worked and merged in any order; none blocks another.
- No Rust toolchain runs here — do **not** invoke `cargo fmt/clippy/build/test`. The only gate is YAML validity (Step "Validate" in each task).
- `delete-other-labels: true` makes each `labels.yml` authoritative on sync. Every edit in this plan is additive except gonzalo's area color normalization, so no labels are removed from any repo.
- If `python3 -c "import yaml"` fails (PyYAML missing), fall back to: `gh label list` won't validate the file, so use `ruby -ryaml -e 'YAML.load_file(".github/labels.yml")'` or any YAML linter.
