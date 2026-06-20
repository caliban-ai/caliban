# Eval Follow-ups Design (#239, #240, #241)

**Date:** 2026-06-20
**Status:** accepted (sprint)
**Source:** Findings from the SWE-bench Lite eval harness (`~/dev/caliban-ai/qa/evals`) driving the caliban release binary headless. Tickets `caliban-ai/caliban#239`, `#240`, `#241`.

## Goal

Address three independent harness weaknesses surfaced by dogfooding caliban as a SWE-bench implementer:

- **#241** — a transient provider transport error (HTTP 500 wrapping an Ollama EOF) aborted a whole run with no retry.
- **#240** — `Edit`/`MultiEdit` require an exact substring match and give a bare "not found" on a miss, disproportionately penalizing weaker/local models (qwen 56% edit-failure vs sonnet 5%) — anti-provider-agnostic.
- **#239** — on build-heavy repos the agent burns the entire turn budget on build setup and never edits (empty patch). Ship the *detectable, low-risk* increment now; defer the build-time-box / adaptive-verification tuning (needs eval iteration).

These are independent subsystems (providers, tools, agent-loop) handled as one sprint, three commits.

## Non-goals

- Baking the eval's "do not build/run tests" prompt into caliban — the ticket (#239) explicitly warns this harms real users and games the benchmark. The in-loop nudge stays **neutral**.
- The build-setup time-box and adaptive verify/skip calibration (the other half of #239) — deferred; requires re-running the eval to tune. #239 stays open for it.
- Any CI/eval wiring — the eval remains local-only.

---

## #241 — Retry transient transport errors

**Current state.** `caliban-agent-core/src/retry.rs` already defines `RetryPolicy` (default: 3 attempts, 500ms initial backoff, ×2, 30s cap, jitter) and `with_retry`, and the streaming loop already wraps the provider call in it (`stream/mod.rs:806`). `is_retryable` returns true for `RateLimit`, `Network`, `StreamInterrupted`, and `ServerError { status: 502..=599 }`.

**Gaps.**
1. `ServerError { status: 500 }` is **not** retryable, but the #241 failure was exactly an HTTP 500 (`server error (HTTP 500): {"error":"Post ... EOF"}`) — Ollama wrapped an upstream EOF in a 500. A single blip killed the instance at turn 0.
2. `with_retry` is **silent** — there is no per-attempt log, so retries are not observable (ticket AC requires this).

**Design.**
- Widen the retryable `ServerError` range from `502..=599` to **`500..=599`** ("transient 5xx", per the ticket). 501 is technically non-transient but is vanishingly rare from LLM endpoints and a bounded 3-attempt retry on it is harmless; favor the simple, ticket-aligned range.
- Add a `tracing::warn!` inside `with_retry`'s retry branch, emitted once per retry attempt, naming the error class, the attempt number, and the backoff duration. Debounced by construction (one per attempt; max 2 retries by default).
- Non-transient errors (`Auth`, `InvalidRequest`, `ModelUnavailable`/404, `ContentFilter`) remain non-retryable → fail-fast preserved.

**Acceptance.**
- A transient 500/EOF/connection-reset/timeout is retried with backoff; a terminal error is surfaced only after attempts are exhausted.
- Each retry emits an observable warning.
- 4xx/auth still fail fast with no retry.

---

## #240 — Whitespace-tolerant Edit matching + near-miss feedback

**Current state.** `Edit` (`fs/edit.rs`) and `MultiEdit` (`fs/multi_edit.rs`) both locate `old_string` via `str::matches` (exact substring), require exactly one occurrence unless `replace_all`, and on a miss return `old_string not found in file` / `edit #N: old_string not found in current contents (rolling back)`. No whitespace tolerance, no near-miss feedback. Errors reach the model as `Error: execution failed: <message>` with `is_error: true`.

**Design — shared matcher `crates/caliban-tools-builtin/src/fs/match_old.rs`.** A tiered locator used by both tools so the behavior (and its tests) live in one place (DRY):

1. **Tier 1 — Exact.** Count exact `old_string` occurrences. This tier **always takes precedence**: if any exact match exists, fuzzy tiers are never consulted. Uniqueness / `replace_all` semantics are unchanged.

2. **Tier 2 — Whitespace-tolerant** (only when exact count == 0). Locate `old_string` allowing:
   - line-ending normalization (CRLF/CR → LF),
   - per-line trailing-whitespace differences,
   - a **uniform** leading-indent shift: every line of the matched window differs from the corresponding `old_string` line by the *same* leading-whitespace prefix. When matched, `new_string` is **reindented by that same delta** before insertion so the replacement lands at the file's actual indentation. (Non-uniform indentation differences are *not* guessed — they fall through to Tier 3.)

   Tier 2 must yield exactly one window (unless `replace_all`); multiple windows → ambiguous error that lists the candidate line ranges.

3. **Tier 3 — Near-miss feedback** (when Tier 1 and Tier 2 both find nothing). Find the file window most similar to `old_string` (slide a window of `old_string`'s line count over the file; rank by normalized Levenshtein distance over the trimmed, line-joined text; bound the scan and the reported snippet). Return an error that names the closest location and shows a per-line diff (`- expected` / `+ found`, with line numbers) instead of the bare "not found". Implemented self-contained (small Levenshtein), no new dependency (`strsim`/`similar` are absent; `nucleo-matcher` is shaped for interactive ranking, not edit-distance).

**Matcher API (illustrative — implementer may refine types, tests pin behavior):**
- `find_unique(text, old, new) -> Located` where `Located` carries the byte range to replace and the replacement string to splice (verbatim for Tier 1 / trailing-ws; reindented for the indent-shift case), or an outcome of `Ambiguous { ranges }` / `NotFound { near: Option<NearMiss> }`.
- `NearMiss` renders to a feedback string with line numbers + per-line diff.

**Behavioral contract (tests):**
1. Exact unique → replaces, success.
2. Exact multiple + `replace_all=false` → existing "matched N times" error (unchanged).
3. Exact multiple + `replace_all=true` → replaces all (unchanged).
4. `old_string` with trailing whitespace the file lacks → unique Tier-2 match, applies.
5. CRLF file vs LF `old_string` → matches.
6. `old_string` uniformly under-indented (e.g. 4 spaces less) → matches; `new_string` is reindented (+4) so the written file's indentation is correct.
7. No match anywhere → error contains the closest window snippet with line numbers and a `-`/`+` diff (assert the message is *not* the bare "not found").
8. Tier-2 ambiguity (two normalized windows, no `replace_all`) → ambiguous error listing locations.
9. Exact match exists *and* a fuzzy window also exists → exact wins (fuzzy never consulted).

Both `Edit` and `MultiEdit` route through the shared matcher; `MultiEdit` keeps its sequential apply + full-file rollback on any failed edit, and its per-edit `edit #N:` message prefix on near-miss/ambiguous feedback.

**Acceptance.**
- An `old_string` differing only in leading/trailing whitespace still applies.
- A failed Edit returns the closest near-match + diff, not a bare "not found".
- (Validation, post-merge) re-running the eval shows a materially lower ollama edit-failure rate.

---

## #239 — No-edit-progress signal + neutral nudge (bounded increment)

**Current state.** The turn loop (`stream/mod.rs`, `'outer: for turn_index in 0..max_turns`) tracks `turns_completed` and `total_usage` into `RunOutcome { final_messages, turn_count, total_usage, stopped_for }`. Tools expose `is_read_only()` (PR #162); file-mutating tools (`Write`/`Edit`/`MultiEdit`/`NotebookEdit`) return `false`. Synthetic messages are already injected mid-loop in other recovery paths via `history.push(Message::user_text(...))` + `break 'inner`.

**Design.**
- Track `turns_since_last_edit: u32` in the loop, incremented per completed turn, **reset to 0 whenever a non-`is_read_only` tool call succeeds** in a turn. Also track a cumulative `turns_without_edit` high-water value and a `no_edit_nudge_emitted: bool` for telemetry.
- New `AgentConfig.no_edit_nudge_threshold: u32` (default **10**; `0` disables). When `turns_since_last_edit` reaches the threshold and the nudge has not already fired for the current no-edit streak, inject **one** synthetic user message and re-arm only after a subsequent successful edit (debounced — at most one nudge per streak).
- **Nudge text (neutral):**
  > "You have taken {N} turns without editing any files. If you have already identified the change you need to make, make the edit now rather than continuing to investigate. If you are still investigating, you can disregard this note."

  It must not instruct the model to skip builds, tests, or verification (that is the eval-specific behavior #239 warns against).
- Emit a `tracing` event when the nudge fires (observable).
- Add `turns_without_edit: u32` and `no_edit_nudge_emitted: bool` to `RunOutcome`, thread through the `RunEnd` `TurnEvent`, and surface as `turns_without_edit` / `no_edit_nudge_emitted` on the headless `ResultFrame` JSON.

**Interaction with read-only sessions.** A legitimately read-only/research session will see at most one gentle, ignorable nudge at the threshold — acceptable, and disable-able via `no_edit_nudge_threshold = 0`.

**Acceptance.**
- The agent gets an in-loop no-edit signal before exhausting the turn budget (nudge + `tracing` + `RunOutcome`/`ResultFrame` fields).
- The nudge is neutral and fires at most once per no-edit streak.
- Default behavior on edit-producing runs is unchanged (no nudge when edits happen before the threshold).

**Deferred (remains open on #239):** turn-budget time-box on build/install setup, and adaptive verify-vs-skip calibration — to be designed and tuned against a fresh eval run, since a static rule risks the two-sided regression the v1/v2 A/B already demonstrated.

---

## Testing strategy

- **#241:** unit tests on `is_retryable` (500 retryable, 501/4xx classification) and a `with_retry` behavior test (retries a 500 then succeeds; 400 fails fast immediately) in `caliban-agent-core/tests/retry_backoff.rs`.
- **#240:** unit tests on the `match_old` matcher (the 9-point contract above) + tool-level tests in `edit.rs`/`multi_edit.rs` asserting whitespace matches apply and misses return near-miss feedback.
- **#239:** `caliban-agent-core` integration tests with `MockProvider`: (a) a run that never edits crosses the threshold → exactly one nudge message appears in history and `no_edit_nudge_emitted` is true; (b) a run that edits before the threshold → no nudge, `turns_without_edit` reflects the streak; (c) `no_edit_nudge_threshold = 0` → never nudges.

All work passes the full local gate before any push: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, `cargo test --workspace`.
