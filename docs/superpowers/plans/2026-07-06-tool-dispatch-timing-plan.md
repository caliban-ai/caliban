# Stream-json Tool-Dispatch Timing — Implementation Plan (#28)

> **For agentic workers:** TDD. Steps use checkbox syntax.

**Goal:** Opt-in `t_ms` dispatch-duration on the stream-json `tool_result` frame,
via `--include-tool-dispatch-events`.

**Architecture:** Measure dispatch in the headless driver
(`ToolCallStart`→`ToolCallEnd`); thread `Option<u64>` into the encoder; additive
`t_ms` on the `ToolResult` frame. agent-core untouched.

## Global Constraints

- Default off → byte-identical output.
- `t_ms` is a dispatch **duration** (ms), on `tool_result` only.
- Full local gate: fmt / clippy `-D warnings` / build / test, workspace.

---

### Task 1: t_ms on tool_result, gated by --include-tool-dispatch-events

**Files & steps:**
- `caliban/src/headless/events.rs`: add `t_ms: Option<u64>`
  (`#[serde(skip_serializing_if = "Option::is_none")]`) to `ToolResult`; add
  `t_ms` param to `tool_result()`.
- `caliban/src/headless/encoder.rs`: add `dispatch_ms: Option<u64>` to
  `FrameEncoder::tool_call` (trait + 4 impls; `StreamJsonEncoder` passes it to
  `tool_result`, others `_dispatch_ms`).
- `caliban/src/headless/mod.rs`: `HeadlessRunConfig.include_tool_dispatch_events:
  bool` (+ default false); `HeadlessDriver.tool_dispatch_starts: HashMap<String,
  Instant>` (+ init); at `ToolCallStart` insert when flag on; at `ToolCallEnd`
  compute `dispatch_ms` and pass to `encoder.tool_call`.
- `caliban/src/args.rs`: `--include-tool-dispatch-events` clap arg (mirror
  `include_hook_events`).
- `caliban/src/startup/drivers.rs`: wire the arg into `HeadlessRunConfig`.

- [ ] **Step 1: Write failing tests** — (a) serialization: `tool_result(id,
  false, content, Some(7))` → `json["t_ms"] == 7`; `None` → no `t_ms` key.
  (b) driver stream-json run WITH flag → `tool_result` frame has numeric `t_ms`;
  WITHOUT flag → no `t_ms`. Model driver tests on existing stream-json headless
  tests (`include_partial_messages_streams_thinking_delta_frames`).
- [ ] **Step 2: Run, watch fail** — `cargo test -p caliban tool_dispatch` /
  serialization test → compile fail (field/param absent).
- [ ] **Step 3: Implement** across the five files.
- [ ] **Step 4: Run, watch pass** — same; plus existing headless tests
  (`tool_result_serializes`, stream-json driver tests) for no regression.
- [ ] **Step 5: touch changed .rs, then full gate** — fmt/clippy/build/test.
- [ ] **Step 6: Commit** — `feat(observability): opt-in t_ms tool-dispatch timing
  on stream-json tool_result frames (#28)`.

---

## Self-Review

- Spec coverage: flag, config, driver timing, encoder thread, frame field, tests
  — all in Task 1.
- No placeholders. Type consistency: `dispatch_ms: Option<u64>` /
  `t_ms: Option<u64>` used identically across builder, encoder, driver, tests.
