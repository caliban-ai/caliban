# Headless result-frame enrichment — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the headless `result` frame to carry the final assistant message (not a cross-turn concat) and enrich it additively toward the Claude Code contract (`is_error`, `num_turns`, `usage{}`, `duration_ms`), closing #222.

**Architecture:** `ResultFrame` (serde struct in `caliban/src/headless/events.rs`) serializes directly, so new serde fields flow to both `json` and `stream-json` output automatically. The `result`-value fix lives in `emit_result` (feed the already-tracked final-turn text for success). Timing is a `started: Instant` on the driver, reset per input frame. All additions are derived from data already at the emission site.

**Tech Stack:** Rust, serde, the `caliban` binary crate (`caliban/src/headless/{events.rs,mod.rs}`).

## Global Constraints

- Local CI-mirror gate before push: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, `cargo test --workspace`. All pass.
- Commit author identity for `~/dev/caliban-ai/**`: `john.ford2002@gmail.com`; end commits with `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Additive only** — do NOT rename/remove `turns`, `total_input_tokens`, `total_output_tokens` (existing consumers depend on them). Add `num_turns`, `usage{}`, `is_error`, `duration_ms` alongside.
- `duration_api_ms` is out of scope (follow-up ticket).

---

### Task 1: Enrich `ResultFrame` + builder (`events.rs`)

**Files:**
- Modify: `caliban/src/headless/events.rs` — `ResultFrame` struct (`:268-319`), a new `UsageTotals` struct, `result_frame` builder (`:472-520`), unit tests (`mod tests`, `:746+`).

**Interfaces:**
- Produces: `ResultFrame` gains `is_error: bool`, `num_turns: u32`, `usage: UsageTotals`, `duration_ms: u64`. `result_frame(...)` gains a trailing `duration_ms: u64` parameter.

- [ ] **Step 1: Write the failing tests** in the `tests` module of `caliban/src/headless/events.rs`:

```rust
#[test]
fn result_frame_adds_cc_contract_fields() {
    let f = result_frame(
        ResultSubtype::Success,
        "final answer",
        "sess-1",
        0.0,
        3,      // turns
        100,    // total_input_tokens
        42,     // total_output_tokens
        None,
        None,
        None,
        0,
        0,
        false,
        1234,   // duration_ms
    );
    let v = serde_json::to_value(&f).unwrap();
    // result = the passed final message.
    assert_eq!(v["result"], "final answer");
    // Additive CC keys.
    assert_eq!(v["is_error"], false);
    assert_eq!(v["num_turns"], 3);
    assert_eq!(v["usage"]["input_tokens"], 100);
    assert_eq!(v["usage"]["output_tokens"], 42);
    assert_eq!(v["duration_ms"], 1234);
    // Legacy keys still present (non-breaking).
    assert_eq!(v["turns"], 3);
    assert_eq!(v["total_input_tokens"], 100);
    assert_eq!(v["total_output_tokens"], 42);
}

#[test]
fn result_frame_is_error_true_for_non_success() {
    for st in [
        ResultSubtype::Error,
        ResultSubtype::MaxTurns,
        ResultSubtype::Cancelled,
        ResultSubtype::BudgetExceeded,
        ResultSubtype::MaxTokens,
    ] {
        let f = result_frame(
            st, "", "s", 0.0, 1, 0, 0, None, None, None, 0, 0, false, 0,
        );
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["is_error"], true, "subtype {:?} must be is_error=true", st);
    }
}
```

(Existing builder tests — `result_frame_success_carries_result_field` etc. — must be updated to pass the new trailing `duration_ms` arg, e.g. append `, 0` before the closing paren. Do this in Step 3.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban --bin caliban headless::events`
Expected: FAIL — `result_frame` arity mismatch / missing `is_error`/`num_turns`/`usage`/`duration_ms`.

- [ ] **Step 3: Implement.** In `caliban/src/headless/events.rs`:

Add the usage struct above `ResultFrame`:
```rust
/// Claude-Code-style token usage object, emitted alongside the flat
/// `total_input_tokens` / `total_output_tokens` for drop-in CC compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UsageTotals {
    pub(crate) input_tokens: u32,
    pub(crate) output_tokens: u32,
}
```

