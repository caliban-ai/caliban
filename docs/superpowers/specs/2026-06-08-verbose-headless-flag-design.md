# Design: `--verbose` headless flag (full untruncated tool I/O)

**Date:** 2026-06-08
**Status:** Approved (design)
**Topic:** observability — headless tool-call visibility
**Tracks:** caliban-ai/caliban#27

## Goal

Headless mode has no `--verbose` flag to surface full, untruncated tool
inputs/outputs. Add one so operators running `caliban -p` in the default
**text** output format can see complete tool-call detail.

## Background — what each output mode shows today

- **`--output-format stream-json`** already emits full `tool_use` (untruncated
  `input` Value) and `tool_result` (full `content`) frames per call
  (`mod.rs:600-612`). Nothing to add — programmatic consumers already get
  complete detail.
- **`--output-format text`** (the default) emits **only** the final assistant
  answer to stdout. Tool calls are completely invisible — `ToolCallStart` /
  `InputDelta` / `ToolCallEnd` are no-ops except for the `tool_calls_seen`
  counter.
- **`--output-format json`** emits one aggregate result object; tool I/O is
  not itemized.
- The interactive single-prompt path (`startup::run_and_render`) prints tool
  I/O to stderr but **truncated to 80 chars** (`summarize(.., 80)`). That path
  is not headless and is out of scope.

So the gap the ticket names is specifically **text mode**, which shows nothing.

## Decisions

- **Flag:** `--verbose` (`clap` bool, `env = "CALIBAN_VERBOSE"`,
  `help_heading = "Headless / -p mode (ADR 0025)"`). Matches the existing
  headless flag family.

- **Scope: text mode → stderr.** When `--verbose` and
  `--output-format text`, print each completed tool call as a fully
  untruncated two-line block to **stderr**. stdout stays the assistant
  answer only, so `caliban -p ... > out.txt` / `$(caliban -p ...)` capture is
  unchanged. This mirrors the driver's existing stderr convention — the
  model-mismatch warning already uses `eprintln!` in text mode
  (`mod.rs:706-709`).

- **No-op elsewhere.** stream-json is already full (a `--verbose` there would
  be redundant); json keeps its single-object contract. Documented on the
  flag. This keeps the blast radius to the one mode that lacks visibility.

- **Block format** (per completed call, to stderr):

  ```
  🔧 <name>(<full compact input JSON>)
     → [(error) ]<full result body>
  ```

  - Input: the buffered tool-input JSON parsed via the existing
    `parse_tool_input`, serialized compact and **in full** (no 80-char cap).
  - Result body: if every result block is `Text`, the concatenated raw text
    (full); otherwise the full `content_blocks_to_json` serialization. Errors
    get an `(error) ` prefix. Mirrors the single-prompt path's `🔧 name(..)` /
    `   → ..` shape, minus the truncation.

- **Buffering:** text+verbose must accumulate the streamed input JSON the same
  way stream-json already does. Generalize the existing `OutputFormat::StreamJson`
  guard on `ToolCallStart` / `ToolCallInputDelta` / `ToolCallEnd` into a
  `buffers_tool_io()` predicate: `StreamJson || (Text && verbose)`.

## Non-goals

- No change to stream-json or json output. No un-truncating the interactive
  single-prompt path (separate concern; could be a follow-up).
- No new result-frame schema fields. No log file (that's `--debug-file`, #26).

## Implementation sketch

`args.rs` — new field in the headless group:

```rust
/// Dump full, untruncated tool inputs/outputs to stderr in headless
/// `--output-format text`. No effect on `stream-json` (already full) or
/// `json`. `CALIBAN_VERBOSE` is also honored.
#[arg(long, env = "CALIBAN_VERBOSE", help_heading = "Headless / -p mode (ADR 0025)")]
pub(crate) verbose: bool,
```

`startup.rs::run_headless` — thread into `HeadlessRunConfig { .. verbose: args.verbose }`.

`headless/mod.rs`:
- Add `verbose: bool` to `HeadlessRunConfig` (+ `minimal()` default `false`).
- Add `fn buffers_tool_io(&self) -> bool`.
- `ToolCallStart` / `ToolCallInputDelta`: gate on `buffers_tool_io()`.
- `ToolCallEnd`: keep the stream-json arm; add a `Text if verbose` arm that
  takes the buffered input, renders via the pure helpers, and `eprintln!`s it.
- New pure helpers `render_verbose_tool_io(name, input, is_error, content)` and
  `verbose_tool_result_body(content)`.

## Test plan (TDD)

Pure-function unit tests (the core behavior):

1. `render_verbose_tool_io` includes the tool name, the **full** input JSON,
   and the result; an `(error)` call gets the `(error) ` prefix.
2. **Untruncation guarantee:** a >200-char input and a >200-char result both
   appear in full in the rendered block (the key difference from the
   single-prompt path's 80-char cap).
3. `verbose_tool_result_body` returns full joined text for text blocks, and
   full JSON for structured/mixed content.

Predicate test:

4. `buffers_tool_io`: true for `Text+verbose` and `StreamJson`; false for
   `Text` without verbose and for `Json`.

stdout cleanliness:

5. Guaranteed by construction — the verbose arm only ever calls `eprintln!`
   (stderr) and never touches `self.writer` (stdout). The `buffers_tool_io`
   predicate test also pins that non-verbose text mode stays a no-op, so no
   behavior changes for existing text/json consumers. A full provider+tool
   integration test would add a heavyweight mock harness for no extra
   coverage of the new logic, so it's intentionally omitted.

Flag parsing (`args.rs`):

6. `--verbose` parses to `true`; absent → `false`.

CI gate: `cargo fmt --all -- --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo build --workspace --all-targets`,
`cargo test --workspace`.
