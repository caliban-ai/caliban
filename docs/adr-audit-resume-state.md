# ADR audit — resume state

> Working notes for picking up the ADR conformance audit later.
> Full findings live in [`2026-05-25-adr-conformance-audit.md`](2026-05-25-adr-conformance-audit.md).

## Where we are

- **Branch:** `jf/docs/adr-conformance-audit` (off `main`, rebased 2026-05-26).
- **Audit doc:** `docs/2026-05-25-adr-conformance-audit.md` (579 lines, untracked, ready to commit).
- **This file:** untracked planning notes; commit if you want them in-tree.
- **Status:** read-and-write pass is complete; nothing has been committed; no code changes proposed yet.

## Coverage

- All 40 ADRs (0001-0040) were read in full.
- Conformance was spot-checked by inspecting the workspace
  (`crates/`, `caliban/src/`, hook trait methods, MCP modules,
  settings/router/checkpoint/plugins/worktrees/supervisor/telemetry
  layouts) and verifying specific commitments (env var names, file
  paths, module names, the four built-in output styles, etc.).
- Technical merit was judged per-ADR; none rated "questionable."

## Headline findings (mirror of TL;DR in the audit)

1. **Status drift.** 21 of 40 ADRs are misstated in
   `adrs/README.md` — 10 are `accepted` in body but `proposed` in
   index; another 11 say `proposed` everywhere but the parity matrix
   marks the rows ✅.
2. **Crate-boundary deviations without ADR updates.**
   - ADR 0027 commits to a new `caliban-tui-ask` crate; the Ask
     modal actually lives at `caliban/src/tui/ask.rs` (202 LOC).
   - ADR 0029 commits to a new `caliban-auto-mode` crate; the
     implementation lives inside `caliban-agent-core`
     (`auto_mode.rs`, `mode_filter.rs`, `permission_mode.rs`,
     ~1,750 LOC).
3. **Aging defaults.**
   - 8 KiB combined memory prefix (ADR 0018) is conservative against
     2026-era context windows; truncation-first drops auto-memory.
   - 5 MiB / 1568 px image cap (ADR 0039) is Anthropic-shaped only.
   - 256-entry per-session LRU for the auto-mode classifier (ADR
     0029) starts cold each run.
4. **TUI tick is a known shim, not a fix** (ADR 0014). Two years on,
   no follow-up ADR has closed it out.
5. **`is_parallel_safe()` deferred** (ADR 0016). Write surface has
   grown (Edit/Write/MultiEdit/NotebookEdit/WriteMemoryTopic) — same
   turn can collide.
6. **AGPL-3.0-only** (ADR 0003) increasingly bites the more
   library-shaped crates (`caliban-provider`, `caliban-mcp-client`,
   `caliban-model-router`, `caliban-images`, `caliban-telemetry`) we
   ship. No action; flag the choice.
7. **Missing ADRs.** No ADR for: (a) the `caliband` sibling-binary
   under `crates/caliban-supervisor/src/bin/`, (b) `arc-swap` as the
   shared-state primitive, (c) the `rmcp = "1.7"` pin specifically.

## Confirmed conformance highlights (don't re-verify these)

- All 24 declared crates exist; workspace `Cargo.toml` matches.
- AGPL-3.0 declared via `license.workspace = true` everywhere.
- All hook events from ADR 0024 are present on the `Hooks` trait
  (`session_start/end`, `user_prompt_submit`, `pre_compact/post_compact`,
  `config_change`, `cwd_changed`, `file_changed`, `subagent_start/stop`,
  `permission_request/denied`).
- MCP v2 phases A/B/C all shipped (`oauth.rs`, `elicitation.rs`,
  `resource.rs`, `Transport::Http`/`Sse` arms).
- Router v2 features all present (`fallback.rs`, `hedging.rs`,
  `breaker.rs`, `capabilities.rs`, `discovery.rs`, `effort.rs`).
- Checkpoint layout matches Claude Code; `CALIBAN_CHECKPOINT_ROOT`
  env override exists.
- Output styles: 4 built-ins present (`default/proactive/explanatory/learning.md`).
- Slash command registry and per-group impls present
  (`basic/observe/config/model/perms/session/dx/existing`).
- `ReadMemoryTopicTool` + `WriteMemoryTopicTool` in
  `caliban-tools-builtin`.

## Next steps (when resuming)

In rough priority order. Each is a separate small PR; none block each
other.

1. **Commit the audit + this file.** Suggested message:
   `docs: ADR conformance audit (2026-05-25)`.
2. **Reconcile ADR status drift** ([Finding 1](2026-05-25-adr-conformance-audit.md#finding-1-adr-status-drift)).
   - Update `adrs/README.md` status column from each ADR's body and
     the parity matrix.
   - Optionally add an "Implemented in" pointer to each ADR's
     References section.
   - Probably one PR.
3. **Decide on the two crate-boundary deviations** ([Finding 2](2026-05-25-adr-conformance-audit.md#finding-2-two-crate-boundaries-collapsed-without-an-adr-update)).
   - Option A: amend ADRs 0027 + 0029 with a "Revised" section
     explaining why the crate was collapsed (likely the right call
     for the Ask modal; arguably right for auto-mode too).
   - Option B: extract `caliban-tui-ask` and `caliban-auto-mode` for
     real.
   - Two ADR amendments, or one extraction PR per crate.
4. **Revisit the 8 KiB memory cap** ([Finding 3](2026-05-25-adr-conformance-audit.md#finding-3-aging-defaults-worth-revisiting)).
   Either bump the default to ~32 KiB or add a per-scope `cap_tokens`
   knob in `caliban-memory`. Touches `caliban-memory` + tests + a
   spec note.
5. **Close out the 50 ms TUI tick** ([Finding 4](2026-05-25-adr-conformance-audit.md#finding-4-tui-tick-papers-over-a-root-cause)).
   Use the debug log to confirm stalls are gone. Either remove the
   tick (cleanest) or write a short ADR documenting why it stays.
6. **Re-examine `is_parallel_safe()`** ([Finding 5](2026-05-25-adr-conformance-audit.md#finding-5-is_parallel_safe-deferral-is-reasonable-but-rotting)).
   Inventory which tools mutate filesystem state; decide whether to
   add a per-tool flag + per-path exclusion policy or accept the
   "rare collision" status quo. Probably warrants a small ADR.
7. **Write the missing ADRs** ([Finding 7](2026-05-25-adr-conformance-audit.md#finding-7--no-adr-for-the-bincli-split-itself)).
   Short ones — `caliband` binary placement, `arc-swap` choice,
   `rmcp` version pin. One paragraph each.

## What was NOT done

- No code changes. Audit is documentation-only by design.
- No commits made on this branch. Audit + this file are untracked.
- No verification of *runtime* behavior — conformance was checked
  against source structure, type names, module presence, and env-var
  strings, not against a running caliban.
- No comparison against external sources (Claude Code changelog,
  upstream Anthropic docs) — the parity matrix is the proxy for that.

