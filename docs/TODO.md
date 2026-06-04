# Caliban TODOs

Living backlog of small-to-medium findings that aren't large enough to warrant
a full spec under `docs/superpowers/`, but are concrete enough to act on. New
findings should follow the existing entry shape: `Finding → Commit → File →
Lines → Suggested fix` with sub-bullets for placement and notes.

When a finding is closed, delete it from this file in the same PR that closes
it (the commit history is the audit trail). Promote items to a proper spec if
they grow.

---

## Claude Code parity sweep

The bulk of this sweep closed across Plan A/B/C (PR #60) and the probe
follow-up wave (#66–#74): `/clear` context reset, MaxTokens halt,
stream-idle watchdog, stalled-tokens UI hint, refusal/content-filter
surfacing, reactive compaction, failure-aware hook dispatch +
`TurnDecision`, `/cost`, `/doctor`, autocompact, microcompact,
tool-result size cap, `/effort`, `/resume`, `/context`, `/export`, the
4-button permission modal, and the conversation-level prompt-cache
marker all shipped — verified in code on `main` as of 2026-05-28 and
removed from this list.

**Closed 2026-06-01:** MaxTokens recovery is re-enabled by default.
Stage A budget escalation is now hoisted above the `TurnEnd` yield
+ `turns_completed +=1` increment (`stream/mod.rs` post-stream-drain
silent-retry block, mirroring the reactive-compact arm at ~line 595),
so the retry is invisible to consumers and doesn't burn turn slots.
Regression test
`stage_a_retry_does_not_double_count_turn` guards the invariant.
CLI flag `--max-tokens-recovery [bool]` and settings key
`max_tokens_recovery` (default `true`) are wired with CLI > settings
> default precedence in `caliban/src/startup.rs::build_agent`. The
CLI + headless surfacing messages now include the `/effort low`
one-keystroke remediation hint (TUI already had it).

---

## Ollama probe follow-ups (2026-05-27)

F1/F2/F3/F5 from the original probe were closed in PR #66 (`14afe66`).
**Update 2026-05-27:** F4 (session persistence) is fixed in **#70**;
F6 (continue-past-MaxTokens) is fixed in **#68**. F7 (Ollama
`tool_call_id` round-trip, future-proofing) remains open.

- Finding (F7): Tool-result correlation has no `tool_call_id` round-trip for Ollama. The IR `ToolResult` block carries the `tool_use_id`, but the Ollama adapter drops it when serializing to the wire (Ollama's `role: "tool"` message format doesn't define a correlation field today). Correct for the current Ollama protocol, but future-proofing only: (1) if Ollama later adds a `tool_call_id` field, our adapter won't forward it without a code change; (2) parallel tool calls rely on positional order rather than ID.
  - Commit: 43a288f (Ollama probe baseline)
  - File: crates/caliban-provider-ollama/src/ir_convert.rs:111–131
  - Severity: none today (informational). Track here so we don't lose it when Ollama's tool-message schema evolves.

---

## LMStudio probe follow-ups (2026-05-27)

Probe ran caliban's OpenAI provider against LMStudio
(`http://localhost:1234/v1`) serving three loaded models
(`qwen2.5-coder-7b-instruct-mlx`, `qwen3.5-9b-mlx`,
`google/gemma-4-e4b`). Full writeup:
[`docs/2026-05-27-lmstudio-probe-findings.md`](2026-05-27-lmstudio-probe-findings.md).

**Resolution status (2026-05-31):** F2/F3/F4 landed in #71, F6/F11 in
#69, F7/F12 in #70, F13/F14/F15 in #72, and F16 (headless `Ask` → deny
with no actionable hint) landed on `feature/recommendations` —
`NonInteractiveAskHandler` now returns a tool-class-aware remediation
(`--permission-mode acceptEdits` for file-edit tools, narrow `--allow
'Bash(<glob>)'` for Bash, generic `--allow '<Tool>'` otherwise). No
open findings remain in this section.

---

## Parallel sub-agent probe follow-ups (2026-05-30)

