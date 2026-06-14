# ADR Review Wave Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the 2026-05-25 ADR conformance audit and all 6 follow-up workstreams on branch `jf/docs/adr-conformance-audit` as a single sprint-mode PR.

**Architecture:** Atomic commits per workstream (10 expected). Doc-only commits for Sections 1-3, 7 and the ADR amendments in 4-6. Real code in Sections 4 (memory cap), 5 (parallel_conflict_key), and possibly 6 (TUI tick fix or close-out ADR).

**Tech Stack:** Rust workspace (Cargo); `caliban-memory`, `caliban-agent-core`, `caliban-tools-builtin`, `caliban-settings`; ratatui (Section 6); markdown ADRs under `docs/adr/`; spec at `docs/superpowers/specs/2026-05-26-adr-review-wave-design.md`.

**Spec reference:** [`docs/superpowers/specs/2026-05-26-adr-review-wave-design.md`](../specs/2026-05-26-adr-review-wave-design.md)

---

## Pre-work — research only (no commits)

### Task 0.1: Locate the 8 KiB memory constant

**Files:** read-only — `crates/caliban-memory/src/`

- [ ] **Step 1: Find the constant**

Run: `grep -rn "8192\|8 *\* *1024\|8_*KiB\|8_*kib" crates/caliban-memory/src/`

Expected: locate definition (likely `budget.rs` or `lib.rs`). Note file + line.

- [ ] **Step 2: Find all references**

Run: `grep -rn "<constant_name>" crates/caliban-memory/`

Expected: list of call sites. If >2 sites without intermediate type, plan to introduce a `MemoryBudget` struct in Task 4. If 1-2 sites, just change the constant.

- [ ] **Step 3: Check settings schema location**

Run: `grep -rn "memory\|cap_tokens" crates/caliban-settings/src/`

Expected: identify where the `[memory]` section schema lives (likely `schema.rs` + `schema.json`).

### Task 0.2: Verify next free ADR number

**Files:** read-only — `docs/adr/`

- [ ] **Step 1: List ADR files**

Run: `ls docs/adr/ | grep -E '^[0-9]{4}-' | sort | tail -5`

Expected: highest is `0040-*`. Next free is `0041`. If concurrent PR has landed `0041`, shift sequence accordingly.

### Task 0.3: Verify the audit's "21 of 40 misstated" claim

**Files:** read-only

- [ ] **Step 1: Check ADR 0025 body Status**

Run: `grep -m1 '^Status:' docs/adr/0025-*.md`

