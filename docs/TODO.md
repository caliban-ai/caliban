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
removed from this list. Two items remain partially done:

- Finding: MaxTokens budget-blowout recovery is implemented but disabled by default. The two-stage recovery (Stage A one-shot budget escalation to `escalated_max_tokens = 16_384`, Stage B meta-continuation) shipped in #60, and the clean halt + `StopCondition::MaxTokensExhausted` shipped in #68 — which also set `max_tokens_recovery = false` by default because Stage A's re-issue re-emitted `TurnEnd` and inflated the turn count past the cap.
  - File: crates/caliban-agent-core/src/agent.rs:62,101 (`max_tokens_recovery: bool`, default `false`); crates/caliban-agent-core/src/stream/mod.rs:1076,1194 (recovery gate); :354–355 (per-turn escalation tracking).
  - Remaining work: (1) confirm/fix Stage A's `TurnEnd` double-count so recovery can be safely re-enabled (split attempt-end vs turn-end semantics); (2) add a CLI flag (e.g. `--max-tokens-recovery`) to opt back in — there is no flag today, the field is only settable in code. Pair with an `/effort low` suggestion in the surfacing message so the user has a one-keystroke remediation.

- Finding: the custom statusline runs but is never rendered. `StatuslineRunner`, the `settings.statusLine` schema, and the claude-code-compatible stdin context shipped in `caliban-settings` (Plan C 2026-05-26), but nothing in the TUI invokes it — `/statusline` is still a stub (caliban/src/tui.rs:707) and `render.rs` has no status-line prefix path. Matrix row K is 🟡 pending this.
  - File: crates/caliban-settings/src/statusline.rs (runner — done); caliban/src/tui/render.rs (render-prefix integration — missing); invocation site after `TurnEnd`/`RunEnd`.
  - Remaining work: spawn the configured command after each `TurnEnd`/`RunEnd` (and at session start), cap stdout to one line (~120 chars), cache between turns so it doesn't run mid-render, and render it as a prefix/suffix on the existing statusline. Default timeout 200 ms; on timeout render the previous output and log.

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

**Resolution status (2026-05-28):** F2/F3/F4 landed in #71, F6 in #69,
F7/F12 in #70, F11 in #69, F13/F14/F15 in #72 — all merged, so their
entries were pruned. Only **F16** (below) remains open; it isn't
addressed by any PR yet.

- Finding (LMStudio F16 — NOT yet addressed by any PR): Headless `-p` running a `Write`/`Edit`/`Bash` prompt without `--auto-allow` fails on the first such tool call. Surfaced while documenting F15 in #72. Headless `-p` resolves to `PermissionMode::Default`, whose rule tail **Asks** for mutating tools; in a non-interactive context the `Ask` resolves to a hard deny, so a headless prompt that needs to write a file or run a command fails on the first mutating call. Read-only tools (Read/Glob/Grep) are Allowed by the default tail, which is why F15/E5 saw tools "just work" — they only exercised reads.
  - Commit: 8b87b35 (LMStudio probe baseline)
  - File: crates/caliban-agent-core/src/permissions.rs (default-rules tail + `NonInteractiveAskHandler { auto_allow: false }` at :576); headless dispatch path in caliban/src/headless/mod.rs
  - Severity: Medium — a whole tool class fails silently in the headline headless mode.
  - Suggested fix: decide the intended headless default. Either (a) emit a clear error ("tool X requires --auto-allow or an --allow rule in headless mode") instead of an opaque deny, plus document the requirement loudly; or (b) make headless `-p` default to a more permissive mode for workspace-scoped mutating tools. Pairs with F15's `permission_mode` surfacing (now in `system/init` per #72) so the failure is at least diagnosable.

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

- Finding (IE3-followup — Medium for Terminal.app users): clipboard write is OSC-52 only. Some macOS Terminal.app configurations don't honour OSC-52, so drag-select still highlights and extracts but the clipboard write is silent. Plan adds an `arboard` fallback when OSC-52 fails or env detection says no; v1 stays OSC-52-only because adding `arboard` as a direct dep on the binary pulls in X11/Wayland clipboard libs that break headless builds.
  - File: `caliban/src/tui/clipboard.rs` (add fallback path); `caliban/Cargo.toml` (optional `arboard` feature, default-on, that headless / CI can disable with `--no-default-features`).
  - Severity: Medium for Terminal.app users (the OSC-52 emit returns Ok but the terminal silently drops the sequence); Low otherwise.

- Finding (caliban sub-agent driver — informational, F2-family confirmation): driving sub-agents through caliban + Ollama (`qwen3.5:9b-mlx` on the remote box) at `--max-turns 5` hit `result: max_turns` (exit 75) on a single-task Grep-and-list prompt — the model kept re-issuing Grep instead of summarising. Confirms F2/F5 (Qwen enumerated-plan under-execution and 9B coherence ceiling) extends to the MLX-quantised variant; `--max-turns 5` is too tight for any agentic delegation against 9B Qwen. Practical floor for delegation is `--max-turns 10–12` and the model is only reliable for 1–2 step lookups.
  - File: no code change; document on the next probe doc + matrix update.
  - Severity: informational — pure model-quality observation against the caliban driver path; caliban handled the halt cleanly (distinct exit 75 per `#70`'s structured result).

