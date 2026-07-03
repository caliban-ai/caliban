# ADR 0049 · Result-frame enrichment toward the Claude Code contract

- **Status:** accepted
- **Date:** 2026-07-03
- **Source:** [`docs/superpowers/specs/2026-07-03-result-frame-enrichment-design.md`](../superpowers/specs/2026-07-03-result-frame-enrichment-design.md)

## Context

[ADR 0025](0025-headless-output-protocol.md) defined the headless output protocol, wrapping "closely around
Claude Code's documented shape" while explicitly declining "byte-identical compatibility because caliban is
provider-agnostic" (and naming "add a compat translator" as its revisit path). Two gaps surfaced in practice
([#222](https://github.com/caliban-ai/caliban/issues/222), QA dogfooding):

1. **`result` aggregated all assistant text.** For success, `result` was populated from `final_text`, which
   byte-concatenates every assistant text delta across the *whole* run. Any tool-using (multi-turn) run
   therefore put turn-1 narration at the front of `result` instead of the final answer. Claude Code's
   `result` is the final assistant message only. (0025's own text called `result` "the assistant's reply",
   singular, but did not forbid the concat.)
2. **Key drift.** The frame emitted `turns` and flat `total_input_tokens`/`total_output_tokens`; Claude Code
   uses `num_turns` and a `usage{}` object, and additionally emits `is_error`, `duration_ms`,
   `duration_api_ms`. Renaming caliban's keys would break existing stream-json consumers.

## Decision

We will **enrich the result frame additively and fix the `result` value**, amending ADR 0025's result-frame
shape:

- `result` (success) = the **final assistant message** (`HeadlessDriver.last_assistant_text`, the last turn's
  reply), falling back to the accumulated text only when the per-turn tracker is empty.
- Add **`is_error: bool`** (`true` for any non-`success` subtype).
- Add **`duration_ms: u64`** — wall-clock run duration (per input frame in the stream-json multi-frame path).
- Add the Claude-Code key names **additively**: emit `num_turns` (= `turns`) and a
  `usage: { input_tokens, output_tokens }` object **alongside** the existing `turns` /
  `total_input_tokens` / `total_output_tokens`. Existing consumers are unaffected; CC drop-in consumers get
  their keys.

We will **not** rename or remove the legacy keys (that breaking step stays deferred to 0025's future compat
translator), and we **defer `duration_api_ms`** to a follow-up: accurate provider-API-only timing needs
instrumentation the headless driver doesn't have (it sees agent-core `TurnEvent`s, which interleave tool
execution).

## Consequences

- **Positive:** `result` is now the answer, not a running monologue — the headline bug is fixed. Stream-json
  is a cleaner Claude-Code drop-in (`num_turns`, `usage`, `is_error`, `duration_ms` all present) without
  breaking any existing caliban consumer.
- **Negative:** The frame carries some redundancy (`turns` + `num_turns`, flat tokens + `usage`). A full CC
  key-rename is still outstanding, gated behind the 0025 compat-translator decision. `duration_api_ms` is not
  yet emitted.
- **Revisit if:** downstream consumers demand byte-for-byte CC parity (then do the breaking rename via the
  0025 compat translator), or the deferred `duration_api_ms` follow-up lands provider-level API timing.
