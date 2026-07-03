# ADR 0048 · `--workspace` restricts file/shell tools by default

- **Status:** accepted
- **Date:** 2026-07-03
- **Source:** [`docs/superpowers/specs/2026-07-03-workspace-write-fence-design.md`](../superpowers/specs/2026-07-03-workspace-write-fence-design.md)

## Context

[ADR 0010](0010-workspace-root.md) established `WorkspaceRoot` with restricted mode as **opt-in**
(`.restricted()`), defaulting to permissive because caliban runs with the operator's own permissions. That
default leaves a gap 0010 itself flagged ("permissive default means the model can read/write anywhere the
harness process can"): setting `--workspace <dir>` scopes the *path root* but does **not** confine mutations
to it. Containment only engaged when the separate `--restrict-paths` flag was passed — off by default.

QA dogfooding (finding F2, 2026-06-19, [#237](https://github.com/caliban-ai/caliban/issues/237)) hit this in
practice: a run launched `--workspace <evals> --no-permissions` (auto-approve) but without `--restrict-paths`
appended `.venv/` to the caliban repo's own `.gitignore` — outside the workspace — and `git add`ed it. With
`--no-permissions` there was **no** path fence at all. 0010's own "Revisit if" clause anticipated a
"delegated agent / untrusted-task" mode requiring per-task sandboxing; unattended eval harnesses running many
agents on task prompts are exactly that scenario arriving.

Options weighed: (a) keep opt-in, only warn; (b) make `--no-permissions` imply restriction; (c) make
`--workspace` itself imply restriction. (a) leaves the gap open by default; (b) is surgical but couples path
containment to the permission flag, a surprising mental model; (c) matches the intuition that choosing a
workspace means "scope the agent here."

## Decision

We will make **path restriction the default whenever `--workspace` is explicitly set.** The file/shell tools
(Read/Write/Edit/MultiEdit/NotebookEdit/Bash/Glob/Grep) are confined to the workspace root unless the operator
opts out with the new **`--no-restrict-paths`** flag. A single predicate governs it:

```
should_restrict(args) = !no_restrict_paths && (restrict_paths || workspace.is_some())
```

`--restrict-paths` keeps its meaning (restrict to cwd when no `--workspace` is given) and is now redundant —
but valid — alongside `--workspace`. `--restrict-paths` together with `--no-restrict-paths` is rejected at
parse time. When a run is both auto-approving (`--no-permissions`) and unfenced, caliban emits a startup
warning.

This **amends ADR 0010** (opt-in → default-on-under-`--workspace`); 0010's `WorkspaceRoot` resolver,
canonicalize-then-prefix-check, and `~` expansion are unchanged. The interactive default with **no**
`--workspace` remains permissive.

## Consequences

- **Positive:** `--workspace` is now a real containment boundary, matching operator intuition and Claude
  Code's project-directory model. The F2 class of "agent wrote outside the intended directory" is closed by
  default for both interactive and headless/automation runs. Escape hatch (`--no-restrict-paths`) keeps
  cross-directory workflows possible.
- **Negative:** A behavior change for anyone who set `--workspace` and relied on the agent reaching files
  outside it — they must now add `--no-restrict-paths`. The redundancy between `--restrict-paths` and the
  `--workspace` implication is a minor surface wart.
- **Revisit if:** we introduce per-provider or per-session containment policy (would centralize this
  decision), or a delegated-agent mode needs a stricter fence than a single root (e.g. allow-listed egress
  paths, cf. [#36](https://github.com/caliban-ai/caliban/issues/36)).