Add fields to `ResultFrame` (after `total_output_tokens`, `:292`):
```rust
    /// Claude-Code-contract alias for `turns` (additive; #222).
    pub(crate) num_turns: u32,
    /// `true` for any non-`success` subtype (additive; #222). Lets consumers
    /// branch without enumerating every subtype spelling.
    pub(crate) is_error: bool,
    /// Wall-clock run duration in milliseconds (#222).
    pub(crate) duration_ms: u64,
    /// Claude-Code-style usage object mirroring the flat token totals (#222).
    pub(crate) usage: UsageTotals,
```

Add the `duration_ms` parameter to `result_frame` (end of the arg list, after `no_edit_nudge_emitted: bool`):
```rust
    no_edit_nudge_emitted: bool,
    duration_ms: u64,
) -> ResultFrame {
```

In the `ResultFrame { ... }` literal (`:508+`), set the new fields (compute from existing args):
```rust
    ResultFrame {
        kind: "result".into(),
        subtype: subtype.as_str().into(),
        result,
        session_id: session_id.into(),
        total_cost_usd,
        turns,
        total_input_tokens,
        total_output_tokens,
        num_turns: turns,
        is_error: !is_success,
        duration_ms,
        usage: UsageTotals {
            input_tokens: total_input_tokens,
            output_tokens: total_output_tokens,
        },
        structured_output,
        error,
        last_assistant_text,
        // ... (tool_calls_seen, turns_without_edit, no_edit_nudge_emitted unchanged)
```

