# Tool Result Stream Payload — Implementation Plan (#391)

> **For agentic workers:** TDD. Steps use checkbox syntax.

**Goal:** Add bounded, display-ready `result_text` + `truncated` to
`TurnEvent::ToolCallEnd`, correlatable via the existing `tool_use_id`.

**Architecture:** Two additive fields on an existing NDJSON-serialized enum
variant; populated at the two emission sites by flattening the in-hand
`ToolResultBlock.content` and capping it.

**Tech Stack:** Rust, `caliban-agent-core`, serde NDJSON, tokio.

## Global Constraints

- Purely additive to `ToolCallEnd`; `content` field unchanged.
- Cap: `STREAM_RESULT_TEXT_CAP = 8 * 1024` chars, char-based truncation.
- `result_text` is the flattened head (no injected marker); `truncated` flag
  signals capping.
- Full local gate before PR: `cargo fmt --all -- --check`, `cargo clippy
  --workspace --all-targets -- -D warnings`, `cargo build --workspace
  --all-targets`, `cargo test --workspace`.

---

### Task 1: Add fields + helpers, populate both emission sites, test

**Files:**
- Modify: `crates/caliban-agent-core/src/stream/mod.rs`
  - `ToolCallEnd` variant (~333): add `result_text: String`, `truncated: bool`.
  - Near `tool_result_text` (~171): add `STREAM_RESULT_TEXT_CAP` const,
    `flatten_content_text(&[ContentBlock]) -> String` (refactor
    `tool_result_text` to reuse it), and `capped_result_text(&[ContentBlock],
    usize) -> (String, bool)`.
  - Emission sites: normal (~1949) and denied/error (~1829) — compute
    `(result_text, truncated)` from the in-hand `.content` and include.
- Test: `crates/caliban-agent-core/tests/tool_result_payload.rs` (new).

**Interfaces:**
- Produces: `TurnEvent::ToolCallEnd { .., result_text: String, truncated: bool }`.
- `capped_result_text(content: &[ContentBlock], max_chars: usize) -> (String, bool)`.

- [ ] **Step 1: Write the failing test** — small result (full text, not
  truncated), large result (truncated, len == cap, prefix), error result
  (error text present, is_error true). Model the harness on the existing
  `tests/streaming.rs` scenario-11b tool-use test (a scripted provider stream
  emitting a tool-use block, an `EchoTool`-style tool). Assert on the
  `ToolCallEnd` matching the tool_use_id.
- [ ] **Step 2: Run it, watch it fail** — `cargo test -p caliban-agent-core
  --test tool_result_payload` → fails to compile (fields absent) then fails
  assertions.
- [ ] **Step 3: Implement** — add the two fields, the const, the two helpers,
  and populate both emission sites.
- [ ] **Step 4: Run it, watch it pass** — same command, green. Also run
  `tests/streaming.rs` + `tests/parallel_tools.rs` to confirm no regression in
  the other `ToolCallEnd` assertions.
- [ ] **Step 5: touch changed .rs files, then full gate** — `touch` the changed
  files (defeat clippy cache-miss), then fmt/clippy/build/test workspace-wide.
- [ ] **Step 6: Commit** — `feat(observability): emit bounded tool result_text
  on the agent stream (#391)`.

---

## Self-Review

- Spec coverage: fields, cap, flatten, both emission sites, edge cases (empty /
  image-only / error) — all covered by Task 1.
- No placeholders.
- Type consistency: `capped_result_text` signature stable; `result_text: String`
  / `truncated: bool` used identically at both sites and in the test.