Expected: `accepted` (per audit's first cluster).

- [ ] **Step 2: Check ADR 0025 status in README**

Run: `grep '0025' docs/adr/README.md`

Expected: status column shows `proposed`. Confirms drift.

- [ ] **Step 3: Check ADR 0019 body Status**

Run: `grep -m1 '^Status:' docs/adr/0019-*.md`

Expected: `proposed` (per audit's second cluster — body matches index, but parity shows shipped).

- [ ] **Step 4: Check ADR 0019 parity row**

Run: `grep -m1 'skills\|Skills' docs/parity-gap-matrix.md`

Expected: row marked ✅.

- [ ] **Step 5: Check ADR 0033 body Status**

Run: `grep -m1 '^Status:' docs/adr/0033-*.md`

Expected: `proposed` (random pick from cluster 2).

If all five expectations hold: the audit's headline is correct, proceed. If any fail: update the audit's TL;DR before committing in Task 1.3.

---

## Section 1 — Audit pre-commit fixes

### Task 1.1: Fix `docs/adr-audit-resume-state.md` stale references

**Files:**
- Modify: `docs/adr-audit-resume-state.md`

- [ ] **Step 1: Update branch parent line**

Find: `Branch:** \`jf/docs/adr-conformance-audit\` (off \`jf/fix/lmstudio-followups\`).`

Replace with: `Branch:** \`jf/docs/adr-conformance-audit\` (off \`main\`, rebased 2026-05-26).`

- [ ] **Step 2: Remove "side note from previous session"**

Delete the entire `## Side note from the previous session` section (heading + body referencing commit `8c05e6f`).

- [ ] **Step 3: Fix dead anchor links**

Replace each occurrence of `[Finding N](#headline-findings-mirror-of-tldr-in-the-audit)` with `[Finding N](2026-05-25-adr-conformance-audit.md#finding-N-<slug>)` using the actual anchor from the audit doc. The audit's findings anchors are:

- `#finding-1-adr-status-drift`
- `#finding-2-two-crate-boundaries-collapsed-without-an-adr-update`
- `#finding-3-aging-defaults-worth-revisiting`
- `#finding-4-tui-tick-papers-over-a-root-cause`
- `#finding-5-is_parallel_safe-deferral-is-reasonable-but-rotting`
- `#finding-6-agpl-30-only-forecloses-library-shaped-reuse`
- `#finding-7-no-adr-for-the-bincli-split-itself`

### Task 1.2: Update audit TL;DR if Task 0.3 invalidated the headline

**Files:**
- Modify (conditional): `docs/2026-05-25-adr-conformance-audit.md`

- [ ] **Step 1: Decide**

If Task 0.3 all five checks passed: skip this task (no edit needed).

If any check failed: update the audit's TL;DR and Finding 1 with the corrected count. Adjust per-row notes for any ADR that's now misclassified.

### Task 1.3: Commit audit docs

- [ ] **Step 1: Stage the three audit files**

```bash
git add docs/2026-05-25-adr-conformance-audit.md docs/adr-audit-resume-state.md
```

- [ ] **Step 2: Commit**

```bash
git commit -m "$(cat <<'EOF'
docs: ADR conformance audit (2026-05-25)

Documentation-only review of all 40 ADRs. For each ADR, answers two
questions: (1) does the current code match the ADR's commitment?
(2) is the decision a good one on its own merit?

Headline: code conformance is generally strong; ADR README status
hygiene is poor (21 of 40 misstated). Seven cross-cutting findings
surfaced, all non-blocking. Resume notes in adr-audit-resume-state.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Expected: commit lands with both files. `git status` shows working tree clean (modulo `.claude/` which stays untracked).

---

## Section 2 — Finding 1: README status reconciliation

### Task 2.1: Inspect current README format

**Files:** read-only

- [ ] **Step 1: Read the ADR index table**

Run: `grep -A 2 -B 1 '^|.*0025' docs/adr/README.md`

Note the column structure (likely `| # | Title | Status |` or similar). Establish exact text format for the status cell so edits match style.

### Task 2.2: Update README status for cluster 1 (body already accepted)

**Files:**
- Modify: `docs/adr/README.md`

Cluster 1 ADRs: `0025, 0026, 0027, 0029, 0030, 0035, 0036, 0037, 0038, 0040`. For each, change the status cell from `proposed` to `accepted`.

- [ ] **Step 1: Update all 10 rows**

Use targeted edits per row to avoid replacing unrelated `proposed` strings.

### Task 2.3: Update README + body Status for cluster 2 (parity shipped)

**Files:**
- Modify: `docs/adr/README.md`
- Modify: `docs/adr/0019-*.md` through `docs/adr/0039-*.md` (11 files)

Cluster 2 ADRs: `0019, 0020, 0021, 0023, 0024, 0028, 0031, 0032, 0033, 0034, 0039`.

- [ ] **Step 1: For each cluster 2 ADR, update README status cell**

Same pattern as Task 2.2. After this step, README shows all 21 ADRs as `accepted`.

- [ ] **Step 2: For each cluster 2 ADR, update body `Status:` line**

For each file, find `Status: proposed` near the top and replace with `Status: accepted`.

### Task 2.4: Verify no inconsistency remains

- [ ] **Step 1: Confirm all body Status lines are non-`proposed` for ADRs claimed accepted**

Run: `for f in docs/adr/00*.md; do printf '%s: ' "$f"; grep -m1 '^Status:' "$f"; done | grep -E 'proposed' | head -20`

Expected: only the genuinely-still-proposed ADRs remain (audit didn't claim everyone is accepted — some ADRs may legitimately be proposed). The 11 from cluster 2 should NOT appear in this output.

- [ ] **Step 2: Confirm README has no `proposed` rows for cluster 1 or 2 ADRs**

Run: `grep -E '^\|.*(0019|0020|0021|0023|0024|0025|0026|0027|0028|0029|0030|0031|0032|0033|0034|0035|0036|0037|0038|0039|0040).*proposed' docs/adr/README.md`

Expected: no output.

### Task 2.5: Commit

- [ ] **Step 1: Stage and commit**

```bash
git add docs/adr/README.md docs/adr/00*.md
git commit -m "$(cat <<'EOF'
docs(adrs): reconcile README status column with bodies and parity matrix

Updates the status column in docs/adr/README.md for 21 ADRs where the
index disagreed with reality:

- 10 ADRs already marked accepted in their body had stale README
  status (0025, 0026, 0027, 0029, 0030, 0035, 0036, 0037, 0038, 0040).
- 11 ADRs were marked proposed everywhere but parity-gap-matrix.md
  shows their rows shipped — promoted both the body Status line and
  the README column to accepted (0019, 0020, 0021, 0023, 0024, 0028,
  0031, 0032, 0033, 0034, 0039).

Per Finding 1 of the 2026-05-25 ADR conformance audit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Section 3 — Finding 2: ADR 0027 and 0029 amendments

### Task 3.1: Read ADR 0027 to find insertion point

**Files:** read-only

- [ ] **Step 1: Inspect file**

Run: `cat docs/adr/0027-*.md | tail -30`

Note the existing tail structure — likely "References" or "Consequences" is the last section. The "Revised" section is appended after the last existing section.

### Task 3.2: Append Revised section to ADR 0027

**Files:**
- Modify: `docs/adr/0027-*.md`

- [ ] **Step 1: Append the Revised section**

Append at the end of the file (after the last existing section):

```markdown

## Revised 2026-05-26

The original Decision committed the Ask modal to a new `caliban-tui-ask`
crate. In practice the modal shipped at `caliban/src/tui/ask.rs` (~202
LOC) inside the binary.

**Why this is the correct outcome.** The modal is binary-coupled (it
consumes the binary's `App` state, dispatches via the binary's `Action`
enum, and renders into the binary's overlay system). Extracting it would
require either threading App/Action/overlay traits through a public
surface or duplicating them — both costs without payoff. The "extract
when sharable" trigger from the original Decision never fired.

**Revisit if** another consumer needs the modal (e.g., a hypothetical
standalone `caliban-tui` library separated from the binary), or LOC
grows past ~500.
```

### Task 3.3: Commit ADR 0027 amendment

- [ ] **Step 1: Stage and commit**

```bash
git add docs/adr/0027-*.md
git commit -m "$(cat <<'EOF'
docs(adr-0027): note Ask modal consolidation in caliban binary

Adds a Revised 2026-05-26 section documenting that the Ask modal
shipped at caliban/src/tui/ask.rs inside the binary (not in a new
caliban-tui-ask crate as the original Decision committed). The
modal is binary-coupled to App, Action, and the overlay system;
extraction would cost without payoff. Per Finding 2 of the
2026-05-25 ADR conformance audit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3.4: Append Revised section to ADR 0029

**Files:**
- Modify: `docs/adr/0029-*.md`

- [ ] **Step 1: Append the Revised section**

Append at the end of the file:

```markdown

## Revised 2026-05-26

The original Decision committed `caliban-auto-mode` to be a new Layer-3
crate. In practice the implementation lives inside `caliban-agent-core`
across `auto_mode.rs`, `mode_filter.rs`, and `permission_mode.rs`
(~1,750 LOC combined).

**Why this is the correct outcome.** Auto-mode dispatch is tightly
coupled to the permission pipeline (`PermissionsHook`,
`SharedPermissionMode`, the soft-deny → Ask handshake) which already
lives in agent-core. Extracting auto-mode would either pull most of the
permission pipeline out with it or introduce a circular dep. The static
rule pre-pass, the classifier dispatch, and the LRU cache all live next
to the data they need.

**Revisit if** auto-mode grows a second consumer (e.g., a non-agent
classifier client), or if the dispatch path becomes a measurable
compile-time burden on `caliban-agent-core`.
```

### Task 3.5: Commit ADR 0029 amendment

- [ ] **Step 1: Stage and commit**

```bash
git add docs/adr/0029-*.md
git commit -m "$(cat <<'EOF'
docs(adr-0029): note auto-mode consolidation in caliban-agent-core

Adds a Revised 2026-05-26 section documenting that auto-mode shipped
inside caliban-agent-core (auto_mode.rs + mode_filter.rs +
permission_mode.rs, ~1,750 LOC) rather than as a separate
caliban-auto-mode crate. The implementation is tightly coupled to the
permission pipeline that already lives in agent-core; extraction would
introduce circular dependencies or duplicate the pipeline. Per
Finding 2 of the 2026-05-25 ADR conformance audit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Section 4 — Finding 3: Memory cap + per-scope knob

### Task 4.1: Read current memory budget implementation

**Files:** read-only — based on Task 0.1 findings

- [ ] **Step 1: Read the budget module**

Read the file located in Task 0.1. Confirm: where is 8 KiB defined, how is it consumed, are there per-tier sub-caps already?

- [ ] **Step 2: Read existing settings schema for memory**

Run: `grep -A 5 '\[memory\]\|memory_settings\|MemorySettings' crates/caliban-settings/src/schema.rs`

Note the existing structure to extend.

### Task 4.2: Write failing test — default is now 32 KiB

**Files:**
- Test: `crates/caliban-memory/src/<budget_module>.rs` (in-file `#[cfg(test)] mod tests`)

- [ ] **Step 1: Add test**

```rust
#[test]
fn default_combined_cap_is_32_kib() {
    let budget = MemoryBudget::default();
    assert_eq!(budget.combined_cap_bytes(), 32 * 1024);
}
```

(If `MemoryBudget` doesn't exist yet, adapt to use the actual current API — e.g., `combined_prefix_cap()` constant.)

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p caliban-memory default_combined_cap_is_32_kib`

Expected: FAIL (current default is 8 KiB).

### Task 4.3: Bump the default constant

**Files:**
- Modify: file from Task 0.1

- [ ] **Step 1: Change the constant**

Find: `8 * 1024` (or `8192`, or whatever Task 0.1 surfaced).

Replace with: `32 * 1024`.

- [ ] **Step 2: Run test**

Run: `cargo test -p caliban-memory default_combined_cap_is_32_kib`

Expected: PASS.

### Task 4.4: Write failing test — per-scope cap_tokens_auto override

**Files:**
- Test: same module as 4.2

- [ ] **Step 1: Add test**

```rust
#[test]
fn cap_tokens_auto_overrides_per_scope() {
    let mut budget = MemoryBudget::default();
    budget.set_cap_tokens_auto(4096);
    assert_eq!(budget.cap_for_scope(MemoryScope::Auto), 4096);
}
```

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p caliban-memory cap_tokens_auto_overrides_per_scope`

Expected: FAIL (method doesn't exist or returns the default).

### Task 4.5: Add per-scope caps to MemoryBudget

**Files:**
- Modify: file from Task 0.1

- [ ] **Step 1: Extend `MemoryBudget` (or introduce it if it was a bare constant)**

Add fields:

```rust
pub struct MemoryBudget {
    combined_cap_bytes: usize,
    cap_tokens_auto: Option<usize>,
    cap_tokens_claude_md: Option<usize>,
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self {
            combined_cap_bytes: 32 * 1024,
            cap_tokens_auto: None,
            cap_tokens_claude_md: None,
        }
    }
}

impl MemoryBudget {
    pub fn combined_cap_bytes(&self) -> usize { self.combined_cap_bytes }
    pub fn set_cap_tokens_auto(&mut self, n: usize) {
        self.cap_tokens_auto = Some(n);
    }
    pub fn set_cap_tokens_claude_md(&mut self, n: usize) {
        self.cap_tokens_claude_md = Some(n);
    }
    pub fn cap_for_scope(&self, scope: MemoryScope) -> usize {
        match scope {
            MemoryScope::Auto => self.cap_tokens_auto.unwrap_or(self.combined_cap_bytes),
            MemoryScope::ClaudeMd => self.cap_tokens_claude_md.unwrap_or(self.combined_cap_bytes),
        }
    }
}

#[derive(Copy, Clone)]
pub enum MemoryScope { Auto, ClaudeMd }
```

(Adapt to actual existing types and call sites identified in Task 4.1.)

- [ ] **Step 2: Run tests**

Run: `cargo test -p caliban-memory`

Expected: both new tests PASS. All existing tests still PASS (or update existing tests that pinned 8 KiB).

### Task 4.6: Write failing test — combined ceiling scales per-tier caps

**Files:**
- Test: same module

- [ ] **Step 1: Add test**

```rust
#[test]
fn combined_ceiling_scales_per_tier_caps_proportionally() {
    let mut budget = MemoryBudget::default();
    budget.set_cap_tokens_auto(20_000);
    budget.set_cap_tokens_claude_md(20_000);
    // Combined ceiling 20 KiB but per-tier sum is 40_000 → scale to 50%
    budget.set_combined_cap_bytes(20 * 1024);
    assert_eq!(budget.effective_cap_for_scope(MemoryScope::Auto), 10 * 1024);
    assert_eq!(budget.effective_cap_for_scope(MemoryScope::ClaudeMd), 10 * 1024);
}
```

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p caliban-memory combined_ceiling_scales_per_tier_caps_proportionally`

Expected: FAIL.

### Task 4.7: Implement proportional scaling

**Files:**
- Modify: same module

- [ ] **Step 1: Add `set_combined_cap_bytes` + `effective_cap_for_scope`**

```rust
impl MemoryBudget {
    pub fn set_combined_cap_bytes(&mut self, n: usize) {
        self.combined_cap_bytes = n;
    }

    pub fn effective_cap_for_scope(&self, scope: MemoryScope) -> usize {
        let raw = self.cap_for_scope(scope);
        let per_tier_sum =
            self.cap_tokens_auto.unwrap_or(self.combined_cap_bytes)
            + self.cap_tokens_claude_md.unwrap_or(self.combined_cap_bytes);
        if per_tier_sum <= self.combined_cap_bytes {
            return raw;
        }
        // scale down proportionally
        (raw as u128 * self.combined_cap_bytes as u128 / per_tier_sum as u128) as usize
    }
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p caliban-memory combined_ceiling_scales_per_tier_caps_proportionally`

Expected: PASS.

### Task 4.8: Wire MemoryBudget into existing call sites

**Files:**
- Modify: each call site identified in Task 0.1

- [ ] **Step 1: Replace bare-constant uses**

At each site that previously used the 8 KiB constant directly (now 32 KiB), switch to consulting `MemoryBudget::effective_cap_for_scope(...)` where the consumer knows the scope, or `combined_cap_bytes()` for the combined sum case.

- [ ] **Step 2: Run all memory tests**

Run: `cargo test -p caliban-memory`

Expected: all tests pass; truncation tests still pass against the 32 KiB default.

### Task 4.9: Add settings schema fields

**Files:**
- Modify: `crates/caliban-settings/src/schema.rs`
- Modify: `crates/caliban-settings/schema.json` (if separate)

- [ ] **Step 1: Extend the `[memory]` schema**

Add to the memory settings struct:

```rust
#[serde(default)]
pub cap_tokens_auto: Option<usize>,
#[serde(default)]
pub cap_tokens_claude_md: Option<usize>,
#[serde(default)]
pub cap_tokens_combined: Option<usize>,
```

- [ ] **Step 2: Mirror in `schema.json`**

```json
"cap_tokens_auto":     { "type": "integer", "minimum": 0 },
"cap_tokens_claude_md":{ "type": "integer", "minimum": 0 },
"cap_tokens_combined": { "type": "integer", "minimum": 0 }
```

- [ ] **Step 3: Settings → MemoryBudget conversion**

Wherever the settings are converted to `MemoryBudget`, plumb the three new fields through using `set_cap_tokens_auto`, `set_cap_tokens_claude_md`, `set_combined_cap_bytes`.

### Task 4.10: Write integration test — settings parsing

**Files:**
- Test: `crates/caliban-settings/tests/` (or in-module test)

- [ ] **Step 1: Add test**

```rust
#[test]
fn parses_cap_tokens_settings() {
    let toml = r#"
[memory]
cap_tokens_auto = 16384
cap_tokens_claude_md = 16384
cap_tokens_combined = 32768
"#;
    let parsed: Settings = toml::from_str(toml).unwrap();
    assert_eq!(parsed.memory.cap_tokens_auto, Some(16384));
    assert_eq!(parsed.memory.cap_tokens_claude_md, Some(16384));
    assert_eq!(parsed.memory.cap_tokens_combined, Some(32768));
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p caliban-settings parses_cap_tokens_settings`

Expected: PASS.

### Task 4.11: Append Revised section to ADR 0018

**Files:**
- Modify: `docs/adr/0018-*.md`

- [ ] **Step 1: Append**

```markdown

## Revised 2026-05-26

Bumped the combined-prefix default from 8 KiB to 32 KiB. The 8 KiB
default was conservative against 2024 context windows and was
increasingly punishing in 2026 (1M-token Sonnet, 200K standard on most
providers). Truncation-first behavior was dropping the auto-memory
index — exactly the tier that grows.

Added per-scope token caps via three optional `[memory]` settings:
`cap_tokens_auto`, `cap_tokens_claude_md`, `cap_tokens_combined`. When
the per-tier sum exceeds `cap_tokens_combined`, per-tier caps scale
down proportionally rather than silently dropping a tier.

Truncation order within a tier is unchanged from the original Decision.
```

### Task 4.12: Update parity-matrix row (if memory is tracked)

**Files:**
- Modify (conditional): `docs/parity-gap-matrix.md`

- [ ] **Step 1: Search for memory row**

Run: `grep -n 'memory\|Memory' docs/parity-gap-matrix.md | head -10`

If a row mentions the 8 KiB cap explicitly, update to reflect 32 KiB default + the new knob. If no row references the cap, skip.

### Task 4.13: Run full memory + settings test sweep

- [ ] **Step 1: Test**

Run: `cargo test -p caliban-memory -p caliban-settings`

Expected: all pass.

- [ ] **Step 2: Clippy**

Run: `cargo clippy -p caliban-memory -p caliban-settings --all-targets`

Expected: clean.

### Task 4.14: Commit Section 4

- [ ] **Step 1: Stage and commit**

```bash
git add crates/caliban-memory/ crates/caliban-settings/ docs/adr/0018-*.md docs/parity-gap-matrix.md
git commit -m "$(cat <<'EOF'
feat(memory): raise default cap to 32 KiB + add per-scope cap_tokens

- Bumps combined-prefix default from 8 KiB to 32 KiB (ADR 0018).
- Adds three optional [memory] settings: cap_tokens_auto,
  cap_tokens_claude_md, cap_tokens_combined.
- When per-tier sum exceeds the combined ceiling, per-tier caps scale
  down proportionally instead of silently dropping a tier.
- ADR 0018 amended with a Revised 2026-05-26 section.

Per Finding 3 of the 2026-05-25 ADR conformance audit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Section 5 — Finding 5: per-target `parallel_conflict_key`

### Task 5.1: Read the tool trait + parallel dispatcher

**Files:** read-only

- [ ] **Step 1: Find the tool trait**

Run: `grep -rn 'trait Tool' crates/caliban-tools-builtin/src/ crates/caliban-agent-core/src/`

Expected: located. Read its method list.

- [ ] **Step 2: Find the parallel dispatcher**

Run: `grep -rn 'parallel_tools\|join_all\|FuturesUnordered' crates/caliban-agent-core/src/`

Expected: locate the dispatch loop. Note where in the per-batch flow the grouping pass should go.

- [ ] **Step 3: Inspect each write tool**

Read: `crates/caliban-tools-builtin/src/{edit,write,multi_edit,notebook_edit,write_memory_topic}.rs` (or wherever they live; adapt paths).

Note the input struct each tool uses (`EditInput { path, old_string, new_string }` etc.) — needed for the override implementations.

### Task 5.2: Add the trait method (default None)

**Files:**
- Modify: the tool trait file from Task 5.1

- [ ] **Step 1: Add method to trait**

```rust
/// Returns Some(key) if this tool call has a conflict identity that must
/// not run in parallel with another call using the same key. Returns None
/// when the tool is fully parallel-safe (the default).
fn parallel_conflict_key(&self, _input: &ToolInput) -> Option<String> {
    None
}
```

(Adapt `ToolInput` to the trait's actual input parameter type.)

- [ ] **Step 2: Build to confirm no existing tool breaks**

Run: `cargo build --workspace`

Expected: clean build. Default `None` preserves existing behavior.

### Task 5.3: Write failing test — Edit(a) + Edit(b) parallel

**Files:**
- Test: `crates/caliban-agent-core/tests/parallel_conflict_key.rs` (new integration test)

- [ ] **Step 1: Write test that asserts parallel execution**

```rust
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[tokio::test]
async fn edits_on_different_files_run_in_parallel() {
    let start = Instant::now();
    // Construct two Edit calls on different files. Each Edit sleeps 200 ms
    // (use a TestTool with parallel_conflict_key returning the path).
    let calls = vec![
        slow_edit_call("/tmp/a.rs", Duration::from_millis(200)),
        slow_edit_call("/tmp/b.rs", Duration::from_millis(200)),
    ];
    let dispatcher = ParallelDispatcher::new(/* parallel_tools = 2 */);
    dispatcher.dispatch(calls).await;
    // If serial: ~400 ms. If parallel: ~200 ms.
    assert!(start.elapsed() < Duration::from_millis(350),
        "expected parallel execution, took {:?}", start.elapsed());
}
```

(Adapt `slow_edit_call` and `ParallelDispatcher` to actual APIs. If a test harness for the dispatcher doesn't exist, build a minimal one with a `TestTool` whose `parallel_conflict_key` overrides match what `Edit` will return.)

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p caliban-agent-core edits_on_different_files_run_in_parallel`

Expected: FAIL — either no grouping yet (everything in `None` group = parallel, test passes accidentally) OR test infrastructure missing.

If the test passes without any dispatcher changes, that's expected: with default `None`, everything parallelizes. We'll add the grouping pass and same-key test in Task 5.5.

### Task 5.4: Implement the dispatcher grouping pass

**Files:**
- Modify: dispatcher file from Task 5.1

- [ ] **Step 1: Add grouping in the parallel dispatch path**

Replace the existing parallel-fanout block with a grouping pass:

```rust
use std::collections::HashMap;

let mut groups: HashMap<Option<String>, Vec<Call>> = HashMap::new();
for call in batch {
    let key = call.tool.parallel_conflict_key(&call.input);
    groups.entry(key).or_default().push(call);
}

let group_futures = groups.into_iter().map(|(key, calls)| async move {
    if key.is_none() {
        // None group: full parallel via the existing semaphore + join
        futures::future::join_all(calls.into_iter().map(run_call)).await
    } else {
        // Conflict-keyed group: serial preserving submission order
        let mut results = Vec::with_capacity(calls.len());
        for call in calls {
            results.push(run_call(call).await);
        }
        results
    }
});

let nested = futures::future::join_all(group_futures).await;
// Flatten nested results while preserving original submission order if needed
```

(If submission-order preservation matters at the batch level — confirm via existing tests — sort the flattened result by the call's original index.)

- [ ] **Step 2: Run existing dispatcher tests**

Run: `cargo test -p caliban-agent-core`

Expected: existing tests still pass (no `parallel_conflict_key` overrides yet, so all calls land in the `None` group and behave identically).

### Task 5.5: Add Edit / Write / MultiEdit overrides

**Files:**
- Modify: `crates/caliban-tools-builtin/src/edit.rs`, `write.rs`, `multi_edit.rs`

- [ ] **Step 1: Helper for canonical path keying**

In a shared module (e.g., `crates/caliban-tools-builtin/src/parallel.rs` — new file):

```rust
use std::path::{Path, PathBuf};

/// Canonicalize a file path for use as a parallel_conflict_key.
/// Falls back to canonicalize(parent) + filename when the file doesn't
/// exist yet (e.g., Write creating a new file).
pub fn canonical_key(path: &Path) -> String {
    if let Ok(c) = path.canonicalize() {
        return c.display().to_string();
    }
    let parent = path.parent().and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let file = path.file_name().map(|f| f.to_owned()).unwrap_or_default();
    parent.join(file).display().to_string()
}
```

- [ ] **Step 2: Override in Edit**

In `edit.rs`:

```rust
fn parallel_conflict_key(&self, input: &EditInput) -> Option<String> {
    Some(crate::parallel::canonical_key(&input.path))
}
```

- [ ] **Step 3: Override in Write**

Same pattern in `write.rs` using `WriteInput::path`.

- [ ] **Step 4: Override in MultiEdit**

Same pattern in `multi_edit.rs` using `MultiEditInput::path`.

### Task 5.6: Test — Edit(a) + Edit(a) serial

**Files:**
- Test: `crates/caliban-agent-core/tests/parallel_conflict_key.rs`

- [ ] **Step 1: Add test**

```rust
#[tokio::test]
async fn edits_on_same_file_run_serially() {
    let start = Instant::now();
    let calls = vec![
        slow_edit_call("/tmp/a.rs", Duration::from_millis(200)),
        slow_edit_call("/tmp/a.rs", Duration::from_millis(200)),
    ];
    let dispatcher = ParallelDispatcher::new();
    dispatcher.dispatch(calls).await;
    // Serial: ~400 ms.
    assert!(start.elapsed() >= Duration::from_millis(380),
        "expected serial execution, took {:?}", start.elapsed());
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p caliban-agent-core edits_on_same_file_run_serially`

Expected: PASS.

### Task 5.7: Test — Edit + Read on same file parallel

**Files:**
- Test: same file

- [ ] **Step 1: Add test**

```rust
#[tokio::test]
async fn edit_and_read_on_same_file_run_in_parallel() {
    let start = Instant::now();
    let calls = vec![
        slow_edit_call("/tmp/a.rs", Duration::from_millis(200)),
        slow_read_call("/tmp/a.rs", Duration::from_millis(200)),
    ];
    let dispatcher = ParallelDispatcher::new();
    dispatcher.dispatch(calls).await;
    assert!(start.elapsed() < Duration::from_millis(350));
}
```

- [ ] **Step 2: Run**

Expected: PASS (Read keeps default `None`, ends up in None group, parallelizes).

### Task 5.8: Test — symlink collapse to serial

**Files:**
- Test: same file

- [ ] **Step 1: Add test**

```rust
#[tokio::test]
async fn symlink_collapses_to_same_conflict_key() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.txt");
    std::fs::write(&real, "x").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let start = Instant::now();
    let calls = vec![
        slow_edit_call(&real, Duration::from_millis(200)),
        slow_edit_call(&link, Duration::from_millis(200)),
    ];
    let dispatcher = ParallelDispatcher::new();
    dispatcher.dispatch(calls).await;
    assert!(start.elapsed() >= Duration::from_millis(380));
}
```

(Gate with `#[cfg(unix)]` since symlinks differ on Windows.)

- [ ] **Step 2: Run**

Expected: PASS.

### Task 5.9: Add NotebookEdit override

**Files:**
- Modify: `crates/caliban-tools-builtin/src/notebook_edit.rs`

- [ ] **Step 1: Add override**

```rust
fn parallel_conflict_key(&self, input: &NotebookEditInput) -> Option<String> {
    Some(crate::parallel::canonical_key(&input.notebook_path))
}
```

### Task 5.10: Add WriteMemoryTopic override

**Files:**
- Modify: `crates/caliban-tools-builtin/src/write_memory_topic.rs`

- [ ] **Step 1: Add override**

```rust
fn parallel_conflict_key(&self, input: &WriteMemoryTopicInput) -> Option<String> {
    Some(format!("memory:{}:{}", input.scope, input.topic))
}
```

(Adapt to actual field names + the `scope` enum's `Display` impl.)

### Task 5.11: Test — WriteMemoryTopic same scope+topic serial; different parallel

**Files:**
- Test: same file

- [ ] **Step 1: Add two tests**

```rust
#[tokio::test]
async fn write_memory_topic_same_topic_serial() {
    let start = Instant::now();
    let calls = vec![
        slow_memory_write_call("user", "foo", Duration::from_millis(200)),
        slow_memory_write_call("user", "foo", Duration::from_millis(200)),
    ];
    ParallelDispatcher::new().dispatch(calls).await;
    assert!(start.elapsed() >= Duration::from_millis(380));
}

#[tokio::test]
async fn write_memory_topic_different_topics_parallel() {
    let start = Instant::now();
    let calls = vec![
        slow_memory_write_call("user", "foo", Duration::from_millis(200)),
        slow_memory_write_call("user", "bar", Duration::from_millis(200)),
    ];
    ParallelDispatcher::new().dispatch(calls).await;
    assert!(start.elapsed() < Duration::from_millis(350));
}
```

- [ ] **Step 2: Run**

Expected: PASS.

### Task 5.12: Test — cross-namespace (Edit + WriteMemoryTopic) parallel

**Files:**
- Test: same file

- [ ] **Step 1: Add test**

```rust
#[tokio::test]
async fn edit_and_memory_write_run_in_parallel() {
    let start = Instant::now();
    let calls = vec![
        slow_edit_call("/tmp/a.rs", Duration::from_millis(200)),
        slow_memory_write_call("user", "foo", Duration::from_millis(200)),
    ];
    ParallelDispatcher::new().dispatch(calls).await;
    assert!(start.elapsed() < Duration::from_millis(350));
}
```

- [ ] **Step 2: Run**

Expected: PASS (different key namespaces).

### Task 5.13: Append Revised section to ADR 0016

**Files:**
- Modify: `docs/adr/0016-*.md`

- [ ] **Step 1: Append**

```markdown

## Revised 2026-05-26

The original Decision deferred a per-tool `is_parallel_safe()` flag,
noting that no built-in had write contention. That observation was true
in 2024 (Bash/Read/Grep/Glob). It is no longer true: ADRs 0028 + 0035
introduced Edit/Write/MultiEdit/NotebookEdit/WriteMemoryTopic, all of
which can collide on the same target within one turn.

**Revised mechanism:** `parallel_conflict_key(&self, input) -> Option<String>`
on the tool trait. Returns `None` for fully parallel-safe tools (the
default; matches the original 2024 posture). Returns a conflict-identity
string for tools whose effect is keyed to a target — typically the
canonicalized path; for `WriteMemoryTopic` a `memory:{scope}:{topic}`
string. The dispatcher groups batch calls by key; `None` group
parallelizes freely, each non-`None` key group runs serially. Different
keys parallelize against each other.

**What this preserves.** Read/Grep/Glob/Bash continue to behave exactly
as before (default `None`). Two `Edit`s on different files still
parallelize. The parallel-tools differentiator from Claude Code is
intact.
```

### Task 5.14: Workspace test + clippy

- [ ] **Step 1: Workspace test**

Run: `cargo test --workspace`

Expected: all pass.

- [ ] **Step 2: Workspace clippy**

Run: `cargo clippy --workspace --all-targets`

Expected: clean.

### Task 5.15: Commit Section 5

- [ ] **Step 1: Stage and commit**

```bash
git add crates/caliban-tools-builtin/ crates/caliban-agent-core/ docs/adr/0016-*.md
git commit -m "$(cat <<'EOF'
feat(tools): per-target parallel_conflict_key gates same-file write collisions

Adds parallel_conflict_key(&self, input) -> Option<String> to the tool
trait (default None). Overrides in Edit/Write/MultiEdit/NotebookEdit
return the canonicalized absolute path; WriteMemoryTopic returns
"memory:{scope}:{topic}". The parallel dispatcher groups batch calls
by key — None group fully parallelizes (subject to the existing
parallel_tools semaphore); each non-None key group runs serially in
submission order; groups run in parallel against each other.

Net behavior:
- Edit(a.rs) + Edit(b.rs)         → parallel (different keys)
- Edit(a.rs) + Edit(a.rs)         → serial   (same key)
- Edit(a.rs) + Read(a.rs)         → parallel (Read = None)
- Edit(real) + Edit(symlink_real) → serial   (canonicalize collapses)
- WriteMemoryTopic same scope+topic → serial; different → parallel

ADR 0016 amended with a Revised 2026-05-26 section. Per Finding 5
of the 2026-05-25 ADR conformance audit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Section 6 — Finding 4: TUI tick investigation (time-boxed)

### Task 6.1: Locate the 50 ms tick

**Files:** read-only

- [ ] **Step 1: Find tick**

Run: `grep -rn '50\|Duration::from_millis(50)\|tick' caliban/src/tui*` then narrow to the 50 ms tick referenced in ADR 0014.

Note exact file + line.

### Task 6.2: Disable the tick locally

**Files:**
- Modify (TEMPORARY, do not commit yet): the file from Task 6.1

- [ ] **Step 1: Comment out the tick**

Comment out the line that spawns / awaits the 50 ms tick. Leave a `// TODO: ADR review wave Section 6 test` marker.

- [ ] **Step 2: Verify build**

Run: `cargo build -p caliban`

Expected: clean build.

### Task 6.3: Run debug-enabled session

- [ ] **Step 1: Build with debug logging enabled**

Run: `RUST_LOG=caliban=debug,caliban_agent_core=debug cargo run --bin caliban`

- [ ] **Step 2: Drive 3-4 streaming completions**

Manually drive prompts that historically caused stalls. Suggested prompts:

- "Write a 200-word essay about Rust's ownership system." (long single-message stream)
- A turn that calls a tool then waits for follow-up text.
- A reasoning-heavy prompt against a Qwen3 or DeepSeek-R1 model if available locally.

Watch for: cursor freeze, missing token render, stuck `streaming...` indicator.

- [ ] **Step 3: Decide**

If no stalls observed after ~30 minutes of varied prompting: Task 6.4 (fix path skipped, close-out ADR). If stalls observed: Task 6.5 (root-cause hunt).

### Task 6.4: Restore tick + write close-out ADR (fallback path)

Only if Task 6.3 found NO stalls.

**Files:**
- Modify: file from Task 6.1 (restore the tick — `git checkout -- <file>`)
- Create: `docs/adr/<NNNN>-tui-redraw-tick-closeout.md` (number from Task 0.2)

- [ ] **Step 1: Restore tick**

Run: `git checkout -- caliban/src/tui*` (or whichever file was modified in Task 6.2)

- [ ] **Step 2: Write close-out ADR**

Create `docs/adr/0041-tui-redraw-tick-closeout.md` (adjust number):

```markdown
# 0041 — TUI redraw tick close-out

Status: accepted
Date: 2026-05-26
Supersedes: portions of 0014

## Context

ADR 0014 introduced a 50 ms redraw tick into the TUI event loop as a
workaround for stalls observed during streaming completions. The same
ADR explicitly acknowledged the tick "masks the symptom rather than
addressing the root cause" and pointed at a probable missing-waker bug
in `async_stream::try_stream!` as the likely culprit.

Two years on, no follow-up ADR closed the question. The 2026-05-25 ADR
conformance audit (Finding 4) flagged the unresolved status.

## Decision

The 50 ms redraw tick stays.

A time-boxed investigation (~30 minutes of varied streaming prompts
with the tick disabled) under debug logging produced no observable
stalls. The original symptom was not reproducible on the current code,
likely because related changes (the async-stream upgrade in 2025, the
ratatui upgrade, the streaming-parser rewrite) addressed the underlying
issue indirectly.

The tick's cost is negligible (~10 µs every 50 ms = 0.02% CPU) and the
behavior is observably correct. Removing the tick would risk silent
regression for marginal cleanup gain.

## Consequences

- Tick remains in the TUI event loop.
- ADR 0014's open question is now closed.
- Revisit if a contributor identifies a reproducible stall under
  specific conditions, or if a future async-stream change reintroduces
  the problem.

## References

- ADR 0014 (original tick decision).
- 2026-05-25 ADR conformance audit, Finding 4.
```

- [ ] **Step 3: Commit close-out**

```bash
git add docs/adr/0041-tui-redraw-tick-closeout.md
git commit -m "$(cat <<'EOF'
docs(adr-0041): close out 50 ms TUI redraw tick

Time-boxed investigation under debug logging (tick disabled, ~30 min
of varied streaming prompts) produced no observable stalls. Likely the
underlying issue from ADR 0014 was addressed indirectly by subsequent
async-stream / ratatui / streaming-parser updates. Tick's cost is
negligible; removing it would risk silent regression for marginal
cleanup gain. Tick stays; ADR 0014's open question is closed.

Per Finding 4 of the 2026-05-25 ADR conformance audit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 6.5: Root-cause hunt (fix path)

Only if Task 6.3 found stalls.

**Files:**
- Modify: source files in `crates/caliban-agent-core/src/stream.rs` or wherever the `async_stream::try_stream!` call sites live.

- [ ] **Step 1: Grep for try_stream call sites**

Run: `grep -rn 'try_stream!' crates/caliban-agent-core/src/ crates/caliban-provider-*/src/`

- [ ] **Step 2: Read each call site**

Look for any future construction that polls without storing a waker, or any channel reads that don't register a waker on `Pending`.

- [ ] **Step 3: Apply fix**

The exact fix depends on what's found. Typical patterns:
- Replace a custom future with `tokio::select!` over the upstream stream and a sleeping branch (no manual polling).
- Ensure `Pin<Box<dyn Stream>>` wrappers thread `cx.waker()` through.
- Drop a manual `noop_waker` if one is in place.

- [ ] **Step 4: Run with tick still disabled**

Repeat Task 6.3 prompts. Confirm no stalls.

- [ ] **Step 5: Remove the tick for good**

Delete the previously-commented-out tick (and any related timer setup).

- [ ] **Step 6: Write a regression test if locatable**

If the bug was in a specific function with reproducible inputs (e.g., a streaming-parser path), add a unit test under that crate that asserts the waker is registered or the parked task wakes on the next chunk.

- [ ] **Step 7: Append Revised section to ADR 0014**

```markdown

## Revised 2026-05-26

Root cause identified and fixed: <one-line summary>. The 50 ms redraw
tick is removed. ADR 0014's "If the underlying cause is something
deeper..." open question is now answered.

See commit <SHA> for the fix.
```

- [ ] **Step 8: Commit**

```bash
git add crates/caliban-agent-core/ caliban/src/tui* docs/adr/0014-*.md
git commit -m "$(cat <<'EOF'
fix(tui): <one-line root cause> — remove 50 ms redraw tick

ADR 0014's "If the underlying cause is something deeper" hypothesis
was correct. Root cause: <one-line>. Fix: <one-line>. Tick removed;
ADR 0014 amended with a Revised section.

Per Finding 4 of the 2026-05-25 ADR conformance audit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Section 7 — Finding 7: Three missing ADRs

### Task 7.1: Confirm ADR numbering after Section 6

- [ ] **Step 1: List ADRs again**

Run: `ls docs/adr/ | grep -E '^[0-9]{4}-' | sort | tail -3`

If Section 6 added 0041, the next free is 0042. Otherwise 0041.

### Task 7.2: Write ADR for caliband sibling-binary placement

**Files:**
- Create: `docs/adr/<NNNN>-caliband-binary-placement.md`

- [ ] **Step 1: Write the ADR**

```markdown
# <NNNN> — `caliband` sibling-binary placement

Status: accepted
Date: 2026-05-26

## Context

The workspace declares two binaries:
- `caliban` — primary user-facing TUI/CLI, located at the workspace root.
- `caliband` — supervisor daemon, located at
  `crates/caliban-supervisor/src/bin/caliband.rs`.

ADR 0005 ("Workspace layout") establishes that "primary" binaries
live at the workspace root. `caliband` does not — it lives nested
under its owning crate. ADR 0037 introduces the daemon obliquely but
does not document the placement choice.

## Decision

`caliband` stays nested under `caliban-supervisor` as a secondary
binary.

## Consequences

- Clean process boundary between the user-facing CLI and the daemon.
- Shared crate compilation: `caliband` reuses `caliban-supervisor`'s
  modules directly without re-exporting.
- No startup path conflict: launching `caliban` never accidentally
  invokes `caliband`'s `main`.
- `cargo install` requires `--bin caliband` explicitly. Documented in
  the supervisor crate README.
- The workspace root stays focused on the primary product surface.

## References

- ADR 0005 (workspace layout).
- ADR 0037 (subagent worktree isolation + fleet — introduces the
  daemon).
- 2026-05-25 ADR conformance audit, Finding 7.
```

- [ ] **Step 2: Update `docs/adr/README.md` index**

Add the new ADR's row to the index table.

- [ ] **Step 3: Commit**

```bash
git add docs/adr/<NNNN>-caliband-binary-placement.md docs/adr/README.md
git commit -m "$(cat <<'EOF'
docs(adr-<NNNN>): caliband sibling-binary placement

Documents the decision (made in code, never recorded) to nest the
caliband supervisor daemon under crates/caliban-supervisor/src/bin/
rather than at the workspace root. Caliband is secondary to the
user-facing caliban CLI and semantically belongs to the supervisor
crate.

Per Finding 7 of the 2026-05-25 ADR conformance audit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 7.3: Write ADR for arc-swap

**Files:**
- Create: `docs/adr/<NNNN>-arc-swap-shared-state.md`

- [ ] **Step 1: Identify call sites**

Run: `grep -rn 'ArcSwap\|arc_swap::' crates/`

Note the surfaces using `ArcSwap` (settings overlay, model router routes, plugin registry — confirm against actual results).

- [ ] **Step 2: Write the ADR**

```markdown
# <NNNN> — `arc-swap` as shared-state primitive

Status: accepted
Date: 2026-05-26

## Context

Several read-mostly shared-state surfaces in the workspace use
`arc_swap::ArcSwap` rather than `tokio::sync::RwLock`:

- <list of surfaces from Step 1>

The choice was made per-surface during the parity sweep but never
documented at the workspace level.

## Decision

Prefer `arc-swap` for shared state where:
- Readers outnumber writers by >10×.
- Writers can tolerate replacing the entire `Arc` (not partial
  mutation).
- Read latency is on the hot path (avoiding even an uncontended lock
  acquisition matters).

Use `tokio::sync::RwLock` for surfaces with frequent partial mutation
or where writer fairness matters.

## Consequences

- Lock-free reads: every `load()` returns a cheap `Arc` clone with no
  contention.
- No priority inversion under load (reads never block writers; writers
  never block readers).
- Slightly higher memory churn on writes: each `store` allocates a new
  `Arc`. Acceptable for our config-reload / route-update use cases
  where writes are infrequent.
- No fairness guarantees between concurrent writers. Acceptable
  because writers are rare.
- Slight cognitive load for new contributors unfamiliar with the
  semantics (load returns a snapshot, not a live reference).

## References

- `arc-swap` crate documentation: https://docs.rs/arc-swap
- 2026-05-25 ADR conformance audit, Finding 7.
```

- [ ] **Step 3: Update `docs/adr/README.md` index**

- [ ] **Step 4: Commit**

```bash
git add docs/adr/<NNNN>-arc-swap-shared-state.md docs/adr/README.md
git commit -m "$(cat <<'EOF'
docs(adr-<NNNN>): arc-swap as shared-state primitive

Documents the per-surface choice (made in code, never recorded at the
workspace level) to use arc_swap::ArcSwap for read-mostly shared state
where readers outnumber writers by >10x and writers can tolerate full
Arc replacement. tokio::sync::RwLock remains the choice for surfaces
with frequent partial mutation or writer fairness needs.

Per Finding 7 of the 2026-05-25 ADR conformance audit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 7.4: Write ADR for rmcp 1.7 pin

**Files:**
- Create: `docs/adr/<NNNN>-rmcp-version-pin.md`

- [ ] **Step 1: Confirm current pin**

Run: `grep -A 1 'rmcp' Cargo.toml`

Note actual version string.

- [ ] **Step 2: Write the ADR**

```markdown
# <NNNN> — `rmcp` 1.7 version pin

Status: accepted
Date: 2026-05-26

## Context

`caliban-mcp-client` depends on `rmcp` (the Model Context Protocol
Rust SDK). The workspace pins `rmcp = "1.7"` rather than tracking the
latest minor version automatically.

## Decision

Pin `rmcp` at the `1.7.x` minor. Bump in a single dedicated PR after
manual review of the changelog and any breaking changes affecting our
MCP transport, OAuth, elicitation, or resource surface.

## Consequences

- Insulation from breaking changes in MCP transport or server APIs
  between rmcp releases.
- Manual maintenance cost: each bump requires changelog review +
  integration test pass + a dedicated PR.
- Predictable behavior for users running pinned binaries against
  established MCP servers.
- Risk: lagging behind upstream means missing protocol-level
  enhancements (e.g., new transport modalities) until we explicitly
  bump.

## References

- `rmcp` crate: https://crates.io/crates/rmcp
- ADR 0017 (MCP stdio v1) and ADR 0023 (MCP v2) for the surface that
  consumes rmcp.
- 2026-05-25 ADR conformance audit, Finding 7.
```

- [ ] **Step 3: Update `docs/adr/README.md` index**

- [ ] **Step 4: Commit**

```bash
git add docs/adr/<NNNN>-rmcp-version-pin.md docs/adr/README.md
git commit -m "$(cat <<'EOF'
docs(adr-<NNNN>): rmcp 1.7 version pin

Documents the choice (made in Cargo.toml, never recorded) to pin
rmcp at 1.7.x rather than tracking the latest minor automatically.
Bumps are manual + dedicated-PR after changelog review.

Per Finding 7 of the 2026-05-25 ADR conformance audit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Post-work — final verification

### Task 8.1: Whole-workspace test + clippy

- [ ] **Step 1: Workspace test**

Run: `cargo test --workspace`

Expected: all pass.

- [ ] **Step 2: Workspace clippy**

Run: `cargo clippy --workspace --all-targets`

Expected: clean.

- [ ] **Step 3: Format check**

Run: `cargo fmt --all --check`

Expected: no diff. If diff: `cargo fmt --all` and amend the most recent code commit.

### Task 8.2: Verify commit sequence

- [ ] **Step 1: Review log**

Run: `git log --oneline main..HEAD`

Expected: 9 or 10 commits matching the planned sequence (10 if Section 6 took the fix path, 9 if it produced both the close-out ADR and the audit commit as planned).

### Task 8.3: Stop

Push and PR creation are user-triggered. The plan does not push or create a PR.

---

## Notes for the executor

- **Sprint mode is active.** Do not pause between sections for review unless a step's expected output diverges materially from the actual.
- **Re-rebase if main moves.** If `git pull --rebase origin main` would conflict, stop and surface the diff before proceeding.
- **Section 6 is the most likely place for time-box bleed.** Hold the 1-hour cap firmly. The close-out ADR is a fine outcome.
- **Adapt to actual call signatures.** Several tasks (4.x, 5.x) reference type names like `MemoryBudget`, `ToolInput`, `EditInput` that may need adjustment to match the workspace's actual conventions. The shape of the test + the override behavior matters; the exact identifier name doesn't.