Update the existing builder tests to pass a trailing `duration_ms` arg (append `, 0`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban --bin caliban headless::events`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/headless/events.rs
git commit -m "feat(observability): add is_error/num_turns/usage/duration_ms to result frame (#222)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: `result` = final message + timing wiring (`mod.rs`)

**Files:**
- Modify: `caliban/src/headless/mod.rs` — `HeadlessDriver` struct (`:278-311`) + `new` (`:337-350`), the per-input-frame reset in `run_frames` (near `:908`), `emit_result` (`:960-988`), driver tests (`mod tests`).

**Interfaces:**
- Consumes: `result_frame(..., duration_ms)` from Task 1.

- [ ] **Step 1: Write the failing test** in `caliban/src/headless/mod.rs` `mod tests` (mirror the existing `success_result_frame_keeps_legacy_result_field` harness at `:2254`, but with a **two-turn** run):

```rust
/// #222: for a multi-turn (tool-using) run, `result` must be the FINAL
/// assistant message, not the cross-turn concatenation of every turn's text.
#[tokio::test]
async fn success_result_is_final_message_not_concat() {
    // Turn 1 emits "investigating" then a tool call; turn 2 emits "final answer"
    // and ends. `result` must be "final answer", not "investigatingfinal answer".
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(two_turn_text_then_final_stream());
    let agent = build_agent_with(Arc::clone(&mock), None, None, 10);

    let mut buf: Vec<u8> = Vec::new();
    let mut driver =
        HeadlessDriver::new(&mut buf, HeadlessRunConfig::minimal(OutputFormat::Json));
    let summary = driver
        .run(agent, vec![Message::user_text("go")], CancellationToken::new())
        .await
        .expect("run succeeds");
    assert_eq!(summary.subtype, ResultSubtype::Success);

    let frame = parse_json_frame(&buf);
    assert_eq!(frame["subtype"], "success");
    assert_eq!(
        frame["result"], "final answer",
        "result must be the final message, not the concat; got {frame}",
    );
    assert!(frame["duration_ms"].is_u64(), "duration_ms present; got {frame}");
    assert_eq!(frame["is_error"], false);
}
```

Note: build `two_turn_text_then_final_stream()` as a test helper near the other stream builders. If the existing `MockProvider` test helpers don't already provide a two-assistant-turn-with-tool script, add a minimal one that: turn 1 → assistant text "investigating" + a `tool_use` that resolves, turn 2 → assistant text "final answer" + `EndTurn`. Reuse the patterns in the neighboring driver tests (search the `mod tests` for existing multi-turn stream builders like `benign_text_stream` and tool-loop fixtures before writing a new one).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban --bin caliban success_result_is_final_message_not_concat`
Expected: FAIL — `result` is currently `"investigatingfinal answer"` (concat) and/or `duration_ms` missing.

- [ ] **Step 3: Add timing to the driver.** In `caliban/src/headless/mod.rs`:

Add the field to `HeadlessDriver` (after `no_edit_nudge_emitted: bool`, `:310`):
```rust
    /// Wall-clock start of the current run (or input frame in run_frames),
    /// used to compute the result frame's `duration_ms` (#222).
    started: std::time::Instant,
```
Init it in `new` (`:339-349`, add to the `Self { ... }`):
```rust
            started: std::time::Instant::now(),
```

- [ ] **Step 4: Reset the clock per input frame** in `run_frames`, next to `final_text.clear()` (`:908`):
```rust
                    final_text.clear();
                    self.started = std::time::Instant::now();
```
(Confirm the exact surrounding lines when editing; the reset must sit where each stdin `user` frame begins a fresh run, alongside the existing `final_text.clear()`.)

- [ ] **Step 5: Fix `result` value + pass `duration_ms`** in `emit_result` (`:960-988`):
```rust
    fn emit_result(&mut self, s: &HeadlessRunSummary) -> Result<(), HeadlessError> {
        self.flush_hook_events()?;
        let is_success = matches!(s.subtype, ResultSubtype::Success);
        // #222: success `result` = the final assistant message (last turn),
        // not the cross-turn concatenation carried by `final_text`. Fall back
        // to `final_text` when the per-turn tracker is empty.
        let result_source: &str = if is_success && !self.last_assistant_text.is_empty() {
            &self.last_assistant_text
        } else {
            &s.final_text
        };
        let last_assistant_text_override = if is_success || self.last_assistant_text.is_empty() {
            None
        } else {
            Some(self.last_assistant_text.clone())
        };
        let duration_ms =
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let frame = events::result_frame(
            s.subtype,
            result_source,
            &self.config.session_id,
            s.total_cost_usd,
            s.turns,
            s.total_input_tokens,
            s.total_output_tokens,
            s.structured_output.clone(),
            s.error.clone(),
            last_assistant_text_override,
            s.tool_calls_seen,
            s.turns_without_edit,
            s.no_edit_nudge_emitted,
            duration_ms,
        );
        self.encoder.result(&mut self.writer, &frame, s)
    }
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p caliban --bin caliban headless::`
Expected: PASS — the new multi-turn test passes; `success_result_frame_keeps_legacy_result_field` (`:2254`) still passes (single-turn: final message == full text, so `result == "ok"` holds; new additive fields don't break its assertions).

- [ ] **Step 7: Commit**

```bash
git add caliban/src/headless/mod.rs
git commit -m "fix(observability): result frame = final assistant message + duration_ms (#222)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Integration test for the enriched shape (`tests/headless.rs`)

**Files:**
- Modify: `caliban/tests/headless.rs` — extend `json_format_shape_includes_required_fields` (`:227-256`) or add a sibling test that asserts the enriched keys against a **real emitted frame**.

- [ ] **Step 1: Read** `json_format_shape_includes_required_fields` (`caliban/tests/headless.rs:227-256`) to see whether it runs a real driver or builds a `json!` literal. The map notes it uses a hand-built literal; if so, add a NEW test that drives a real headless run and asserts the emitted frame.

- [ ] **Step 2: Write the test** (adjust the run harness to match the file's existing helpers for launching a headless run):
```rust
#[tokio::test]
async fn result_frame_carries_cc_contract_keys() {
    // Drive a real headless success run and assert the enriched key set.
    // (Use this file's existing headless-run helper — see the neighboring
    // tests for how they spawn a run and capture the result frame.)
    let frame = run_headless_and_capture_result_frame().await; // existing/neighboring helper
    assert_eq!(frame["type"], "result");
    assert_eq!(frame["subtype"], "success");
    assert_eq!(frame["is_error"], false);
    assert!(frame["num_turns"].is_u64());
    assert!(frame["usage"]["input_tokens"].is_u64());
    assert!(frame["usage"]["output_tokens"].is_u64());
    assert!(frame["duration_ms"].is_u64());
    // Legacy keys still present.
    assert!(frame["turns"].is_u64());
    assert!(frame["total_input_tokens"].is_u64());
}
```
If no reusable "run and capture result frame" helper exists in `tests/headless.rs`, model the run on the closest existing integration test in that file (they already spawn headless runs) and extract the last NDJSON/JSON line as the result frame.

- [ ] **Step 3: Run**

Run: `cargo test -p caliban --test headless result_frame_carries_cc_contract_keys`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add caliban/tests/headless.rs
git commit -m "test(observability): pin enriched result-frame shape end-to-end (#222)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: ADR 0049 (amends 0025) + follow-up ticket

**Files:**
- Create: `docs/adr/0049-result-frame-cc-enrichment.md`.
- Modify: `docs/adr/0025-headless-output-protocol.md` (status annotation), `docs/adr/README.md` (0025 row annotation + new 0049 row).

- [ ] **Step 1: Author ADR 0049** following the repo's ADR format (see `docs/adr/0048-workspace-default-restricted.md` as a recent amendment example). Content:
  - Title: "Result-frame enrichment toward the Claude Code contract".
  - Status: `accepted`; Source: link the spec `../superpowers/specs/2026-07-03-result-frame-enrichment-design.md`.
  - Context: #222 — `result` carried a cross-turn concat for success; keys drifted from CC (`turns`/flat tokens); no `is_error`/durations. ADR 0025 hedged on CC parity.
  - Decision: `result` (success) = final assistant message; add `is_error`, `duration_ms`, and additive `num_turns` + `usage{}` (keep legacy keys — non-breaking); defer key *renaming* to a compat translator (0025's own revisit path) and `duration_api_ms` to a follow-up.
  - Consequences: CC drop-in without breaking existing consumers; slightly redundant frame; `duration_api_ms` needs provider instrumentation (follow-up).
  - State that it **amends ADR 0025** (result-frame section).

- [ ] **Step 2: Annotate 0025.** Set its Status line to `accepted (result-frame shape amended by [0049](0049-result-frame-cc-enrichment.md))` and update its `README.md` index row the same way. Add the new `0049` row to the index (after `0048`).

- [ ] **Step 3: File the `duration_api_ms` follow-up ticket** (records the deferred field):
```bash
gh issue create --repo caliban-ai/caliban \
  --title "feat(observability): result-frame duration_api_ms (provider API timing)" \
  --label "caliban,area/observability,kind/feature,priority/backlog" \
  --body "Follow-up to #222. The result frame now carries wall-clock duration_ms. Claude Code also emits duration_api_ms (time spent in provider API calls). Adding it accurately needs provider-layer API-time instrumentation (the headless driver only sees agent-core TurnEvents, which interleave tool execution). Thread per-request API duration from the provider/transport layer and accumulate it into the result frame. Ref ADR 0049."
```
(Then add the created issue to the board if the org workflow doesn't auto-add it.)

- [ ] **Step 4: Commit**

```bash
git add docs/adr
git commit -m "docs(observability): ADR 0049 (amends 0025) for enriched result frame (#222)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Full gate

- [ ] **Step 1: Run the CI-mirror gate**

Run:
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```
Expected: all pass (`cargo fmt --all` first if the check complains).

- [ ] **Step 2: Straggler grep** — any other `result_frame(` call site needing the new `duration_ms` arg, and any consumer reading `["turns"]` that should also accept `num_turns`:

Run: `rg 'result_frame\(' caliban/src --type rust`
The only production call site is `emit_result`; all others are tests (updated in Task 1/2).

- [ ] **Step 3: Handoff to cai-ship-it** (Ship step). Diff touches `docs/adr/`, so cai-ship-it runs the adr-validate gate.

## Self-Review

- **Spec coverage:** result=final-message → Task 2; is_error/num_turns/usage/duration_ms → Task 1 (+ wiring Task 2); additive-not-breaking → Task 1 (legacy keys retained) + Task 5 straggler check; ADR amendment → Task 4; duration_api_ms deferral → Task 4 Step 3 (follow-up ticket); tests → Tasks 1–3. All mapped.
- **Placeholder scan:** the ADR number (0049) and the follow-up issue body are concrete; the only "match the existing helper" notes (Task 2 stream builder, Task 3 run harness) point at named neighboring tests to model on, with the exact assertions given. No TBD.
- **Type consistency:** `result_frame(..., duration_ms: u64)` — the trailing param is used identically in the builder (Task 1) and the `emit_result` call (Task 2). `UsageTotals { input_tokens, output_tokens }` field names match the test assertions. `started: std::time::Instant` field name matches its use in `emit_result`.
