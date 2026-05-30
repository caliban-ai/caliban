# Ollama support probe — findings (2026-05-28)

Live probe of caliban's native Ollama provider against a local Ollama
instance serving `qwen3.5:9b`. The motivating question: does Ollama's
GGUF server-side tool-call parsing avoid the **tool-calls-leak-into-the-
reasoning-channel** bug that the LM Studio MLX engine exhibits (see
`README.md` "Known model limitations" and
[`2026-05-27-lmstudio-probe-findings.md`](2026-05-27-lmstudio-probe-findings.md))?

**Short answer: yes.** On Ollama/GGUF the leak does not reproduce — tool
calls arrive as structured `tool_use` blocks in every scenario. The only
weaknesses observed are model-quality (chain-following on the 9B build),
not caliban parsing bugs.

## Method

- **Live endpoint:** Ollama `0.24.0` on `http://localhost:11434`
  (caliban's native `--provider ollama`; default base URL, no
  `OLLAMA_BASE_URL` needed).
- **Binary:** `target/release/caliban`, **rebuilt from `main` at
  `8627704`** (post-#75). An earlier pass ran against a stale binary
  (built before #70 landed) and produced a false session-persistence
  failure — see F1.
- **Model under test:** `qwen3.5:9b` — `format: gguf`, `family: qwen35`,
  `parameter_size: 9.7B`, `quantization_level: Q4_K_M` (per
  `GET /api/tags`).
- **Scenario battery:** plain chat, reasoning capture, single tool call
  (Glob), 2-step chain (Glob→Read), 3-step chain (Glob→Read→Grep),
  parallel tool calls, plus cross-cutting checks (bad model name,
  unreachable endpoint, `caliban doctor --deep`, `-p --session`
  persistence across two turns). All re-run on the fresh binary with
  identical results, then **extended with long agentic chains** — a
  5-turn investigative `--session` with deep within-turn tool use (L1)
  and a clean 4-turn context-recall session (L2). See "Extended battery"
  below.
- **Determinism flags:** every run used `--bare --no-skills --no-mcp
  --no-plugins --no-hooks --no-sub-agent --no-permissions` and
  `--workspace /Users/johnford2002/dev/personal/caliban` (real files for
  Glob/Read/Grep). Only read-only tools were exercised, so
  `--no-permissions` plus the default rule tail dispatch cleanly (no
  mutating tools, so F16 from the LM Studio probe is not in scope).
- **Artifacts:** `/tmp/ollama-probe*/` — stale-binary pass in
  `ollama-probe/`, fresh-binary re-run in `ollama-probe2/`, agentic L1 in
  `ollama-probe2-ses/`, recall L2 in `ollama-probe2-rec/` (stream-json
  captures + stderr).

## Verdict — what works

| Capability | qwen3.5:9b (Ollama) | Notes |
|---|---|---|
| Plain chat (`-p`, text) | ✅ | "pong", exit 0, ~14.5 s incl. cold model load |
| Reasoning capture | ✅ | Reasoning preserved as a clean `thinking` block; correct answer (17×23 = 391) |
| **Tool call NOT leaked into reasoning** | ✅ | **0 `<tool_call>` / `<function=` / `</think>` markers in text/thinking across every scenario** — the LM Studio MLX failure does not reproduce |
| Single tool call (Glob) | ✅ | Structured `tool_use` block; correct count (6 files) |
| 2-step chain (Glob → Read) | ✅ | Both dispatch; correctly reported toolchain channel `1.95.0` |
| 3-step chain (Glob → Read → Grep) | ⚠️ model quality | Only 2/3 tools issued; model skipped step 3. **No loop, no silent stall, no leak** — ended `result: success`. Contrast LM Studio, which looped re-emitting Glob |
| Parallel tool calls | ⚠️ model choice | Model issued two `Read`s sequentially (one per turn), not as a parallel batch; both round-tripped correctly |
| stream-json frames | ✅ | `system/init → tool_use → tool_result → message → result` ordering correct |
| Session persistence (`-p --session`) | ✅ | Roles persist `[system,user,assistant,user,assistant]`; turn 2 correctly recalled "42" (verifies #70 on the fresh binary) |
| Bad model name | ✅ | Exit 1, `model unavailable: model 'qwen-nonexistent:9b' not found` |
| Unreachable endpoint | ✅ | Exit 1, `network error: HTTP request failed ... (http://localhost:1/api/chat)` |
| `caliban doctor --deep` | ✅ | `✓ ollama — http://localhost:11434/ (4 model(s) reachable)` |

## Extended battery — long agentic chains

To stress the provider under realistic agentic load, two longer runs on
the fresh binary:

- **L1 — 5-turn investigative session** (tools on, `--session`, stream-json):
  the model was walked through a multi-step codebase investigation (list
  provider crates → read `ollama/src/lib.rs` → grep `tool_call_id` →
  conclude → summarize), each turn building on the last.
- **L2 — 4-turn clean recall session** (`--no-tools`): three facts stated
  across turns 1–3, recalled in turn 4 — isolates context replay from
  tool-flailing.

| Aspect under agentic load | Result |
|---|---|
| **Tool-call leakage into reasoning** | ✅ **0 markers across all 5 L1 turns / 46 messages** — holds under sustained load |
| Multi-turn context replay | ✅ L2 turn 4 correctly recalled all three facts (teal / 7 / Brindlemark) across 4 turns |
| Session persistence at scale | ✅ 46 messages persisted across L1's 5 turns |
| `result: max_tokens` halt + exit code | ✅ L1 turn 1 halted on budget (exit 1) |
| `result: max_turns` halt + exit code | ✅ L1 turn 3 halted at the turn cap (exit 75) — distinct code |
| Model coherence under load | ❌ severe degradation — see F5 |

The headline survives the harder test: **no tool-call leak, correct
context replay, correct resilience halts** — all the caliban-side
behavior held. What broke was the model.

## Remote 27B comparison (`qwen3.5:27b`)

The same battery re-run against a **remote** Ollama (`qwen3.5:27b`,
GGUF/Q4_K_M, on `192.168.1.220:11434` via `OLLAMA_BASE_URL`,
`--max-tokens 4096`). caliban talked to the LAN endpoint with no change
beyond the env var; ~17.5 min for 9 scenarios (~2 min/scenario over LAN).
Every scenario: `result: success`, **leak = 0**. Comparison to the 9B:

| Scenario | 9B (local) | 27B (remote) |
|---|---|---|
| Single tool, 2-step, parallel | ✅ | ✅ (identical, correct answers) |
| 3-step single-turn chain (Glob→Read→Grep) | ⚠️ skipped Grep | ⚠️ **also skipped Grep** — F2 persists across sizes |
| Parallel reads | sequential | sequential (model choice) |
| 5-turn investigative session | ❌ hallucinated edits, wandered, claimed "no context", hit max_tokens/max_turns | ✅ **fully coherent** — see below |

The 5-turn session is the decisive difference. On the 27B every turn was
correct and on-task: turn 1 tried `Glob 'crates/caliban-provider-*'`
(0 matches — the dir-glob limit), **recognized the miss and fell back to
`Bash: ls -d crates/caliban-provider-*`** to count 6 provider crates;
turn 2 accurately summarized `OllamaProvider`; turn 3 grepped
`tool_call_id` and read the `// No tool_call_id correlation.` comment
correctly; turn 4 (no tools) used that prior finding to answer "no, it
does not round-trip `tool_call_id`"; turn 5 summarized coherently. Clean
21-message session, no flailing, no `max_*` blowouts. This is exactly the
F5 degradation **resolved by a stronger model** — confirming F5 is a 9B
capability ceiling, not a caliban defect.

F2, however, **reproduced on the 27B**: the explicit "do these 3 steps,
one tool per step" single-turn instruction still stopped after Glob→Read
and skipped the Grep step. Yet the same model did fluent multi-step tool
work *across turns* (A5). So F2 is better characterized as a Qwen-family
quirk — under-executing an enumerated multi-step plan crammed into a
single turn — than a raw capability limit, and it is independent of model
size.

## Findings

### F1 — (methodology, resolved) Stale local binary masked #70's session-persistence fix

The first pass ran `target/release/caliban` built **2026-05-27 19:11**,
but #70 (`c705b32`, the F1/F4 headless `-p --session` persistence fix)
landed **2026-05-27 21:40** — 2.5 h later, along with #71–#74. Against
the stale binary, two `-p --session` turns left only `[system]` in the
session file and turn 2 had no recall. After `scripts/rebuild.sh
--release` (now `main@8627704`), the same scenario persists
`[system,user,assistant,user,assistant]` and turn 2 answers "42". No
caliban code finding — a reminder to rebuild local binaries after a merge
wave before probing.

### F2 — (model quality, no caliban action) Enumerated single-turn tool chains run incomplete (both 9B and 27B)

On a 3-step plan (Glob → Read → Grep) the model stopped after Read,
skipped the Grep step, and produced an off-target answer. Crucially, the
agent loop, tool dispatch, parsing, and termination were all correct —
the run ended `result: success` with no loop and no leaked tool call.
**This reproduced on both `qwen3.5:9b` and the larger `qwen3.5:27b`**, so
it is not a size ceiling — it's a Qwen-family instruction-following quirk:
the model under-executes an enumerated N-step plan crammed into one turn,
deciding it has "enough" after a couple of calls. The same 27B did fluent
multi-step tool work when the steps were spread across conversation turns
(see the Remote 27B comparison). No caliban change indicated. Mitigations
are prompt-side (one step per turn) or model-side.

### F3 — (model quality) Tool calls issued sequentially, not in parallel

Asked to read two independent files, the model emitted the two `Read`
calls across two turns rather than as a single parallel batch. caliban
supports parallel tool execution (default on), so this is a model
emission choice, not a caliban limitation.

### F4 — (doc accuracy, recommended) Clarify the README "Known model limitations" Qwen3 note

`README.md:281–335` describes the Qwen3 tool-call-in-reasoning breakage
in terms that read as a general Qwen3 limitation, though the root cause
is the **LM Studio MLX engine not parsing Qwen-native `<tool_call>` XML
into the OpenAI `tool_calls` array**. This probe shows the **same model
family on Ollama/GGUF parses tool calls correctly** — the leak is
engine-specific, not model-specific. Recommend refining that section to:
(a) scope the parsing leak to LM Studio's MLX path, (b) note Ollama/GGUF
(and `mlx_lm.server` with explicit `--reasoning-parser` /
`--tool-call-parser` flags) parse correctly, and (c) keep the residual
"3+ step chains are unreliable on 9B reasoning builds" caveat as a
model-quality note.

### F5 — (model quality, no caliban action) `qwen3.5:9b` degrades sharply under multi-step agentic load

In L1, once the session grew past ~2 turns of real tool use, the model
lost the plot: it hallucinated a different task (tried to *add error
variants* to `error.rs` and invoked `NotebookEdit` on a non-notebook
file), wandered into unrelated greps (`image_format`, `png|jpg`), and in
turns 4–5 falsely claimed it had "no access to previous steps" despite
46 messages of persisted, replayed context. L2 proves the context was
present and replayed correctly (clean 4-turn recall succeeded), so this
is a capability ceiling of the 9B Q4 build, not a caliban defect. caliban
contained every failure cleanly — bad tool inputs returned structured
errors, runaway turns hit the `max_turns` cap, budget blowouts hit the
`max_tokens` halt. Practical guidance: this model is reliable for 1–2
step tool tasks; for deeper agentic chains, prefer a larger/stronger
model. A reasoning model also wants a generous per-turn `--max-tokens`
(L1 turn 1's halt at 1500 was partly reasoning-budget starvation);
4096+ is more appropriate for agentic use.

**Confirmed by the 27B re-run:** the identical 5-turn investigative
session on `qwen3.5:27b` (at `--max-tokens 4096`) was fully coherent —
correct findings each turn, prior-context reuse, intelligent tool
fallback (`Bash` when `Glob` couldn't list dirs), zero `max_*` halts.
F5 is therefore confirmed as a 9B capability ceiling that a stronger
model clears. caliban behaved identically across both; the differentiator
was purely the model.

### F6 — (probe-config note) Under `--no-permissions`, a flailing model can attempt mutating tools

L1 ran with `--no-permissions` (all tools allowed) for determinism. When
the model hallucinated an edit task it attempted `NotebookEdit` — which
failed harmlessly (invalid notebook JSON), and no `Edit`/`Write`
succeeded. In a normal session the permission layer would gate these
(headless `-p` denies mutating tools without `--auto-allow` per the LM
Studio probe's F16; the TUI prompts). Worth remembering that
`--no-permissions` removes that backstop — appropriate for a read-only
probe, not for unattended agentic runs with a weak model.

## Conclusion

Switching Qwen from the LM Studio MLX engine to **Ollama (GGUF, `qwen35`
parser)** resolves the tool-calls-in-reasoning bug at the provider
boundary, exactly as the ecosystem-side parsing model predicts: Ollama does
server-side what LM Studio's MLX path does not. caliban's Ollama provider
handled every scenario cleanly — structured tool calls, preserved
reasoning, correct stream-json framing, multi-turn session persistence and
replay, resilience halts (`max_tokens` / `max_turns`) with distinct exit
codes, and clear error surfaces — and **all of this held under sustained
5-turn agentic load with zero tool-call leakage**. The remaining rough
edges are entirely `qwen3.5:9b` capability limits — long chains, sequential
reads, and sharp coherence degradation past ~2 agentic turns (F5) — not
caliban defects. For agentic work, pair caliban/Ollama with a stronger
model and a generous per-turn `--max-tokens`.
