# Parallel sub-agent probe — findings (2026-05-30)

Live probe of caliban's parallel `AgentTool` dispatch path against a
self-hosted Ollama backend whose concurrency we already characterised as
`NUM_PARALLEL=1` (see Method). The motivating question: when caliban
issues N concurrent sub-agent requests at a single-NUM_PARALLEL backend,
does the *client-side* parallel-dispatch machinery behave correctly under
server-side queueing, and what does this combination actually cost in
wall time vs inlining the work? Bookends two earlier findings —
[`2026-05-27-lmstudio-probe-findings.md`](2026-05-27-lmstudio-probe-findings.md)
and [`2026-05-28-ollama-probe-findings.md`](2026-05-28-ollama-probe-findings.md).

**Short answer:** caliban's parallel dispatch is sound — three
`AgentTool` sub-agents fired concurrently, all returned correct results,
no client-side timeouts/drops/panics, 0 leaks. But on a `NUM_PARALLEL=1`
backend the pattern is a **wall-time net loss** vs inlining: P2 (parallel
sub-agents) ran 2.77× slower than P1 (parent does the work inline) for
the same logical task. Sub-agents on a serialising backend buy
**context isolation, not speed.**

## Method

- **Live endpoints:** caliban's native Ollama provider against
  `qwen3.5:27b` on `192.168.1.220:11434` via `OLLAMA_BASE_URL`.
- **Binary:** `target/release/caliban` at `main@8627704` (built
  2026-05-28 10:40), `caliban 0.1.0`.
- **Backend concurrency baseline (already measured 2026-05-29):** 3
  concurrent `/api/generate` requests at `temperature: 0` with fixed
  `num_predict: 64` each kept full solo throughput (~10.5 tok/s) but
  finished at ~7 s / ~14 s / ~20 s for a wall of ~20 s ≈ 3× the single
  baseline of ~7 s. Tokens/sec **not** divided by N → serialised FIFO →
  effective `OLLAMA_NUM_PARALLEL = 1` on this box.
- **Scenario design:**
  - **P1 — baseline (no sub-agents):** parent reads three provider crate
    `lib.rs` files inline (`--no-sub-agent`) and produces a 3-bullet
    summary.
  - **P2 — parallel sub-agents:** parent spawns three `AgentTool`
    sub-agents in a single assistant turn (no `--no-sub-agent`), each
    restricted to the `Read` tool, each tasked with one crate's
    `lib.rs`; parent aggregates returned summaries.
- **Why this design isolates the question:** P1 and P2 perform the same
  logical work (read three files, summarise). The only structural
  difference is the sub-agent abstraction. Wall-time delta = sub-agent
  overhead. Stream-json frame ordering tells us caliban's dispatch
  behaviour; tool_result content tells us whether all sub-agents
  succeeded.
- **Determinism flags:** `--bare --no-skills --no-mcp --no-plugins
  --no-hooks --no-permissions`, `--workspace
  /Users/johnford2002/dev/personal/caliban`, `--max-tokens 4096`,
  `--max-turns 12`, `--output-format stream-json`. P1 adds
  `--no-sub-agent`; P2 keeps `AgentTool` enabled.
- **Artifacts:** `/tmp/ollama-parallel/{p1.json,p2.json,*.err}`.

## Verdict — what works

| Aspect | Result |
|---|---|
| **Parallel `AgentTool` dispatch (client-side)** | ✅ Parent's single assistant turn contained three `tool_use:AgentTool` blocks; caliban's `FuturesUnordered`/`Semaphore` (`parallel.rs`) dispatched them concurrently. `AgentTool` has no `parallel_conflict_key`, confirmed in code. |
| **All sub-agents returned correct results** | ✅ Distinct, accurate one-sentence summaries for ollama / openai / anthropic crates, no `is_error`, no dropped frames. |
| **Client-side stability under queueing** | ✅ No timeouts, hangs, panics, or `result: error` despite the backend serialising for ~7 minutes. |
| **Tool-call leakage into reasoning** | ✅ 0 markers across P1 and P2. |
| **Final `result` frame** | ✅ `result: success` cleanly emitted for both. |

## Wall-time cost

| Scenario | Approx. inferences | Wall |
|---|---|---|
| P1 — parent inline (3 sequential Reads + synthesis) | ~5 | **152 s** |
| P2 — parallel sub-agents (3× [reasoning + Read + summary] + parent dispatch + parent synthesis + 1 spurious parent Read) | ~9–10 | **421 s** (≈ 2.77× P1) |

Even though caliban fired the three sub-agents in parallel, every
sub-agent inference queued at the single Ollama slot. Multiply that
queue depth by the per-sub-agent overhead (each sub-agent runs a full
agent loop: think → call Read → think → emit summary, ~2–3 inferences)
and you get a deeper inference queue than the baseline — which is
exactly what the wall time shows.

## Findings

