# Headless result-frame enrichment toward the Claude Code contract

**Date:** 2026-07-03
**Ticket:** [#222](https://github.com/caliban-ai/caliban/issues/222) (headless result frame diverges from ADR 0025 — aggregates all assistant text + key drift)
**Amends:** ADR [0025](../adr/0025-headless-output-protocol.md) (headless output protocol) — result-frame shape
**Status:** Approved (design)

## Problem

The terminal `result` frame in `--output-format {json,stream-json}` diverges from the Claude Code (CC)
headless contract in two ways:

1. **`result` aggregates all assistant text, not the final message.** `result` is populated from
   `final_text`, which byte-concatenates every `AssistantTextDelta` across the *whole* run
   (`caliban/src/headless/mod.rs:569-574`, buffer created at `:479`, never reset between turns). For any
   tool-using (multi-turn) run, `result` therefore starts with turn-1 narration and ends with the final
   summary. CC's `result` is the **final assistant message only**. The correct value is already computed
   (`self.last_assistant_text`, captured at `TurnEnd`, `mod.rs:651-654`) but **discarded for success**
   (`mod.rs:966-971`, `events.rs:489-507`).
2. **Key drift.** The frame emits `turns`, `total_input_tokens`, `total_output_tokens`; CC uses `num_turns`
   and a `usage{}` object, and additionally emits `is_error`, `duration_ms`, `duration_api_ms`.

Nuance discovered during design: **ADR 0025 does not claim byte-identical CC parity** — it explicitly states
"we do not commit to byte-identical compatibility because caliban is provider-agnostic" and names "add a
compat translator" as its *revisit* path. It also currently blesses the success-path concatenation. So this
work is "enrich toward CC without breaking existing consumers," not "fix a contract violation."

## Decision

Enrich the result frame **additively** (non-breaking) and fix the `result` value:

1. `result` (success) = the **final assistant message** (`self.last_assistant_text`), not the cross-turn
   concat.
2. Add **`is_error: bool`** = `subtype != "success"`.
3. Add **`duration_ms: u64`** = wall-clock elapsed from run entry to `emit_result`.
4. **Additively** add the CC key names alongside the existing ones: emit `num_turns` (= `turns`) and a
   `usage: { input_tokens, output_tokens }` object, while keeping `turns` / `total_input_tokens` /
   `total_output_tokens`. Existing caliban stream-json consumers are unaffected; CC drop-in consumers get
   their keys.
5. **Defer `duration_api_ms`** to a follow-up. Accurate API-only timing needs provider-layer instrumentation
   that does not exist; the headless driver only sees agent-core `TurnEvent`s, which interleave tool
   execution, so measuring there would conflate tool time. `duration_ms` delivers the headline duration now.
6. **Amend ADR 0025** to document the enriched shape and record that key *renaming* (the breaking path) is
   intentionally deferred to a future compat translator.

Non-goals: renaming/removing existing keys (breaking), `duration_api_ms` (follow-up), any change to the
non-success `result`/`last_assistant_text`/`tool_calls_seen` behavior (already correct per ADR 0025).

## Components

### 1. `ResultFrame` struct + builder (`caliban/src/headless/events.rs`)
- Add fields to `ResultFrame` (`:268-319`): `is_error: bool`, `num_turns: u32`, `duration_ms: u64`, and a
  `usage: UsageTotals` object (new small struct `{ input_tokens: u32, output_tokens: u32 }`). Keep the
  existing `turns` / `total_input_tokens` / `total_output_tokens` fields.
- `result_frame(...)` builder (`:489-507`): stop forcing `result = final_text` for success; take an explicit
  `final_message` argument (the final assistant text) and use it for the success `result`. Populate
  `is_error` from the subtype, `num_turns = turns`, `usage` from the token totals.

### 2. Driver wiring (`caliban/src/headless/mod.rs`)
- **Timing:** capture `let started = Instant::now();` at run entry (`HeadlessDriver::run` ~`:440` and the
  stream-json `run_frames` ~`:856`), store on the driver (or thread into `HeadlessRunSummary`, `:230-305`),
  and compute `duration_ms` at `emit_result` (`:960-988`).
- **`result` value:** in `emit_result`, pass `self.last_assistant_text` (the final message) as the success
  `result` source instead of `s.final_text`. For the stream-json per-`user`-frame path, `last_assistant_text`
  is already reset appropriately at `TurnEnd`; confirm it reflects the last turn of the current frame.
- The non-success path is unchanged.

### 3. Encoders (`caliban/src/headless/encoder.rs`)
- No structural change — `JsonEncoder::result` (`:369-381`) and `StreamJsonEncoder::result` (`:503-510`)
  serialize `ResultFrame` directly, so new serde fields flow through automatically.

### 4. ADR 0025 amendment (`docs/adr/`)
- New ADR (next number) amending 0025's result-frame section: records final-message `result`, `is_error`,
  `duration_ms`, additive `num_turns`/`usage`, and the deferral of key-renaming + `duration_api_ms`.
  Annotate 0025's status + index row (bidirectional, `0005 ← 0042` pattern).

### 5. Follow-up ticket
- File a ticket for `duration_api_ms` (needs provider-layer API timing) — noted in the PR, not implemented
  here.

## Data flow

Run entry stamps `started: Instant`. During the run, `last_assistant_text` tracks the current turn's
assistant text (reset at each `TurnEnd`); `final_text` still accumulates for the `text` deltas / text-format
output. At `emit_result`: `duration_ms = started.elapsed().as_millis()`; success `result =
last_assistant_text`; `is_error = subtype != success`; `num_turns = turns`; `usage = { total_input_tokens,
total_output_tokens }`. Serde emits all keys.

## Error handling

- Error/non-success subtypes: `is_error = true`; `result` stays absent (unchanged); `error` string and
  structured fields unchanged.
- `duration_ms` is always present (≥ 0), success or error.
- No new failure modes — all additions are derived from data already at the emission site (subtype, turns,
  token totals, a start `Instant`).

## Testing

- **Unit (`events.rs`):**
  - success `result` = the passed final message, not a concatenation;
  - `is_error` false for success, true for `error`/`max_turns`/`cancelled`/`budget_exceeded`/`max_tokens`;
  - `num_turns == turns`; `usage.input_tokens/output_tokens` match the flat totals; legacy keys still present.
- **Driver (`mod.rs`):**
  - multi-turn regression: a run with ≥2 assistant turns emits `result` = the **final** turn's text, not the
    concatenation. Update `success_result_frame_keeps_legacy_result_field` (`:2254`) which currently locks in
    the bug.
  - `duration_ms` present and ≥ 0.
- **Integration (`caliban/tests/headless.rs`):** assert the enriched key set (`is_error`, `num_turns`,
  `usage`, `duration_ms`) against a real emitted frame (`json_format_shape_includes_required_fields`,
  `:227-256`), not a hand-built literal.
- Full CI-mirror gate green; adr-validate at ship (ADR touched).

## Acceptance criteria (from the ticket)

- [x] `result` contains only the final assistant message (Component 1–2).
- [x] Result-frame keys move toward the ADR 0025 / CC contract — CC keys added additively (`num_turns`,
      `usage`, `is_error`, `duration_ms`); intentional non-rename recorded in the ADR amendment. `duration_api_ms`
      tracked as a follow-up.
- [x] A test pins the enriched result-frame shape (Testing).
