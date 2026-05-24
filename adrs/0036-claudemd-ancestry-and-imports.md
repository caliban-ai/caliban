# ADR 0036 · CLAUDE.md ancestor walk + `@`-imports

- **Status:** proposed
- **Date:** 2026-05-24
- **Author:** john.ford2002@gmail.com
- **Spec:** `docs/superpowers/specs/2026-05-24-claudemd-ancestry-design.md`

## Context

`caliban-memory`'s project tier currently loads exactly one file —
`<workspace_root>/CLAUDE.md`. Claude Code instead walks from cwd
upward, concatenating every `CLAUDE.md` (and `AGENTS.md` and
`.caliban.md`) it finds, supports `@path/to/file` imports inside any
of them (bounded recursion + approval for external paths), loads
nested children on demand as the model reads into subdirectories, and
honors `.claude/rules/<topic>.md` files with optional `paths:` glob
frontmatter for scoped activation. The matrix marks this row 🟡
because the single-file loader exists but lacks every other behavior.

We need parity to make caliban usable in monorepos, in deeply-nested
project layouts, and in any workflow where contributors share
CLAUDE.md fragments via imports.

## Decision

### Five behaviors, one orchestrator

The new project tier in `caliban-memory` orchestrates five distinct
concerns:

1. **Ancestor walk** — start at cwd, walk up to git root (or fs root,
   configurable via `WalkStop`), concatenate every CLAUDE.md /
   AGENTS.md / `.caliban.md` in broad → narrow order.
2. **`@`-imports** — recursion-bounded (depth ≤5), cycle-detected by
   canonical path, with an approval dialog for first-time external
   imports persisted to `~/.caliban/imports-allowlist.json`.
3. **Nested-on-demand** — `Read`/`Edit`/`Glob` success notifies an
   `AncestryAddendum` which appends any newly-touched directory's
   CLAUDE.md to the system prompt for the rest of the session.
4. **`.caliban/rules/<topic>.md`** — path-glob-scoped rules with a
   `RulesActivator` that lights them up on first matching path touch.
5. **`claude_md_excludes`** — gitignore-style patterns scoped to the
   workspace root, evaluated during walk.

All five share the existing `MemoryPrefix` machinery; `project` slot
becomes a richer `ProjectTier` struct containing four `Vec<TierFile>`
collections (base / imports / rules / nested) instead of one
`TierFile`.

### Three filenames, no precedence battles

`CLAUDE.md`, `AGENTS.md`, and `.caliban.md` are all loaded when present
in the same directory. Within a directory we load
`.caliban.md` → `CLAUDE.md` → `AGENTS.md` (most-specific → most-general).
We do not surface "which file overrode which" because they don't
override — they concatenate. Operators who need exclusion use
`claude_md_excludes`.

### `@`-import semantics align with Claude Code, minus HTTP

Local paths only. `@./foo.md`, `@~/notes/x.md`, `@/abs/path.md` all
work; `@http(s)://…` is rejected outright. This keeps imports
auditable (a static set of filesystem paths) and avoids embedding an
HTTP fetcher inside the prompt-assembly path.

External imports (those outside the workspace root and outside
`~/.config/caliban/`) require approval. The dialog persists decisions;
non-interactive callers (`--print`, CI, `--bare`) deny by default but
respect `CALIBAN_APPROVE_IMPORTS=1` for unattended runs.

### Nested-on-demand is one-shot per (path, session)

Once the model `Read`s a file and we load that directory's CLAUDE.md,
we keep it for the rest of the session. We do not detect file changes
and reload, we do not unload when the model leaves the subtree. This
keeps the system prompt monotone (only grows), which matches how
operators reason about it.

### Rules use `globset`, the workspace's existing glob crate

`globset` is already a workspace dep. Rules build a single
`GlobSet` at startup; path-touch hooks ask "does this path match any
unactivated rule?" — O(1). Rules without a `paths:` frontmatter are
always-active (loaded at startup, before any path touch).

### `claude_md_excludes` is gitignore-style with explicit semantics

We adopt the gitignore matching semantics (`!` negation, last-match
wins for a given path). Patterns are evaluated **relative to the
workspace root**, not to the absolute filesystem path — operators
write `node_modules/**`, not `/Users/foo/proj/node_modules/**`. The
workspace root is the start of the ancestor walk (the cwd at startup).

### `--add-dir` paths contribute CLAUDE.md only opt-in

Adding a directory to the agent's accessible-paths set should not
silently inject another CLAUDE.md into the prompt. Operators who want
that behavior set `CALIBAN_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1`. Each
`--add-dir` then performs its own ancestor walk, concatenated after
the cwd walk in declaration order.

### Regression escape: `CALIBAN_DISABLE_CLAUDE_MD_WALK=1`

If the new loader misbehaves in a real-world repo we don't have CI
coverage for, operators set this env to fall back to the legacy
single-file project tier. This is a maintenance lifeline; we expect
it to be unused in steady state.

## Consequences

- **Positive:** Closes three 🟡 / 🔴 rows under C. Memory &
  checkpointing in one PR. Caliban becomes deployable in monorepos
  without prompt-injection workarounds. `@`-imports unlock content
  sharing between repos (a single `~/notes/api-conventions.md` can be
  imported from every project's CLAUDE.md). Rules let
  language/framework-specific guidance be scoped to where it applies
  instead of polluting the top-level CLAUDE.md.
- **Negative:** Project-tier complexity goes up materially — five
  concerns sharing one orchestrator. The approval-dialog UX adds a new
  modal flow the TUI must handle. The system prompt grows
  monotonically during a session, which interacts with the existing
  memory budget enforcement (truncation logic now runs against a
  larger surface). Operator authoring of `claude_md_excludes`
  gitignore patterns is a known footgun (test #18 covers the
  common case).
- **Revisit if:** A real-world repo demands HTTP imports — we'd
  revisit the security model (signed manifests? lockfile?). If the
  approval dialog frequency proves annoying in practice, add
  `[memory] auto_approve_under = ["~/dev/personal/**"]`. If the
  monotone-prompt-growth interacts badly with long sessions, add a
  rule-level "deactivate after N turns since last match" knob.