### F1 — (informational, doc gap) stream-json `tool_use` framing is **deferred** and paired with `tool_result`

The probe initially appeared to show *serial* sub-agent dispatch:
`tool_use:AgentTool → tool_result → tool_use:AgentTool → tool_result →
tool_use:AgentTool → tool_result` strictly paired in the stream. Code
inspection (`caliban/src/headless/mod.rs:243–245`) clarifies: caliban
buffers per-call state and emits the stream-json `tool_use` frame
deferred so it pairs with its `tool_result` rather than firing at
`ToolCallStart` with `input: null`. So the strict pairing is a
**display semantic**, not the dispatch ordering. caliban actually
dispatched all three in parallel (confirmed by the aggregated
`message` frame at index [7] which contains a thinking block plus three
`tool_use` blocks emitted by the model in one assistant turn).

This is defensible UX — clean ordered output for humans — but it
**obscures actual dispatch timing** from stream-json consumers. A
downstream tool can't distinguish "caliban serialised the calls" from
"the backend queued the calls" from "they truly ran in parallel" by
looking at stream-json alone. Frames also carry no timestamps.

**Suggested fix:** document the deferred-emission semantic in
`docs/adr/0025-headless-output-protocol.md` (the headless output protocol
ADR), so consumers know not to use frame ordering as a timing signal.
Optional follow-up: an opt-in `--include-tool-dispatch-events` (or a
millisecond-precision `t_ms` field on `tool_use`/`tool_result` frames)
for consumers that need real dispatch timing.

### F2 — (conceptual / guidance, no code) parallel sub-agents on a `NUM_PARALLEL=1` backend are a wall-time net loss

P2 took 2.77× longer than P1 for the same logical task. The sub-agent
pattern's value on a hosted-API backend (each sub-agent gets its own
fleet capacity → real horizontal parallelism) **does not transfer** to
a single self-hosted model with `NUM_PARALLEL=1`: every sub-agent
inference queues at the one Ollama slot, and the sub-agent abstraction
adds per-sub-agent inference overhead (reasoning + final-summary turns)
on top of the underlying work.

The one durable benefit on such a backend is **context isolation** —
each sub-agent gets a fresh context window, shielding the parent's.
That's real and sometimes worth the wall-time cost.

**Suggested mitigations (no caliban code change required):**

- Document the trade-off in the README "Known model limitations" /
  performance section: parallel sub-agents on `NUM_PARALLEL=1` are
  isolation-not-speed.
- Users who want concurrent throughput should either raise
  `OLLAMA_NUM_PARALLEL` on the server (memory permitting — each slot
  needs its own KV-cache allocation), reach for a continuous-batching
  server like vLLM, or fan out across multiple boxes.
- caliban already exposes `--no-parallel-tools` / `--parallel-tool-limit`
  to cap the dispatch side; neither helps when the bottleneck is the
  backend slot count.

### F3 — (caliban code, Low) `caliban doctor --deep` should detect backend serialisation and warn

Today `/doctor --deep` confirms an Ollama endpoint is reachable and
lists loaded models (`✓ ollama — http://localhost:11434/ (4 model(s)
reachable)`). It does **not** characterise concurrency. A two-request
concurrent probe (mirroring the manual test above — fire two
`/api/generate` calls with `temperature: 0` and `num_predict: 16`,
compare wall vs single) is cheap and would let `doctor` surface
"backend serialises requests (`NUM_PARALLEL=1`); parallel sub-agents
will not speed up" as an explicit warning.

**Suggested placement:** new probe alongside the existing Ollama row in
`caliban/src/diagnostics.rs`, behind `--deep` (it issues two real
inference calls). Skip when the backend is a hosted provider where the
answer is uninteresting.

### F4 — (model quality, not caliban) parent degrades at post-sub-agent synthesis

After three sub-agent `tool_result`s landed, the parent unnecessarily
dispatched its own `Read` of `crates/caliban-provider-openai/src/lib.rs`
(re-doing work a sub-agent had just completed) and then produced a
final answer that referenced only the openai crate, ignoring the
ollama and anthropic summaries the other two sub-agents had returned.
This is the same Qwen-family "model gets confused by accumulated tool
results" pattern documented as F2 / F5 in the 2026-05-28 doc — not a
caliban defect. The agent loop ran to clean `result: success` despite
it.

## Conclusion

caliban's parallel dispatch machinery handles concurrent `AgentTool`
sub-agents correctly under sustained server-side queueing — all three
sub-agents returned accurate results with no client-side anomalies.
The probe's main contribution is shifting the **mental model** for
self-hosted agentic work: on a single-NUM_PARALLEL backend, "parallel
sub-agents" buy context isolation, not throughput, and total wall time
goes up (here, 2.77×) because every sub-agent inference still queues
at the one model slot. The one small caliban-side action item is F3
(a `doctor --deep` concurrency probe so users see this characteristic
of their backend before they're surprised by it); the rest is
documentation and design guidance.
