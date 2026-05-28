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
