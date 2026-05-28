# Caliban TODOs

Living backlog of small-to-medium findings that aren't large enough to warrant
a full spec under `docs/superpowers/`, but are concrete enough to act on. New
findings should follow the existing entry shape: `Finding → Commit → File →
Lines → Suggested fix` with sub-bullets for placement and notes.

When a finding is closed, delete it from this file in the same PR that closes
it (the commit history is the audit trail). Promote items to a proper spec if
they grow.

---

## Claude Code parity sweep (open)

- Finding: /clear does not immediately reset the context-window utilization shown in the statusline. The transcript and in-memory history are cleared, but the ContextWindow tracker isn't refreshed until the next model event (e.g., TurnEnd/RunEnd), so the statusline continues to show the pre-clear percentage until after the next response.
  - Commit: 2f41ddba651ee26124ef18fead3434dbd12ddc19
  - File: caliban/src/tui/slash/basic.rs
  - Lines: 25–33 (ClearCommand::execute)
  - Suggested fix: after clearing transcript/messages/session and TTFT, also reset the context tracker so the statusline updates immediately.
    - Add: ctx.app.context_window.record_history(&[]);
    - Placement: just before returning SlashOutcome::Continue.

- Finding [PARTIALLY RESOLVED — Stage A/B/C recovery shipped in #60; loop double-count fixed in #68]: when a turn ends with `stop_reason: MaxTokens` and no `ToolUse` block, the loop silently halts via the catch-all `EndOfTurn` branch. Reasoning-heavy models (e.g. gpt-5) can burn the entire `max_tokens` budget on internal/reasoning tokens, emit a single empty AssistantTextDelta, and exit the run with zero visible output and no next-turn signal. Reproduced 2026-05-26 with gpt-5-2025-08-07 at the default 1024-token cap: TurnEnd { stop_reason: MaxTokens, output_tokens: 2048, tool_results: [] } → agent went idle. Claude Code's leaked source handles this with a two-stage recovery (one-shot budget escalation, then meta-message continuation up to a cap). **Update 2026-05-27:** this two-stage recovery was already shipped in PR #60 (the "caliban currently has neither" claim was stale). PR #68 found it was over-firing — `max_tokens_recovery` defaulted to `true` and Stage A's re-issue re-emitted `TurnEnd`, inflating the turn count past the cap — so #68 disabled recovery by default and added an explicit `StopReason::MaxTokens` halt + `StopCondition::MaxTokensExhausted`. **Remaining work:** fix Stage A's `TurnEnd` double-count so recovery can be safely re-enabled (split attempt-end vs turn-end semantics), and add a CLI flag to opt back in.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-agent-core/src/stream/mod.rs
  - Lines: 903–908 (post-turn continue/halt decision); also 170–183 (StopCondition enum) and AgentConfig.max_tokens default (1024).
  - Suggested fix: split the non-`ToolUse` branch so `MaxTokens` is handled distinctly from `EndTurn`/`StopSequence`/`ContentFilter`/`Refusal`, and add at least Stage A recovery.
    - Stage A (minimum): if `turn_stop_reason == MaxTokens` and no override is in effect, re-issue the same request once with an escalated `max_tokens` (e.g. `ESCALATED_MAX_TOKENS = 16_384`). Gate with a per-turn flag so it fires at most once; on the second hit fall through to halt.
    - Stage B (optional, follow-up): inject a meta user message ("Output token limit hit. Resume directly — no apology, no recap. Pick up mid-thought. Break remaining work into smaller pieces.") and continue the run, capped by a `max_tokens_recovery_count` (suggest 3).
    - Stage C: add `StopCondition::MaxTokensExhausted` variant at 170–183 and emit it on final surrender so the TUI/statusline can distinguish a clean end-of-turn from a budget-blowout. Currently both report `EndOfTurn`, which is what made the hang invisible.
    - Plumbing: thread an escalation/recovery counter into the inner-loop state alongside `turn_index`; do not store on `AgentConfig` (per-run, not per-agent).

- Finding: no stream-idle watchdog. The provider stream loops (`while let Some(item) = sse.next().await`) will sit forever if the upstream TCP connection silently dies mid-response — the underlying HTTP client's request timeout only covers the initial `fetch`, not the streaming body. Result: a dropped SSE connection hangs the session indefinitely with no error and no recovery. Claude Code wraps its stream loop with a per-chunk idle timer (default 90s, half-time warning) that aborts the stream and falls back to a non-streaming retry. Pattern: `claude.ts:1874–1928` (`resetStreamIdleTimer` on every chunk; on timeout set `streamIdleAborted = true` and release resources; outer code at 2308–2330 falls back to non-streaming).
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-provider/src/stream.rs (and the per-provider `stream_parse.rs` consumers); `caliban-provider-openai/src/stream_parse.rs:63,381,583,978` and `caliban-provider-anthropic/src/stream_parse.rs` all have bare `while let Some(item) = ... .next().await` loops.
  - Suggested fix: wrap chunk reads in `tokio::time::timeout`; on first timeout emit a warning event, on second timeout abort the stream and surface `StopCondition::ProviderError("stream idle")`. Make the timeout env-configurable (`CALIBAN_STREAM_IDLE_TIMEOUT_MS`, default 90_000) and the warning fraction (default half). Bonus: thread a non-streaming retry path through `Provider::stream` callers — on idle-abort, retry once with the streaming flag off before surrendering.
    - Placement: a small wrapper helper in `caliban-provider/src/stream.rs` that turns a `MessageStream` into a watched stream; have each provider's `stream_parse` consume via the wrapper rather than calling `.next()` directly.
    - Telemetry: emit a structured tracing event on warning + timeout so the debug log (`~/Library/Caches/caliban/debug.log`) makes the abort obvious in post-mortems.

- Finding: no "tokens stalled" UI signal. The TUI shows the same pre/streaming state whether tokens are flowing or the model has gone quiet for 30s. Users can't tell "still working" from "stuck waiting on the network" without checking the log. Claude Code transitions the spinner color toward red after 3s of no token deltas (suppressed when tools are actively running), fading in smoothly so it doesn't startle on every micro-gap. Pattern: `src/components/Spinner/useStalledAnimation.ts` — drives intensity from elapsed time since last delta, gates on `hasActiveTools`, smooths with a per-frame easing.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: caliban/src/tui/render.rs (status/spinner area) and caliban/src/tui/events.rs (delta event handling).
  - Suggested fix: track `last_delta_at: Instant` in the TUI app state; update on every `AssistantTextDelta` / `ToolCallInputDelta`. In the render tick, compute `idle = now - last_delta_at` and after `>3s` (and no in-flight tool dispatch) render the spinner glyph or label in a dimmer / warmer color. Reset when a delta arrives or a tool starts.
    - Placement: state on `tui::app::App`; render branch in the existing spinner cell.
    - Cheap variant: just change the spinner verb/suffix (e.g. "Thinking… (no tokens for 12s)") without the color animation. Lower lift, still answers the user question.

- Finding: refusals halt silently. The provider crates parse `StopReason::Refusal` (at `crates/caliban-provider/src/response.rs:40` and per-provider `ir_convert.rs:190`, etc.), but the agent loop treats refusal the same as `EndTurn` via the catch-all branch at stream/mod.rs:903–908. No user-facing explanation is rendered; the run just ends and the TUI shows nothing distinguishing it from a normal completion. Claude Code yields a synthetic assistant message before halting: `claude.ts:2258–2264` calls `getErrorMessageIfRefusal(stop_reason, model)` and yields it into the message stream.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-agent-core/src/stream/mod.rs (post-turn branch) and crates/caliban-provider/src/response.rs (StopReason).
  - Suggested fix: in the same split of the non-`ToolUse` branch proposed for MaxTokens, handle `StopReason::Refusal` by (a) emitting a synthetic assistant message into the transcript with a model-aware explanation, and (b) emitting `StopCondition::Refusal(String)` as a new variant on `StopCondition` (alongside `MaxTokensExhausted` from the prior TODO). Also add for `ContentFilter`, which has the same "halts invisibly" problem today.
    - Placement: synthetic message construction near the turn-end yield; new `StopCondition` variants at stream/mod.rs:170–183.
    - Caveat: the message text should be terse and not editorialize ("Model declined to respond.") — Claude Code's helper keys off the model so 3P providers don't get Anthropic-flavored copy.

- Finding: no reactive compaction on prompt-too-long (HTTP 413 / context-window-exceeded). caliban's `Compactor` trait (compact.rs: NoopCompactor, DropOldestCompactor, SummarizingCompactor) only runs proactively before a request — there's no path that triggers compaction *in response to* a 413 from the provider and retries. A run that overshoots the model's context window today fails to `StopCondition::ProviderError(...)` with no recovery. Claude Code splits this into two layered recoveries: (1) `contextCollapse` drains pre-staged collapses cheaply, (2) `reactiveCompact` does a full summary on the failing turn's messages and retries once. Pattern: `query.ts:1062–1175` (`isWithheld413` branch → collapse drain → reactive compact → surface).
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-agent-core/src/compact.rs and the request-failure path in crates/caliban-agent-core/src/stream/mod.rs; provider error mapping in each `caliban-provider-*/src/error.rs`.
  - Suggested fix: define a `ProviderError::PromptTooLong` variant (vs the generic `ProviderError(String)`) in the provider error type so the agent loop can detect it; on encountering it, call the configured `Compactor::compact` against the current messages and re-issue the request once. Cap with `hasAttemptedReactiveCompact` flag per run so a still-413ing retry surrenders rather than spinning. Independent of this, add a one-shot per-turn "withheld error" pattern so the loop holds the 413 until recovery exhausts (mirrors Claude Code's `isWithheldPromptTooLong`).
    - Placement: error mapping near each provider's `error.rs::From` impls; recovery branch at stream/mod.rs alongside the proposed MaxTokens stage. Keep the existing `Compactor` trait untouched — reuse the same strategy for proactive and reactive triggers.
    - Optional follow-up: staged `contextCollapse`-style incremental drops (cheaper than a full summary) as a second `Compactor` strategy; let the agent try collapse first, summary second.

- Finding: no death-spiral protection on `after_run` / `after_turn` hooks when the last message is a provider error. caliban's hook trait (hooks.rs:326,337) runs `after_run` / `after_turn` unconditionally regardless of why the turn ended. A user-installed hook that mutates session state (e.g. appends a "check your work" prompt, kicks off a tool retry, posts to a status endpoint) can re-enter the loop with the same error condition and spiral: error → hook injects input → retry → same error → hook fires again. Claude Code short-circuits at `query.ts:1262–1265` with `if (lastMessage?.isApiErrorMessage) { void executeStopFailureHooks(...); return }` — stop hooks are skipped and a separate failure-hook path runs instead.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-agent-core/src/hooks.rs (trait shape) and crates/caliban-agent-core/src/stream/mod.rs:911–933 (after_run invocation site).
  - Suggested fix: gate the `after_turn` / `after_run` invocations on the outcome — if `stopped_for` is `ProviderError(_)` (or future `MaxTokensExhausted` / `Refusal`), skip the normal hook and instead invoke a sibling `after_run_failure(&self, ctx, &outcome)` method (default impl: noop). Hook implementors opt in to failure handling explicitly and can't accidentally drive a spiral from their `after_run`.
    - Placement: add `after_run_failure` / `after_turn_failure` defaults in hooks.rs alongside the existing methods; swap the call sites in stream/mod.rs to dispatch on `stopped_for` variant.
    - Note: the existing `RunHookOutcome.stopped_for` already carries the info needed to switch; no new plumbing required.

- Finding: `after_turn` hook can't redirect the run. Trait signature returns `Result<()>` (hooks.rs:337), so a hook that detects "agent gave up early without finishing the user's request" has no way to inject a continuation message or force another turn. Claude Code's stop-hook system can request continuation, which powers patterns like "if the model returned without writing files, prompt it to actually write them" — currently impossible in caliban without forking the loop.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-agent-core/src/hooks.rs:337 (after_turn signature); decision point at stream/mod.rs:903–908.
  - Suggested fix: change `after_turn` to return `Result<TurnDecision>` where `TurnDecision` is `{ Continue, ContinueWith(Vec<Message>), Stop }`. Default impl returns `Continue`. In stream/mod.rs, after the existing continue/halt decision but before `break 'outer`, call `after_turn`; if it returns `ContinueWith(msgs)`, append them and loop. Gate with a per-run counter (max ~3 forced continuations) to prevent runaway hooks.
    - Placement: trait change in hooks.rs; dispatch hook in stream/mod.rs after line 908 but inside the outer loop so a `ContinueWith` re-enters cleanly.
    - Migration: this is a breaking change to the `Hooks` trait. Either bump the trait version or add a separate `redirect_turn` method with a default `Continue` impl, leaving `after_turn` purely observational. Latter is less disruptive.

- Finding: cost tracking exists but isn't exposed to the user. `caliban-telemetry/src/cost.rs:270` has a `price(provider, model, usage, as_of)` priced ledger and `metrics.rs:183` emits cost as a metric, but nothing in `caliban/src/tui/` reads or renders it — `grep cost caliban/src/tui/` returns no hits. Users can't see session cost in the statusline or via a slash command. Claude Code surfaces this two ways: a `/cost` slash that prints a per-model breakdown (`src/commands/cost/cost.ts`) and an on-exit summary (`src/costHook.ts` writes `formatTotalCost()` to stdout when the process exits if billing access is enabled). Both rebuild from the same `cost-tracker.ts` totals.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: caliban/src/tui/slash/ (new `cost.rs`); caliban/src/tui/app.rs (statusline); read from crates/caliban-telemetry/src/cost.rs (existing ledger).
  - Suggested fix: add a `/cost` slash command that reads the session's cumulative USD and per-model breakdown out of the telemetry ledger and renders it in an overlay (similar shape to the existing `/perms` overlay). Optional: render the running total in a corner of the statusline (small font, formatted `$0.0123`). On session end, emit a one-line summary to the transcript.
    - Placement: new `tui/slash/cost.rs` following the existing slash module pattern; wire it into the slash dispatcher next to `perms`/`session`/`config`.
    - Caveat: the cost ledger is currently a per-process structure; persisting cumulative cost across resumes needs the cost state to round-trip through `caliban-sessions` (Claude Code does this via `saveCurrentSessionCosts`).

- Finding: no `/doctor` command for environment / config / provider-health diagnosis. When something is mis-configured (wrong API key, MCP server failing to start, sandbox detection wrong, model not available), the user finds out by running a real turn and getting an opaque error. Claude Code's `/doctor` (`src/commands/doctor/index.ts`) is a one-shot check that runs auth, settings, IDE/MCP, and sandbox probes and reports each as pass/warn/fail with remediation hints — much faster feedback than failing on a real prompt.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: caliban/src/tui/slash/ (new `doctor.rs`); reads from crates/caliban-settings, crates/caliban-mcp-client/src/manager.rs (server health), crates/caliban-sandbox/src/detect.rs (sandbox detection), and each `caliban-provider-*` for cheap auth pings.
  - Suggested fix: add `/doctor` that runs a checklist: (a) settings load + parse, (b) configured providers — auth ping each, (c) MCP servers — connect + list_tools to each, (d) sandbox detection result, (e) checkpoint store writability, (f) session store writability. Render results as a table of name / status / hint, with non-zero exit (when run via `--print`) so CI can use it.
    - Placement: `tui/slash/doctor.rs` for the interactive version; expose the same checks behind a `caliban doctor` subcommand in `caliban/src/main.rs` so it works headless. (The binary crate lives at `caliban/` at the workspace root, not under `crates/`.)
    - Cheap MVP: skip the provider pings (which cost a request) and just verify config completeness + MCP connect; add real auth pings behind a `--deep` flag.

- Finding: `Compactor` strategies only run when the caller explicitly invokes them — no auto-trigger on context-window pressure. caliban-agent-core/src/compact.rs defines `NoopCompactor`, `DropOldestCompactor`, `SummarizingCompactor` and a `Compactor::compact` trait method, but stream/mod.rs's turn loop has no pre-turn check that says "tokens used > N% of model context → compact before sending the next request." Result: long sessions silently approach the context limit and 413 (which is itself unhandled — see the reactive-compaction TODO above). Claude Code threads autocompact through every turn at `query.ts:453–518` with a tracking state, threshold check, and `consecutiveFailures` counter that disables further attempts after repeated failures.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-agent-core/src/stream/mod.rs (pre-turn hook); crates/caliban-agent-core/src/compact.rs (existing strategies); crates/caliban-agent-core/src/cache.rs (token-usage source).
  - Suggested fix: before each turn in the inner loop, compute current tokens-in-history vs the resolved model's context window. If the ratio crosses a configurable threshold (suggest 0.75), call the configured `Compactor::compact`. Track `AutoCompactTracking { last_attempt_turn, consecutive_failures }` on the per-run state; disable autocompact for the rest of the run after `MAX_CONSECUTIVE_FAILURES` (suggest 2) so a broken compactor can't loop.
    - Placement: small `auto_compact()` helper invoked at the top of each turn iteration before the request is built, alongside the existing `before_turn` hook dispatch.
    - Config: `AgentConfig.auto_compact_threshold: Option<f32>` (None = disabled, Some(0.75) = trigger at 75%). Bonus: expose via `CALIBAN_AUTO_COMPACT_THRESHOLD` env var.

- Finding: no "microcompact" — sub-message-level trimming that runs every turn and is cheap because it operates by `tool_use_id` (no LLM call required). Long sessions accumulate stale tool results (file reads of files that have since been edited, grep dumps that have been re-run, etc.) that take space in the prompt despite being superseded. Claude Code runs microcompact on every turn at `query.ts:412–426`, separately from autocompact, and the two compose: snip → microcompact → autocompact. Today caliban only has the autocompact-style summarizer, which is heavy.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-agent-core/src/compact.rs (new `Compactor` strategy or sibling trait); invoked from crates/caliban-agent-core/src/stream/mod.rs before each turn.
  - Suggested fix: add a `MicroCompactor` strategy that walks the message history and, for each `ToolResult` block whose `tool_use_id`'s output has been superseded (same tool, same target, more recent invocation), replaces the older block with a one-line placeholder like `[superseded: <tool>(<key>)]`. Run on every turn. Cheap because no LLM is involved and decisions are local. Track tokens-freed and surface in telemetry.
    - Placement: separate `MicroCompactor` rather than overloading the `Compactor` trait — they have different invocation cadences. Or extend `Compactor` with a `micro_compact(&self, messages)` default-noop method.
    - Caveat: needs a per-tool "supersession" predicate. Start with `Read` (same path → newer wins), `Grep` (same args → newer wins), `Bash` (no supersession — different runs may matter). Extend as patterns emerge.

- Finding: no system-wide tool-result size cap with auto-persist-to-disk + preview. Each caliban built-in tool hardcodes its own truncation (`shell/bash.rs:STDOUT_CAP/STDERR_CAP`, `fs/read.rs:MAX_FILE_BYTES=5MB`, `web/web_fetch.rs:100KB`, `agent/agent_tool.rs:5K`, `search/grep.rs:max_matches`). Means: (a) limits are inconsistent and surprising, (b) when a tool's output is large but under-cap, it still bloats the prompt across many turns, (c) third-party / MCP tools have no cap at all. Claude Code's `constants/toolLimits.ts` sets `DEFAULT_MAX_RESULT_SIZE_CHARS = 50_000` system-wide, persists oversized results to disk, and replaces them in the message with a preview + file path that the model can re-read on demand.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-agent-core/src/post_process.rs (post-tool result rewriting — existing module is a natural seam); crates/caliban-agent-core/src/stream/parallel.rs (where tool results are accumulated); per-tool caps in crates/caliban-tools-builtin/src/.
  - Suggested fix: add a post-process pass after tool dispatch that checks each `ToolResult` block size against a global cap (`DEFAULT_MAX_RESULT_SIZE_CHARS = 50_000`). If exceeded: write the full content to a temp file under `~/Library/Caches/caliban/tool-overflows/<session>/<tool_use_id>.txt`, replace the block content with `[truncated: <N> chars, full content at <path>; head 2KB / tail 2KB shown below]` plus the head/tail. The Read tool already handles re-reading by path, so the model can follow up. Apply to ALL tool results including MCP — keeps the cap uniform.
    - Placement: a `cap_tool_results()` pass in `post_process.rs` called by `stream/parallel.rs` after gathering the parallel-dispatch batch.
    - Per-tool overrides: keep the existing per-tool caps as soft hints (`maxResultSizeChars` field on the tool descriptor), but enforce the global cap as the hard ceiling.

- Finding: no `/effort` command to adjust reasoning effort mid-session. The MaxTokens hang in the prior TODO came from gpt-5 burning the budget on reasoning tokens. Lowering effort would have avoided it, but caliban exposes no runtime knob — effort would need a config-file edit + restart. Claude Code has `/effort [low|medium|high|max|auto]` (src/commands/effort/index.ts) that updates the inference config in-place, takes effect on the next turn, and is `immediate: true` so it bypasses the agent loop.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: caliban/src/tui/slash/ (new `effort.rs`); reads/writes effort on the active `Agent`'s shared config; per-provider effort knob in crates/caliban-provider-openai/src/ir_convert.rs and similar.
  - Suggested fix: add an `Effort` enum (`Low`/`Medium`/`High`/`Max`/`Auto`) on `AgentConfig`, plumb it into the per-provider request build (OpenAI: `reasoning.effort`; Anthropic: `thinking.budget_tokens` derived value; others: noop). Add `/effort <level>` slash command that updates the value via the existing `SharedPermissionMode`-style `ArcSwap` pattern (see permission_mode.rs:131 — same lock-free shape). Show current level in the statusline when running on a reasoning-family model.
    - Placement: enum in `agent_core::config`; slash handler in `tui/slash/effort.rs`; per-provider plumbing in each `caliban-provider-*/src/ir_convert.rs`.
    - Coupling: pair with the `MaxTokensExhausted` recovery TODO — if recovery fires once, suggest `/effort low` in the surfacing message so the user has a one-keystroke remediation.

- Finding: no `/resume` slash command. caliban-sessions persists session state and `caliban-checkpoint` powers `/rewind`, but there's no in-TUI flow to pick a prior session and resume it. Users have to either remember the session ID and pass it on the CLI, or restart caliban with `--resume <id>`. Claude Code's `/resume` (src/commands/resume/index.ts, aliases `continue`) opens an interactive picker that searches across past sessions by title/content and resumes the selected one without restarting the process.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: caliban/src/tui/slash/ (new `resume.rs`); reads from crates/caliban-sessions/src/store.rs (existing list/load); reuses crates/caliban-checkpoint for working-tree restoration if applicable.
  - Suggested fix: `/resume [query]` opens an overlay listing recent sessions (last-modified desc, ~20 visible, scrollable), with fuzzy filter against session title + first user message. Selecting one swaps the live session state in-place — message history, ContextWindow tracker, cost ledger, plan/permission mode — without restarting. If a checkpoint is associated with the selected session, optionally prompt before restoring the working tree.
    - Placement: overlay in `tui/slash/resume.rs`; session swap helper on `tui::app::App`; reuse the existing session-load path that `--resume` uses on startup.
    - UX: aliases `/continue`; argument hint `[query]` matches the existing slash conventions.

- Finding: no `/context` visualization. The statusline shows a single percentage (when the ContextWindow tracker is up to date — see the first TODO), but there's no way for the user to see *what* is filling the window. A long session might have a single 80KB stale tool result hogging space, but the user can't tell. Claude Code's `/context` (src/commands/context/index.ts) renders a colored grid where each cell is a message and color encodes type (system / tool result / assistant / user), giving an at-a-glance view of context composition.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: caliban/src/tui/slash/ (new `context.rs`); reads from the active session's message history + crates/caliban-agent-core/src/cache.rs for the token counter.
  - Suggested fix: `/context` opens an overlay with two views: (a) a horizontal stacked bar broken down by message-type (system / user / assistant / tool_use / tool_result), labeled with absolute token counts and percentages; (b) a vertical list of the top-N largest blocks (descending tokens) so the user can spot a single fat tool result. Update live as new turns arrive.
    - Placement: overlay in `tui/slash/context.rs`; token estimation reuses whatever the autocompact-threshold check uses (see autocompact TODO above).
    - Non-interactive variant: a `--print` mode that emits a one-line "73%: 12K sys / 4K user / 31K asst / 53K tool_result" so `caliban context` is useful in scripts.

- Finding: no `/export` slash command for the current transcript. To share or archive a session, users have to dig into `~/Library/Application Support/caliban/sessions/<id>/` and decode the on-disk format manually. Claude Code's `/export [filename]` (src/commands/export/index.ts) writes the conversation to a file or copies it to the clipboard in a clean human-readable format.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: caliban/src/tui/slash/ (new `export.rs`); reads from the live session message history.
  - Suggested fix: `/export [path]` writes the session to a markdown file (default `caliban-session-<YYYY-MM-DD>-<short-id>.md`) with sections per message, code-fenced tool calls/results, and a header containing model/cost/duration. If `path` is `-` or omitted with a TTY pasteboard available, copy to clipboard instead. Strip cache_control / internal IDs from the output.
    - Placement: `tui/slash/export.rs`; reuse the on-disk session serializer's `to_markdown` if it exists, else write a minimal renderer here.
    - Extension: support `--format json` for machine-readable export; useful for piping into evals.

- Finding: permission modal has no "always allow this" runtime option. caliban's PermissionMode enum (permission_mode.rs:17–35) provides session-wide modes (Default/AcceptEdits/Plan/Auto/DontAsk/BypassPermissions), but during a normal `Ask` flow the user can only choose accept/reject for this *one* invocation. To turn "always allow `gh pr view`" into a persistent rule the user must Ctrl-C and edit the rules config. Claude Code's permission modal offers Accept / Reject / Always-allow / Always-reject for the specific tool+args pattern, written into the session's runtime ruleset.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-agent-core/src/permissions.rs (rule store / runtime additions); caliban/src/tui/ask.rs (the modal — currently 2-option accept/reject).
  - Suggested fix: extend the modal to 4 options: Allow once / Always allow / Reject once / Always reject. The "Always" variants add a rule to a per-session in-memory ruleset (and optionally write to the project rules file with an extra confirmation). The pattern is the tool name + a derived signature from the arguments (e.g. for Bash: the first token of the command; for Edit: the file path's first path-segment). Show the derived pattern in the modal so the user knows what they're allowing.
    - Placement: extend the existing modal state machine in `tui/ask.rs`; rule insertion helper in `agent_core::permissions`.
    - UX safety: the "Always reject" path is more important than the allow side — lets a user one-key block a runaway tool without killing the session.

- Finding: no custom statusline support. The TUI's statusline is hardcoded; there's no way for the user (or a plugin) to add a project-specific status line (branch, dirty count, build status, k8s context, etc.) without editing the Rust source. Claude Code's `settings.statusLine` accepts a command path; the harness runs it after each turn and renders stdout as the statusline, giving full project-specific customization for free.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-settings/src/ (new `statusline.rs` config field); caliban/src/tui/render.rs (statusline render path); invocation after `TurnEnd` event.
  - Suggested fix: add `settings.statusLine: { command: string, timeout_ms?: u32 }`. After each `TurnEnd`/`RunEnd` (and at session start), spawn the command with the workspace as cwd, capture stdout (cap at one line, ~120 chars), and render it as a prefix/suffix to the existing statusline. Cache between turns so it doesn't run mid-render. Default timeout 200ms; on timeout, render the previous output and log.
    - Placement: settings schema in `caliban-settings`; spawn-and-cache in a small helper alongside the statusline renderer; invocation tied to the existing event stream.
    - Compat note: claude-code passes a JSON blob (model / cost / mode) to the command on stdin so the script can branch; worth mirroring that contract from day one so existing claude-code statusline scripts work in caliban.

- Finding: prompt cache markers are placed on system + tools only — not on conversation messages. `crates/caliban-agent-core/src/cache.rs:18` marks the last system `TextBlock` and the last `Tool`, which caches the *static* prefix (system + tools). But on a 20-turn session, the growing message history is *not* cached: every turn re-pays the full input-token cost of every prior message. Anthropic supports a per-message `cache_control` that caches the prefix up to that message; placing one marker on the last user message before the new prompt means turn N+1 re-uses the cached prefix from turn N at the cache discount. Claude Code does exactly this — `claude.ts:3078,3167–3181` enforces "exactly one message-level cache_control marker per request" with placement on the last message that contains one. Real-session evidence: the debug log entries for turn 7 → 8 → 9 show `cache_read_input_tokens` hovering at ~106K but never climbing as the session adds messages — caliban is re-paying for everything past the system prefix.
  - Commit: ea8a56bd665a16d8a47ea5e858b6647187c7f3dc
  - File: crates/caliban-agent-core/src/cache.rs:18 (`apply_prompt_cache` — currently only handles system + tools).
  - Suggested fix: extend `apply_prompt_cache` to also place a single `cache_control: Ephemeral` marker on the last block of the last user message in the conversation (after the static-prefix markers). Anthropic permits up to 4 cache breakpoints per request — adding one for the conversation is well within budget and gives a turn-over-turn cache benefit. Skip the marker if the message is < ~1K tokens (caching tiny prefixes is wasted overhead).
    - Placement: extend the existing function rather than splitting — keep all cache-marker logic in one place. Add a `min_cache_block_tokens: usize` const (suggest 1024) to gate the conversation marker.
    - Validation: add a test that runs `apply_prompt_cache` on a multi-turn message vec and asserts exactly one marker on the last user message in addition to the system + tool markers.
    - Telemetry: existing `caliban::cache: prompt cache stats cache_read=...` log will show the improvement immediately — expect `cache_read` to grow with conversation length instead of staying flat at the system-prefix size.

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
  - File: crates/caliban-agent-core/src/permissions.rs (default-rules tail); headless dispatch path in caliban/src/headless/mod.rs
  - Severity: Medium — a whole tool class fails silently in the headline headless mode.
  - Suggested fix: decide the intended headless default. Either (a) emit a clear error ("tool X requires --auto-allow or an --allow rule in headless mode") instead of an opaque deny, plus document the requirement loudly; or (b) make headless `-p` default to a more permissive mode for workspace-scoped mutating tools. Pairs with F15's `permission_mode` surfacing (now in `system/init` per #72) so the failure is at least diagnosable.
