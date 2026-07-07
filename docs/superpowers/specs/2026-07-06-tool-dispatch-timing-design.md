# Stream-json Tool-Dispatch Timing — Design (#28)

**Goal:** Opt-in dispatch-latency timing on stream-json tool frames, so consumers
can correlate tool latency (e.g. for `NUM_PARALLEL` characterisation).

## Decision

Add a `--include-tool-dispatch-events` flag (mirroring the existing
`--include-hook-events`) that attaches a **`t_ms` dispatch-duration field to the
`tool_result` frame** in stream-json output. Default off → output is
byte-identical to today.

### Why `t_ms` on `tool_result` only (not `tool_use`)

Both the `tool_use` and `tool_result` frames are emitted together at
`ToolCallEnd` time (the `tool_use` input JSON is only complete then —
`StreamJsonEncoder::tool_call`). A timeline offset on `tool_use` would therefore
be the same instant as the result and carry no information. The direct,
unambiguous latency signal is the **dispatch duration** — time from
`ToolCallStart` to `ToolCallEnd` — which lands naturally on `tool_result`. Each
parallel tool's `tool_result` carries its own duration, which is exactly what
latency / parallelism analysis needs (no consumer subtraction required).

### Mechanism

- Measure in the headless driver, not the core stream protocol (agent-core is
  untouched). The driver keeps `tool_dispatch_starts: HashMap<String, Instant>`:
  - At `TurnEvent::ToolCallStart`, when the flag is on, insert
    `(tool_use_id, Instant::now())`.
  - At `TurnEvent::ToolCallEnd`, when the flag is on, take the start and compute
    `dispatch_ms = start.elapsed().as_millis() as u64`.
- Thread `dispatch_ms: Option<u64>` into `FrameEncoder::tool_call`. Only
  `StreamJsonEncoder` uses it (passes it to the `tool_result` builder); the other
  impls ignore it. `None` when the flag is off → the field is omitted.
- `ToolResult` frame gains `t_ms: Option<u64>` with
  `#[serde(skip_serializing_if = "Option::is_none")]`.

## Files

- `caliban/src/args.rs` — `--include-tool-dispatch-events` flag (mirror
  `include_hook_events`).
- `caliban/src/startup/drivers.rs` — wire `args.include_tool_dispatch_events`
  into `HeadlessRunConfig`.
- `caliban/src/headless/mod.rs` — config field + default; driver
  `tool_dispatch_starts` map; record at `ToolCallStart`, compute + pass at
  `ToolCallEnd`.
- `caliban/src/headless/encoder.rs` — `FrameEncoder::tool_call` gains
  `dispatch_ms: Option<u64>` (4 impls; only `StreamJsonEncoder` uses it).
- `caliban/src/headless/events.rs` — `ToolResult.t_ms` + `tool_result()` builder
  param.

## Testing

- **Flag on:** a stream-json run through the driver with the flag set → the
  `tool_result` frame carries a numeric `t_ms` (>= 0).
- **Flag off (default):** the same run → no `t_ms` key on the `tool_result`
  frame (byte-compatible with current output).
- **Builder/serialization unit test:** `tool_result(id, false, content, Some(7))`
  serializes `t_ms: 7`; `None` omits the key.

## Acceptance

With `--include-tool-dispatch-events`, each stream-json `tool_result` frame
carries a `t_ms` dispatch-duration; without it, output is unchanged. Enables
latency correlation for `NUM_PARALLEL` characterisation.