Probe drove caliban to spawn three parallel `AgentTool` sub-agents
against a self-hosted Ollama backend whose `NUM_PARALLEL=1` was
characterised empirically the previous day. Full writeup:
[`docs/2026-05-30-parallel-subagent-probe-findings.md`](2026-05-30-parallel-subagent-probe-findings.md).
caliban's dispatch machinery handled the load cleanly (all sub-agents
returned correct results, no client-side anomalies, 0 leaks). One
small caliban-side action item surfaced; F1/F2/F4 from the probe are
documentation/guidance, not code.

- Finding (F3 — Low): `caliban doctor --deep` should detect single-NUM_PARALLEL backend serialisation and warn. Today the doctor probe confirms an Ollama endpoint is reachable and lists loaded models, but it does not characterise concurrency. Fire two `/api/generate` calls with `temperature: 0` and `num_predict: 16`; if the wall time is ≈ 2× single, the backend serialises (`NUM_PARALLEL=1`) and parallel sub-agents will not speed up — surface that as a warning so users see it before being surprised by it.
  - Commit: (probe baseline; new probe, no prior PR)
  - File: `caliban/src/diagnostics.rs` — new probe alongside the existing Ollama row; gated behind `--deep` (it issues two real inference calls).
  - Severity: Low — diagnostic-only; no behavioural defect.
  - Suggested placement: extend the existing Ollama probe section so the row reads e.g. `✓ ollama — http://… (4 models, NUM_PARALLEL=1 detected: parallel sub-agents will serialise)`. Skip when the configured provider is a hosted API where the answer is uninteresting.
  - Optional follow-up: if F1's stream-json deferred-`tool_use` semantic is also addressed, an opt-in `--include-tool-dispatch-events` (or millisecond `t_ms` field on `tool_use`/`tool_result` frames) would let consumers correlate dispatch timing with this `NUM_PARALLEL` characterisation.


---

## TUI ergonomics follow-ups (post-IE1/IE2/IE3 PR)

The original IE1 / IE2 / IE3 findings shipped (immediate slash
commands during inference; queued user messages with two-stage Esc;
mouse drag-select inside alt-screen with OSC-52 clipboard write). The
items below are intentional v1 scope cuts noted during implementation,
to be picked up in follow-up work.

- Finding (IE1-followup — Low): only 13 of 37 registered slash commands are tagged `immediate: true` in v1. Many of the remaining 24 (e.g. `/skills`, `/memory`, `/plugin`, `/plugins`, `/hooks`, `/agents`, `/mcp`, `/statusline`, `/tui`, `/login`, `/logout`, `/status`, `/feedback`, `/heapdump`, `/voice`) likely qualify but were left default-false for conservatism. Audit each `execute()` body — if the only `SlashOutcome` variants returned are `Continue` / `Overlay` / `StatusMessage`, flip to `immediate: true`. The regression test in `caliban/src/tui/slash.rs::known_immediate_commands_are_tagged_in_builtin_registry` is the place to extend.
  - File: `caliban/src/tui/slash/*.rs` (per-command meta blocks)
  - Severity: Low — UX nicety; users can already opt these in by waiting for the turn to settle.

