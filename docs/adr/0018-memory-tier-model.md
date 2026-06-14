# ADR 0018 · Memory tier model (CLAUDE.md ingestion + auto-memory)

- **Status:** accepted
- **Date:** 2026-05-23

## Context

caliban has no persistent memory across sessions. The default system
prompt is rebuilt from cwd + tool list each invocation (ADR 0014), so
operator preferences, project conventions, and learned facts about
the user have to be re-supplied by hand every time. Claude Code
solves this with a `CLAUDE.md` mechanism plus an auto-memory tier
the agent can write to via its existing file tools. The user's own
`~/.claude/CLAUDE.md` already exercises this pattern; that mental
model is the target.

## Decision

caliban adopts a **three-tier memory model**, all of which live on disk
as plain Markdown and are read at session start. A fourth MCP-mediated
tier slots in later (forward link only; not in this ADR).

### Tier 1 — Global

- Path: `~/.config/caliban/CLAUDE.md` (XDG `$XDG_CONFIG_HOME` honored).
- Owner: the operator. caliban never writes here.
- Contents: cross-project preferences (tool choice, style, persona).
- Read once at startup, optional (missing file is fine).

### Tier 2 — Project

- Path: `<workspace_root>/CLAUDE.md` where `workspace_root` is
  `WorkspaceRoot::root()` (ADR 0010).
- Owner: the project / repo. caliban never writes here (operators
  commit it like any other file).
- Contents: repo-specific conventions, build commands, taboos.
- Read once at startup, optional.

### Tier 3 — Auto-memory

- Directory: `~/.local/share/caliban/projects/<sanitized-cwd>/memory/`
  (XDG `$XDG_DATA_HOME` honored). Sanitization replaces `/` with `-`
  and drops the leading dash, so `/Users/jf/dev/caliban` becomes
  `Users-jf-dev-caliban`.
- Files: one `MEMORY.md` (index, ≤ 200 lines) plus arbitrary
  `<slug>.md` topic pages.
- Owner: **the agent.** Writes go through the existing `Write`/`Edit`
  tools — no special memory tool, no separate trust path.
- Only `MEMORY.md` is loaded eagerly. Topic pages are lazily fetched
  by the agent via `Read` when the index points it at one.

### Composition

All three tiers are concatenated into the system prompt **above** the
auto-generated default (cwd + tool list + conventions, per ADR 0014).
Order: global → project → auto-memory index. Each tier is wrapped in
explicit delimiters so the model can tell them apart:

```
<global-claude-md path="…/CLAUDE.md">…</global-claude-md>
<project-claude-md path="…/CLAUDE.md">…</project-claude-md>
<auto-memory-index path="…/MEMORY.md">…</auto-memory-index>

<default system prompt body from system_prompt::build_default …>
```

Missing tiers are simply omitted (no empty tag block).

### Token budget

The combined memory prefix is capped at **8 000 tokens** (estimated as
`chars / 4` — provider-agnostic and cheap). If the combined size
exceeds the cap, auto-memory is truncated first (with a
`[truncated: N bytes]` notice appended to its block), then project,
then global. Hitting the global cap is treated as operator error
(loud `tracing::warn!` plus the truncation marker in the prompt).

### Retrieval

**None in v1.** Memory IS the system prompt prefix. Semantic search
over memory (RAG) is a v2 concern and would slot in as a new tool
(`MemorySearch`), not as a change to how memory is loaded.

### Forward links

- **MCP memory tier.** Once MCP support ships, an MCP server like the
  user's SilverBullet integration plugs in as Tier 4: not eagerly
  loaded, accessed on demand via MCP tool calls. The
  precondition-check pattern from the user's own CLAUDE.md ("skip if
  MCP is absent") applies.
- **`/memory` slash command.** Shows active tiers + paths + sizes;
  offers `$EDITOR` open for the global and project files. Detailed
  in the spec.

## Consequences

- **Positive.** Matches the user's existing mental model exactly,
  zero learning curve. Agent maintains its own knowledge using the
  same Read/Write/Edit it already has — no special memory tool to
  audit, sandbox, or rate-limit. MCP tier slots in cleanly without
  reshaping the loader.
- **Negative.** 8K tokens is real cost on every turn (Anthropic
  prompt caching recoups most of it). Agent can clutter auto-memory
  if write conventions aren't well-specified (the spec pins them
  down). No drift detection between project CLAUDE.md and what the
  agent "remembers" — by design; the project file wins by splice
  order, but contradicting auto-memory will sit side by side.
- **Revisit if:** the 8K cap starts triggering routinely (raise it,
  or add summarization); auto-memory becomes a write-only graveyard
  (add a v2 `MemorySearch` tool and stop loading the full index);
  per-project agent memory grows past what's reasonable to grep
  (move to SQLite, but keep markdown export).

## Crate

New crate `caliban-memory` owns tier discovery, sanitization, file IO,
splicing, and budget enforcement. `caliban-agent-core` does **not**
take a dep on it — the binary (`caliban/src/main.rs`) calls the
memory crate at startup and passes the assembled string to
`system_prompt::resolve` as a prefix.

## Revised 2026-05-26

Bumped the combined-prefix default from 8,000 to **32,000 tokens**. The
8,000-token default was conservative against 2024 context windows and
was increasingly punishing in 2026 (1M-token Sonnet, 200K standard on
most providers). Truncation-first behavior was at risk of dropping the
auto-memory index — exactly the tier that grows.

Added per-scope token caps via three optional `[memory]` settings keys
(all integer, default unset):

- `cap_tokens_auto` — caps the auto-memory tier independently.
- `cap_tokens_claude_md` — caps the combined CLAUDE.md tier (global +
  project). When binding, truncates project first, then global.
- `cap_tokens_combined` — overrides the combined ceiling (`max_tokens`).

When the sum of both per-scope caps would exceed `cap_tokens_combined`,
each is scaled down proportionally rather than silently dropping a
tier. Settings.json values override the corresponding env vars
(`CALIBAN_MEMORY_BUDGET_TOKENS`, `CALIBAN_MEMORY_CAP_TOKENS_AUTO`,
`CALIBAN_MEMORY_CAP_TOKENS_CLAUDE_MD`) when both are present.

Truncation order within a tier is unchanged from the original Decision.
