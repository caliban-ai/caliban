# Eval Follow-ups Implementation Plan (#239, #240, #241)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land three independent eval-driven harness fixes — retry transient transport errors (#241), whitespace-tolerant Edit matching + near-miss feedback (#240), and a no-edit-progress nudge/telemetry (#239) — as one sprint branch, three logical commits.

**Architecture:** Each ticket touches a distinct crate: `caliban-agent-core` (retry + agent loop), `caliban-tools-builtin` (Edit/MultiEdit), and `caliban` (headless ResultFrame). Tasks are mostly independent and can be reviewed in isolation.

**Tech Stack:** Rust workspace, `tokio`, `tracing`, `wiremock`/`MockProvider` for tests.

## Global Constraints

- Author commits as `john.ford2002@gmail.com` (already the repo `user.email`).
- The #239 nudge text MUST be neutral — it must NOT instruct the model to skip builds, tests, or verification.
- No new third-party dependency for #240 (implement Levenshtein/diff helper in-crate).
- Exact-match behavior of Edit/MultiEdit is preserved unchanged; fuzzy tiers run only when exact count == 0.
- Default behavior must not regress: edit-producing runs see no nudge before the threshold; 4xx/auth still fail fast.
- Full local gate passes before any push: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, `cargo test --workspace`.

---

## Task 1: #241 — retry transient 5xx (incl. 500) + observable per-attempt warning

**Files:**
- Modify: `crates/caliban-agent-core/src/retry.rs` (`is_retryable`, `with_retry`)
- Test: `crates/caliban-agent-core/tests/retry_backoff.rs`

**Interfaces:**
- Consumes: `caliban_provider::Error` (variants `ServerError { status, body }`, `Auth`, `InvalidRequest`, `RateLimit`, `Network`, `StreamInterrupted`).
- Produces: unchanged public signatures of `is_retryable` and `with_retry`.

- [ ] **Step 1: Write failing unit tests** in `retry.rs` (`#[cfg(test)]`) or `retry_backoff.rs`:
  - `is_retryable(ServerError { status: 500, .. })` is `true`.
  - `is_retryable(ServerError { status: 503, .. })` is `true`.
  - `is_retryable(InvalidRequest(...))` and `is_retryable(Auth(...))` are `false`.
  - A `with_retry` test where the closure returns `ServerError{500}` once then `Ok` → succeeds after 1 retry (use `RetryPolicy { max_attempts: 3, initial_backoff: Duration::from_millis(0), jitter: false, .. }` to keep it fast); a closure returning `InvalidRequest` → returns immediately after 1 call (assert call count == 1).

- [ ] **Step 2: Run tests, verify the 500 cases fail** (currently `502..=599`).
  Run: `cargo test -p caliban-agent-core retry`