- Finding (IE2-followup — Low): drain pops ONE queued message per `RunEnd`. The original TODO suggested batching consecutive non-slash queued messages into a single user turn at drain time (Claude Code's `dequeueAllMatching` pattern), so a user who hammers Enter doesn't get N back-to-back agent runs. Not implemented for v1.
  - File: `caliban/src/tui/events.rs::drain_one_queued`; `caliban/src/tui.rs` (main loop drain check).
  - Severity: Low — multi-queued use case is uncommon; if needed, change `drain_one_queued` to `drain_consecutive_non_slash` returning a joined string.

- Finding (caliban sub-agent driver — informational, F2-family confirmation): driving sub-agents through caliban + Ollama (`qwen3.5:9b-mlx` on the remote box) at `--max-turns 5` hit `result: max_turns` (exit 75) on a single-task Grep-and-list prompt — the model kept re-issuing Grep instead of summarising. Confirms F2/F5 (Qwen enumerated-plan under-execution and 9B coherence ceiling) extends to the MLX-quantised variant; `--max-turns 5` is too tight for any agentic delegation against 9B Qwen. Practical floor for delegation is `--max-turns 10–12` and the model is only reliable for 1–2 step lookups.
  - File: no code change; document on the next probe doc + matrix update.
  - Severity: informational — pure model-quality observation against the caliban driver path; caliban handled the halt cleanly (distinct exit 75 per `#70`'s structured result).

---

## CI / developer experience (2026-05-30)

PRs #78 and #79 surfaced two CI/DX gaps that landed fixes here (this PR), plus one residual flaky test that needs deeper investigation. The two closed gaps are noted in the commit history; only the open flake is tracked below.

- Finding (CI/DX-1 — Low, flaky): `crates/caliban-agent-core/tests/hooks_shell.rs::stdout_json_updated_input_parses` fails intermittently on CI (Linux runner) with `unexpected: Allow`. The test spawns a shell script that emits a `hookSpecificOutput { updatedInput }` JSON envelope via heredoc; the assertion expects `HookDecision::UpdatedInput("echo safe")` and got `HookDecision::Allow` once on PR #79's first run, then passed cleanly on `gh run rerun --failed`.
  - File: `crates/caliban-agent-core/tests/hooks_shell.rs:99–118` (the test); `crates/caliban-agent-core/src/hooks_router.rs:100–186` (`ShellCommandHook::dispatch`).
  - Severity: Low — only seen once across the IE1/IE2/IE3 + follow-up wave; re-run cleared it without any code change. But it IS real and will keep wasting CI minutes + producing red-then-green noise on PRs unless rooted.
  - Hypothesis: `tokio::process::Command::wait_with_output()` either races the pipe drain or returns before the child's heredoc cat output is fully captured under runner load. The fallthrough `HookDecision::Allow` is reached at `hooks_router.rs:163–171` when `parse_decision_blob(&stdout_text)` returns `Allow` (empty / unparseable / no `hookSpecificOutput`) AND the trimmed stdout doesn't start with `{`. Empty stdout matches both — strong evidence the pipe wasn't drained.
  - Investigation hints: (a) instrument `dispatch` to log `output.stdout.len()` and `output.status` when the JSON parse fails — confirm whether stdout is empty in the failing case. (b) try replacing `wait_with_output()` with explicit `child.stdout.take()` + `read_to_end` after `wait()` to control the drain ordering. (c) repro under `stress-ng --cpu 4` locally to mimic runner load.
  - Fix candidates: (1) drop the test's `heredoc` for a single-line `echo` (smaller pipe-write window); (2) loop the test under a retry decorator (Rust doesn't have built-in; add a per-test helper that re-runs once on `unexpected: Allow`); (3) read stdout explicitly post-wait.
  - Not done here because: deeper investigation belongs in its own focused PR — guessing at the fix would risk masking the root cause.

---

## Performance & scaling (2026-05-31)

**Closed 2026-06-01:** the two-stage tool surface — design half — landed
as ADR-0046 and spec
`docs/superpowers/specs/2026-05-31-two-stage-tool-surface-design.md`.
Implementation (Phases 1–6 of `docs/superpowers/plans/2026-05-31-two-stage-tool-surface.md`)
shipped on `strategic/two-stage-tool-surface`: `tools.lazy_mcp = true`
opt-in hides MCP tools from the wire payload behind a new
`ToolSearch` built-in that activates matches into a sidecar
`McpActivationSet` (LRU at `tools.max_active_schemas`, default 24).
Per-server eager override via `[mcp_servers.X] lazy = false`.
Sub-agent inheritance via frontmatter `inherit_active_mcp`. `/context`
shows the active set when the feature is on.

Built-in / plugin laziness is **not** in scope yet — see the spec's
"Non-goals" section. Track follow-ups (activation persistence across
session restart, `/tools` overlay, telemetry counters, `caliban tools`
CLI) under "Open questions for v1.1" in the spec.

