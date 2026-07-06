# Tool Result Stream Payload — Design (#391)

**Goal:** Make each tool call's *result* visible on the agent event stream in a
bounded, display-ready form, correlatable to the call's start — so an observer
(the prospero dashboard tool inspector) can show **input + result**, not just
input + ok.

## Background / finding

The premise in #391 ("no result content anywhere in the stream") reflects
prospero's *normalization*, not the caliban wire. On the actual wire:

- `TurnEvent::ToolCallEnd` already carries `is_error` + `content: Vec<ContentBlock>`
  (the tool's returned body), correlatable to `ToolCallStart` via a shared
  `tool_use_id` (`crates/caliban-agent-core/src/stream/mod.rs`).
- The wire protocol *is* `TurnEvent` serialized as NDJSON: the worker writes the
  **full** event per line (`caliban/src/worker.rs`), the fan-out hub carries the
  full stream, and nothing slims it.

So the structured result is already present. What is missing is what the ticket
flags as "consider": a **bounded, display-ready** rendering. `content` is
`Vec<ContentBlock>` (may include images, may be unbounded), which forces every
observer to re-flatten and re-cap it. prospero's inspector wants a cheap,
capped text field it can render directly.

## Decision

Add two **purely additive** fields to `TurnEvent::ToolCallEnd`:

```rust
ToolCallEnd {
    turn_index: u32,
    tool_use_id: String,        // existing — correlates to ToolCallStart
    is_error: bool,             // existing
    content: Vec<ContentBlock>, // existing — full structured result, UNCHANGED
    result_text: String,        // NEW — flattened, capped, display-ready text
    truncated: bool,            // NEW — true iff result_text was capped
}
```

- **`result_text`**: the result's text blocks flattened (non-text blocks
  ignored, joined by `\n`), then capped to `STREAM_RESULT_TEXT_CAP` chars. It is
  the **head** of the flattened text — no injected marker; the consumer renders
  its own "truncated" affordance from the flag.
- **`truncated`**: `true` iff the flattened text exceeded the cap.
- **`STREAM_RESULT_TEXT_CAP = 8 * 1024`** (8192 chars). A *display* cap,
  deliberately smaller and separate from `post_process::ToolResultCap`
  (the model-context cap) — a dashboard preview wants a tighter bound than
  what is fed back to the model.

Both fields are computed at the two `ToolCallEnd` emission sites (normal result
and denied/error), where a `ToolResultBlock` (`.content: Vec<ContentBlock>`) is
already in hand.

### Deliberate choices (and why)

- **`content` stays full/structured.** Additive and non-breaking: existing
  consumers (headless error surfacing, TUI rendering, `agents attach`) keep
  working unchanged, and the full authoritative copy also lives in
  `TurnEnd.tool_results`. prospero reads the cheap bounded `result_text` +
  `truncated`.
- **Not shrinking the wire tonight.** Capping/dropping `content` on this frame
  to reduce fleet wire size is a *separate* future optimization (the full copy
  is retained in `TurnEnd.tool_results`); it would change existing-consumer
  behavior, so it is out of scope here. YAGNI until fleet wire size is shown to
  be a real problem.
- **Char-based cap** (`.chars().take(n)`), matching `ToolResultCap`'s
  `max_chars`, so truncation never splits a UTF-8 boundary.

### Edge cases

- **Non-text-only result** (e.g. an image-only result): `result_text` is
  empty, `truncated` is `false`. Acceptable — the structured `content` still
  carries the image for consumers that want it.
- **Error / denied result**: the error text is a `Text` block, so `result_text`
  carries the error message and the inspector can display it. `is_error` stays
  the authoritative success/fail signal.
- **Empty result**: `result_text` empty, `truncated` false.

## Files

- `crates/caliban-agent-core/src/stream/mod.rs` — add the two fields to
  `ToolCallEnd`; add `STREAM_RESULT_TEXT_CAP` const; extract a
  `flatten_content_text(&[ContentBlock]) -> String` helper (reused by the
  existing `tool_result_text`) plus a `capped_result_text(&[ContentBlock],
  usize) -> (String, bool)`; populate at both emission sites.
- Consumers destructuring `ToolCallEnd { .. }` need no change (rest pattern).
  Only the two constructor sites and any exhaustive destructure are touched.

## Testing

- New integration test (`crates/caliban-agent-core/tests/`): drive a run whose
  tool returns a small result → assert the `ToolCallEnd` for that `tool_use_id`
  has `truncated == false` and `result_text` equals the full flattened text.
- Drive a tool returning a >cap result → assert `truncated == true`,
  `result_text.chars().count() == STREAM_RESULT_TEXT_CAP`, and it is a prefix of
  the full text.
- Error path: a denied/errored tool → `result_text` carries the error text,
  `is_error == true`.

## Acceptance

A tool call's result content is available on the agent event stream as a
bounded, display-ready `result_text` (+ `truncated`), correlatable to its start
via `tool_use_id`, so prospero's inspector can display input + result. Unblocks
prospero #5 / #95.