- [ ] **Step 3: Implement.** In `is_retryable`, change the `ServerError` arm from `status: 502..=599` to `status: 500..=599`. In `with_retry`, in the retry branch (after deciding to retry, before/around the `tokio::select!` sleep), add `tracing::warn!(attempt, backoff_ms = sleep_d.as_millis() as u64, error = %e, "provider call failed; retrying");` (use the error's Display; do not move `e` before `last_err = Some(e)`).

- [ ] **Step 4: Run tests, verify pass.**
  Run: `cargo test -p caliban-agent-core retry`

- [ ] **Step 5: Commit.**
  ```bash
  git add crates/caliban-agent-core/src/retry.rs crates/caliban-agent-core/tests/retry_backoff.rs
  git commit -m "fix(providers): retry transient 5xx (incl. 500) with observable per-attempt warning (#241)"
  ```

---

## Task 2: #240 — shared tiered matcher `match_old` (unit-tested in isolation)

**Files:**
- Create: `crates/caliban-tools-builtin/src/fs/match_old.rs`
- Modify: `crates/caliban-tools-builtin/src/fs/mod.rs` (add `mod match_old;`)

**Interfaces:**
- Produces (consumed by Tasks 3 & 4):
  - A locate function that, given `text: &str`, `old: &str`, `new: &str`, `replace_all: bool`, returns an outcome enum:
    - `Located { ranges: Vec<Range<usize>>, replacement: String, tier: MatchTier }` — `ranges` are byte ranges in `text` to replace, `replacement` is the text to splice for each (verbatim `new` for Exact/trailing-ws; reindented `new` for the uniform indent-shift case), `tier ∈ {Exact, Whitespace}`.
    - `Ambiguous { count: usize, locations: Vec<(usize /*1-based line*/, usize)> }`.
    - `NotFound { near: Option<NearMiss> }`.
  - `NearMiss` with a `fn render(&self) -> String` producing a feedback string: the closest window's starting line number and a per-line `- expected` / `+ found` diff.
  - A small in-crate `levenshtein(a: &str, b: &str) -> usize` helper (private).

**Algorithm notes:**
- Tier 1 (Exact): `text.matches(old)` byte offsets. If any exist, return `Located{tier:Exact}` (verbatim replacement) — never consult fuzzy tiers.
- Tier 2 (Whitespace): operate on lines. Normalize each line via `trim_end()` + line-ending normalization. Slide a window of `old.lines().count()` over `text` lines; a window matches if, for every line, `file_line.trim_end()` equals `old_line.trim_end()` after removing a **single uniform leading-whitespace delta** common to all lines in the window. Compute the delta from the first non-blank line pair; verify it holds for all. Map matched windows back to byte ranges. If exactly one (or `replace_all`), reindent `new` by the same delta (add the prefix to / strip the prefix from each non-blank line of `new`) and return `Located{tier:Whitespace}`. If >1 and not `replace_all` → `Ambiguous`.
- Tier 3 (NotFound): pick the window minimizing `levenshtein(window_joined_trimmed, old_joined_trimmed)`; bound the file scan (e.g. skip if file > ~20k lines, return `near: None`) and truncate the rendered snippet (e.g. ≤ ~40 lines). Return `NotFound { near }`.

- [ ] **Step 1: Write failing unit tests** in `match_old.rs` covering the 9-point contract from the spec (exact unique/multiple/replace_all; trailing-ws; CRLF; uniform indent shift with reindent assertion; not-found returns rendered near-miss; tier-2 ambiguity; exact-wins-over-fuzzy). Assert reindent correctness by checking the resulting spliced text's indentation.

- [ ] **Step 2: Run tests, verify they fail to compile/assert** (module not implemented).
  Run: `cargo test -p caliban-tools-builtin match_old`

- [ ] **Step 3: Implement `match_old.rs`** per the algorithm notes; register `mod match_old;` in `fs/mod.rs`.

- [ ] **Step 4: Run tests, verify pass.**
  Run: `cargo test -p caliban-tools-builtin match_old`

- [ ] **Step 5: Commit.**
  ```bash
  git add crates/caliban-tools-builtin/src/fs/match_old.rs crates/caliban-tools-builtin/src/fs/mod.rs
  git commit -m "feat(tools): add tiered whitespace-tolerant matcher with near-miss feedback (#240)"
  ```

---

## Task 3: #240 — wire matcher into `Edit`

**Files:**
- Modify: `crates/caliban-tools-builtin/src/fs/edit.rs`
- Test: same file (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `match_old` locate function + `NearMiss::render` (Task 2).

- [ ] **Step 1: Write failing tests** in `edit.rs`: (a) `old_string` with extra trailing whitespace still applies and writes the file; (b) `old_string` uniformly under-indented applies and the written file has correct indentation; (c) a true miss returns an error whose message contains the near-miss diff (assert it is NOT the bare `old_string not found in file`); keep the existing exact/zero/multiple/replace_all tests passing.

- [ ] **Step 2: Run, verify new tests fail.**
  Run: `cargo test -p caliban-tools-builtin -- edit`

- [ ] **Step 3: Implement.** Replace the `text.matches(...)` counting + not-found/duplicate branch with a call to the `match_old` locate function. On `Located`, splice the replacement(s) and write atomically (preserve the existing `write_atomic` + `FileChanged` hook path). On `Ambiguous`, return the existing "matched N times" style error including the locations. On `NotFound`, return `ToolError::execution` whose message is `near.render()` when present, else fall back to the existing "old_string not found in file".

- [ ] **Step 4: Run, verify pass.**
  Run: `cargo test -p caliban-tools-builtin -- edit`

- [ ] **Step 5: Commit.**
  ```bash
  git add crates/caliban-tools-builtin/src/fs/edit.rs
  git commit -m "feat(tools): Edit uses whitespace-tolerant match + near-miss feedback (#240)"
  ```

---

## Task 4: #240 — wire matcher into `MultiEdit`

**Files:**
- Modify: `crates/caliban-tools-builtin/src/fs/multi_edit.rs`
- Test: same file (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `match_old` locate function + `NearMiss::render` (Task 2).

- [ ] **Step 1: Write failing tests** mirroring Task 3 for the sequential `apply_edits` path: a whitespace-only-different edit in a sequence applies; a missing edit rolls back the whole file and the error message (prefixed `edit #N:`) contains the near-miss diff. Keep existing happy-path/rollback/replace_all tests passing.

- [ ] **Step 2: Run, verify new tests fail.**
  Run: `cargo test -p caliban-tools-builtin -- multi_edit`

- [ ] **Step 3: Implement.** In `apply_edits`, replace the per-edit `current.matches(...)` counting with the `match_old` locate function applied to the running `current` text. Preserve sequential application, full rollback on any failure, and the `edit #N:` prefix on Ambiguous/NotFound messages (wrap `near.render()` with the prefix).

- [ ] **Step 4: Run, verify pass.**
  Run: `cargo test -p caliban-tools-builtin -- multi_edit`

- [ ] **Step 5: Commit.**
  ```bash
  git add crates/caliban-tools-builtin/src/fs/multi_edit.rs
  git commit -m "feat(tools): MultiEdit uses whitespace-tolerant match + near-miss feedback (#240)"
  ```

---

## Task 5: #239 — no-edit tracking + neutral nudge + RunOutcome telemetry

**Files:**
- Modify: `crates/caliban-agent-core/src/agent.rs` (`AgentConfig` + `Default`)
- Modify: `crates/caliban-agent-core/src/stream/mod.rs` (loop tracking, nudge injection, `RunOutcome`, `RunEnd` event)
- Test: `crates/caliban-agent-core/tests/` (new test file, e.g. `no_edit_nudge.rs`, using `MockProvider`)

**Interfaces:**
- Produces (consumed by Task 6): `RunOutcome.turns_without_edit: u32`, `RunOutcome.no_edit_nudge_emitted: bool`; the `TurnEvent::RunEnd` variant carries the same two fields.
- New config: `AgentConfig.no_edit_nudge_threshold: u32` (default `10`; `0` disables).

- [ ] **Step 1: Write failing integration tests** with `MockProvider` (`crates/caliban-agent-core/tests/no_edit_nudge.rs`):
  - A scripted run that only ever calls a read-only tool, with `no_edit_nudge_threshold = 3`, runs past 3 turns → exactly one synthetic user message containing "without editing any files" appears in `final_messages`, and `outcome.no_edit_nudge_emitted == true`.
  - A run that performs a successful `Edit`-class (non-read-only) tool call before the threshold → no nudge message, `no_edit_nudge_emitted == false`.
  - `no_edit_nudge_threshold = 0` → never nudges regardless of turns.

- [ ] **Step 2: Run, verify fail.**
  Run: `cargo test -p caliban-agent-core --features caliban-provider/mock no_edit`

- [ ] **Step 3: Implement.**
  - Add `no_edit_nudge_threshold: u32` to `AgentConfig` (doc comment; default `10` in `Default`); update the `recovery_config_tests` defaults test if it enumerates fields.
  - In the loop: add `turns_since_last_edit: u32`, `turns_without_edit: u32` (high-water), `no_edit_nudge_armed: bool` (starts `true`). When a tool call in the turn succeeds and that tool is not `is_read_only()`, reset `turns_since_last_edit = 0` and re-arm. After a turn completes with no successful edit, increment `turns_since_last_edit` and update the high-water `turns_without_edit`. When `no_edit_nudge_threshold > 0 && turns_since_last_edit >= threshold && no_edit_nudge_armed`, push `Message::user_text(<neutral nudge>)`, set `no_edit_nudge_emitted = true`, disarm, emit `tracing::info!(turns_since_last_edit, "no-edit nudge injected")`, and `break 'inner` to take another turn. Use the exact neutral text from the spec (parameterized by `turns_since_last_edit`).
  - Thread `turns_without_edit` + `no_edit_nudge_emitted` into `RunOutcome` and the `RunEnd` `TurnEvent`.

- [ ] **Step 4: Run, verify pass.** Also run the existing loop suites to catch regressions.
  Run: `cargo test -p caliban-agent-core --features caliban-provider/mock`

- [ ] **Step 5: Commit.**
  ```bash
  git add crates/caliban-agent-core/src/agent.rs crates/caliban-agent-core/src/stream/mod.rs crates/caliban-agent-core/tests/no_edit_nudge.rs
  git commit -m "feat(sub-agents): no-edit-progress nudge + turns_without_edit telemetry (#239)"
  ```

---

## Task 6: #239 — surface no-edit telemetry on the headless ResultFrame

**Files:**
- Modify: `caliban/src/headless/events.rs` (`ResultFrame` struct + constructor)
- Modify: `caliban/src/headless/mod.rs` (capture from `RunEnd`, populate the result frame)
- Test: `caliban/src/headless/` existing test module(s)

**Interfaces:**
- Consumes: `RunOutcome.turns_without_edit`, `RunOutcome.no_edit_nudge_emitted`, and the `RunEnd` fields from Task 5.

- [ ] **Step 1: Write a failing test** asserting the emitted `result` frame JSON includes `turns_without_edit` and `no_edit_nudge_emitted` (serde field names in snake_case, matching the existing `turns`/`total_input_tokens` convention).

- [ ] **Step 2: Run, verify fail.**
  Run: `cargo test -p caliban headless`

- [ ] **Step 3: Implement.** Add `turns_without_edit: u32` and `no_edit_nudge_emitted: bool` to `ResultFrame`; capture them in the `RunEnd` handler in `mod.rs` (alongside `final_messages`/`stopped_for`) and pass them through to the result-frame builder. Keep field ordering/serde attributes consistent with existing fields.

- [ ] **Step 4: Run, verify pass.**
  Run: `cargo test -p caliban headless`

- [ ] **Step 5: Commit.**
  ```bash
  git add caliban/src/headless/events.rs caliban/src/headless/mod.rs
  git commit -m "feat(observability): surface turns_without_edit/no_edit_nudge_emitted on result frame (#239)"
  ```

---

## Final gate (after all tasks)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```

Then dispatch the whole-branch review and surface the PR (closing #240, #241; #239 stays open for the deferred build-time-box / adaptive-verification half).
