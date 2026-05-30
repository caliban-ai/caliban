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

## TUI ergonomics (2026-05-30)

caliban's TUI input handler refuses Enter when a turn is in flight
(`caliban/src/tui/events.rs:869–870`, comment: *"Ignore submit if a
turn is already running"*). Both plain user messages and slash-command
invocations hit the same gate, so `/context`, `/cost`, `/usage`, theme/
model switches, transcript overlays — none of which need the model — go
nowhere while the model is streaming. Separately, a user message typed
during a running turn is silently dropped instead of being queued for
the next turn. IE1 and IE2 below stem from that gate; IE3 is a separate
selection/copy issue rooted in TUI mouse capture.

Surveyed: Claude Code, OpenCode (sst/opencode), Crush
(charmbracelet/crush), Cline, and Aider. The three live patterns:
**(A) server-side / state-side queue + dumb input** (Crush, OpenCode);
**(B) module-level command queue + per-command `immediate: bool`**
(Claude Code); **(C) fully blocking REPL** (Aider). For caliban's
ratatui + crossterm shape, **(A)** is the smallest-diff option for IE2
and a narrow slice of **(B)** (just an `immediate` flag on the slash
registry, no full queue) is the right fit for IE1.

### IE1 — Non-model slash commands should execute during inference

- Finding: `SlashCommandMeta` (`caliban/src/tui/slash.rs:33–44`) has no
  way to declare a command as "doesn't need the model." All slash
  commands fall through the same Enter-gated submit path
  (`events.rs:869–870`), so even `/context`, `/cost`, `/usage`, `/help`,
  `/perms`, `/config`, `/model`, `/effort`, `/export`, `/doctor`, and
  overlay-opening commands refuse to fire while `app.running.is_some()`.
  Proof that the gate is per-handler and not architectural: Ctrl+B
  (background bash) already works mid-turn at `events.rs:799–805`, so
  the event loop *can* dispatch immediate actions during a running
  turn — slash commands just don't classify themselves as such.
- File: `caliban/src/tui/slash.rs:33` (struct); `caliban/src/tui/events.rs:869–870` (the submit gate); per-command meta blocks in `caliban/src/tui/slash/*.rs`.
- Severity: Medium UX — a whole class of commands silently no-ops mid-stream, including the diagnostics users most want while waiting (`/context`, `/cost`, `/usage`).
- Suggested fix: add `pub(crate) immediate: bool` to `SlashCommandMeta`. In the submit handler at `events.rs:869`, *before* the running-turn bail, intercept slash-prefixed input through the registry; if `meta.immediate` is true, execute it directly — overlays open, ephemeral status emits, `Continue`/`Overlay`/`Status` outcomes apply — without touching the agent loop. The classifier is mechanical: if a command's `execute()` returns only `Continue` / `Overlay` / `Status` (no turn-spawning outcome), it's `immediate`. Audit and tag at least: `/context`, `/cost`, `/usage`, `/help`, `/perms`, `/config`, `/model`, `/effort`, `/theme`, `/export`, `/doctor`, transcript-overlay commands.
- Caveat: commands that look immediate but mutate session-level state racing the turn (`/compact`, `/rewind`, `/clear`) should stay `immediate: false` and wait for the turn to settle, so they don't fight `RunEnd` semantics.
- References: Claude Code uses an explicit `immediate?: boolean` on the command type (its `src/types/command.ts` line 199), branched in `handlePromptSubmit.ts` (~lines 239–310) so immediate commands fire even when `queryGuard.isActive`. Crush makes the same split implicit: agent-bound UI actions check `isAgentBusy()` and warn (`crush/internal/ui/model/ui.go` lines 1432, 2008), UI-only actions (theme, help, sidebar) just run.

### IE2 — Queue user-typed messages during inference, auto-send on turn end

- Finding: a plain user message typed during a running turn hits the same `events.rs:869` gate and is silently ignored. There is no "queued for next turn" state, no UI indicator, and no auto-send on `RunEnd`. Practical effect: a user who types a follow-up while the model is mid-stream loses the message and has to retype after the assistant returns.
- File: `caliban/src/tui/events.rs:869–870` (the bail); `caliban/src/tui/app.rs:159` (`running: Option<RunningTurn>` — the natural place to colocate a queue); the `RunEnd`-time `app.running = None;` paths in `events.rs:314` and `:328` (the drain points).
- Severity: Medium UX — silent input loss across a common interaction pattern.
- Suggested fix (Crush-style, smallest diff for ratatui + crossterm):
  1. Add `pub(crate) queued: VecDeque<String>` to `App` (or `RunningTurn`).
  2. At `events.rs:869`, instead of returning, push the buffered input into `app.queued`, clear the input bar, and render a `QUEUED: <preview>` indicator (similar shape to the existing pending-Ask hint at `app.rs:202`) so the user sees it was captured.
  3. On `RunEnd` (the `app.running = None;` sites at `events.rs:314,328`), if `app.queued.front().is_some()`, pop and dispatch the queued message as the next user turn via the same code path the input-bar Enter uses.
  4. Esc semantics (two-stage, mirroring Crush): if the queue is non-empty, the first Esc clears the queue (not the in-flight turn); the second Esc within ~2 s cancels the running turn (the existing `events.rs:813` Esc-cancel path becomes the second-stage behavior).
- Multi-message handling for v1: keep it FIFO. Batch consecutive non-slash queued messages into a single user turn at drain time (Claude Code's `dequeueAllMatching` pattern) so a user who hammers Enter doesn't trigger N back-to-back agent runs.
- Caveats: respect `app.pending_ask` (`app.rs:202`) — don't drain into a turn while an Ask modal is open; don't drain across model swaps mid-session without prompting.
- References: Crush stores per-session FIFO in `internal/agent/agent.go:146` (`messageQueue *csync.Map[string, []SessionAgentCall]`), enqueues on busy at :194–211, drains tail-recursively at :716–728 inside `Run()`. OpenCode keeps no client-side queue at all — the TUI fires `session.prompt()` unconditionally and the server's `runLoop` (`session/prompt.ts:1244`) re-iterates while `lastUser.id > lastAssistant.id`; the "QUEUED" badge is purely derived from message IDs (`routes/session/index.tsx:212,1347`). Claude Code's combined queue + priorities (`src/utils/messageQueueManager.ts`, `queueProcessor.ts`) is the heaviest version — overkill for v1.
- Optional v2 (depends on IE1): a `/queue` immediate command that opens a small overlay listing pending items with peek/edit/cancel; the queue snapshot is derived from `app.queued`.

### IE3 — Mouse drag-select doesn't work for copy/paste from caliban's output

- Finding: dragging the mouse across caliban's transcript to select assistant output for copy/paste does not work. caliban calls `crossterm::EnableMouseCapture` at TUI startup (the canonical `EnterAlternateScreen, EnableMouseCapture` pair in `caliban/src/tui.rs` and restored after the external-editor suspend at `caliban/src/tui/external_editor.rs:130,143`). With mouse capture on, all mouse events (button, motion, drag, release) route to the app and the terminal emulator cannot perform its native drag-to-select. caliban currently *uses* mouse events only for scroll wheel (`MouseEventKind::ScrollUp/ScrollDown` in `handle_mouse` at `caliban/src/tui/events.rs:630–660`); everything else it captures is captured incidentally just to keep scroll working.
- File: `caliban/src/tui.rs` (the startup `EnableMouseCapture`); `caliban/src/tui/external_editor.rs:130,143` (the existing toggle pattern — `Disable` before the external editor, `Enable` after); `caliban/src/tui/events.rs:630–660` (handler).
- Severity: Medium UX — copy/paste from output is a baseline expectation. Today users either know the bypass-modifier or fall back to `/export`, and `/export` itself flags "clipboard support not wired in this build — pass a path" (`caliban/src/tui/slash/export.rs`).
- **Empirical Terminal.app results (2026-05-30):**
  - Neither **Option+drag** nor **Shift+drag** bypasses caliban's mouse capture in macOS Terminal.app. Modifier-bypass docs (interim mitigation #1) do not help Terminal.app users.
  - Unchecking `View → Allow Mouse Reporting` (or `Terminal → Preferences → Profiles → <profile> → Window → "Allow Mouse Reporting"`) **does** restore drag-select. But the setting is per-window and **defaults to ON for every new Terminal.app window**, so interim mitigation #2 works but is friction-heavy for daily use.
  - **For reference, Claude Code in the same Terminal.app has neither issue.** Its Ink-based TUI renders to the normal terminal stream rather than entering alternate-screen mode, so it never calls `EnableMouseCapture`. The terminal's native scrollback and native drag-select both work out of the box. caliban uses ratatui in alternate-screen mode, where in-app scroll-wheel requires explicit mouse capture — and that capture is what breaks selection. The two designs make different trade-offs; caliban's only way to recover Claude-Code-style "select works out of the box" *without giving up alt-screen polish* is the **Recommended fix** below: implement mouse-drag-select inside alt-screen, so capture stays on without breaking selection.
- **Recommended fix (default implementation path): mouse-drag-select-to-clipboard inside alt-screen mode.** No feature trade-off, no default flip, no user toggle — keep `EnableMouseCapture` and all the alt-screen polish (persistent statusline / input bar, real overlays, in-app scroll-wheel, clean exit), and add mouse-driven selection on top. caliban already receives every mouse event when capture is on (`events.rs:630–660` currently consumes only `ScrollUp/ScrollDown`); the other variants are unused. Four components:
  1. **Render-time text-position map.** As the transcript renderer draws each styled span, record a `(row, col) → (message_id, char_offset)` lookup. The structurally invasive piece — wraps caliban's existing draw passes in `caliban/src/tui/render.rs` so each laid-out span carries its origin range; reset the map at the start of each frame.
  2. **Mouse-event state machine** in `events.rs::handle_mouse` (`events.rs:630`). `Down(Left)` at (r,c) → set selection start; `Drag(Left)` → update end + trigger redraw; `Up(Left)` → resolve the range via the position map and emit the extracted text. Double-click selects a word, triple-click a line — layered on the same machine. Scroll events stay independent (`ScrollUp`/`ScrollDown` are separate variants).
  3. **Visual feedback.** Each frame, walk the active selection range and overlay `Style::default().bg(highlight)` on cells in the range. Stock ratatui — no special framework support needed.
  4. **Clipboard write.** Emit OSC-52 (`ESC ] 52 ; c ; <base64> BEL`) on selection-end. Honored by kitty / iTerm2 / WezTerm / Ghostty / modern Konsole / Alacritty / foot. For terminals whose OSC-52 support is uncertain (notably macOS Terminal.app — recent and patchy support), fall back to `arboard` (already a transitive dep via `caliban-images`). Also closes the existing `/export` clipboard-support gap (`caliban/src/tui/slash/export.rs` currently flags "clipboard support not wired in this build").
- Honest sizing: ~100–300 LoC plus the renderer wrap for the position map; **medium effort, not "tiny."** In exchange, every terminal — including Terminal.app — gets native-feeling drag-to-copy *with* in-app scroll-wheel preserved, no user toggles, no parity-matrix regression. Prior art in the Rust TUI ecosystem (verify before relying on): **Helix** (ratatui-adjacent editor) implements mouse text selection in alt-screen mode; **Zellij** (alt-screen TUI multiplexer) does selection across panes; **gitui** (ratatui) implements mouse selection. None of them abandoned alt-screen to do it.
- **Interim mitigations** while the recommended fix is being built (or for users who want immediate relief):
  1. **Modifier-key bypass docs** for terminals that honor it: **Shift+drag** on kitty, Ghostty, WezTerm, foot, Alacritty; iTerm2 additionally accepts Option+drag and ⌘+drag. macOS Terminal.app does NOT honor any bypass (verified above); this mitigation does not apply there.
  2. **Terminal.app preference workaround** — uncheck `View → Allow Mouse Reporting` (or the per-profile equivalent at `Terminal → Preferences → Profiles → <profile> → Window`, [Apple support](https://support.apple.com/guide/terminal/turn-on-mouse-reporting-trmlc69728a5/mac)). Restores native drag-select; cost is losing caliban's in-app scroll-wheel for that window; the setting **resets to ON for every new Terminal.app window** so it's friction-heavy for daily use.
  3. **In-app `/mouse` runtime toggle** (~20 LoC). `/mouse on`/`/mouse off` flips `crossterm::EnableMouseCapture` / `DisableMouseCapture` on demand — the same Disable/Enable pattern already at `external_editor.rs:130,143`. Useful as an escape hatch even after the recommended fix lands; under the recommended approach there's no need to flip the default and no parity-matrix regression.
- References: the modifier-key bypass is documented by the terminal emulators themselves (iTerm2 Selection docs, kitty `terminal-select` docs, Ghostty docs); it's also what Helix and many Bubbletea-based TUIs tell their users to do. The runtime `/mouse` toggle pattern ships in lazygit and a few Bubbletea apps. OSC-52 reference: VT100.net / xterm docs; widely supported.
- Caveat: don't surprise the user by toggling capture under their feet. If `/mouse off` is supplied, persist for the session and surface a status line ("mouse capture off; scroll uses terminal-native"); re-enabling on focus-loss or modal-open would be confusing.
