# CLAUDE.md & Imports

The project memory tier is richer than a single file. At session start, caliban
walks up the directory tree from the current working directory, concatenating
every `CLAUDE.md`, `AGENTS.md`, and `.caliban.md` it finds, then resolves any
`@`-imports inside them and activates path-scoped rules from `.caliban/rules/`.

## Ancestor walk

Starting at cwd, caliban walks toward the filesystem root. The walk stops at
the first git root it finds, or the filesystem root, whichever comes first
(`WalkStop::Both`, the default).

Within each directory, files are loaded in **most-specific → most-general**
order: `.caliban.md` → `CLAUDE.md` → `AGENTS.md`. All three are concatenated;
they don't override each other.

The resulting files are spliced in **broad → narrow** order (root-first) so
that narrower, more-specific instructions appear later and take precedence in
the model's reading.

```admonish tip title="Regression escape"
If the ancestor walk misbehaves in a repo you don't have CI coverage for, set
`CALIBAN_DISABLE_CLAUDE_MD_WALK=1` to revert to the legacy single-file project
tier (`<workspace_root>/CLAUDE.md` only).
```

## `@`-imports

Any of the discovered files may contain `@`-import directives on their own
line:

```text
@./shared/conventions.md
@~/notes/api-style.md
@/abs/path/to/team-guide.md
```

Import resolution is:

- **Depth-bounded** to 5 levels of recursion.
- **Cycle-detected** by canonical path — circular imports are ignored.
- **Local paths only.** HTTP/HTTPS URLs (`@https://…`) are rejected outright to
  keep the prompt-assembly path auditable.
- **External imports** (paths outside the workspace root and outside
  `~/.config/caliban/`) require one-time approval. The approval decision is
  persisted to `~/.caliban/imports-allowlist.json`. In non-interactive mode
  (`--print`, `--bare`, CI), external imports are denied unless
  `CALIBAN_APPROVE_IMPORTS=1` is set.

Imported content is inlined at the import site with an
`<!-- imported from … -->` marker so the model can trace provenance.

## Nested on-demand

When the model reads or edits a file in a subdirectory that has its own
`CLAUDE.md`, that file is appended to the system prompt for the rest of the
session. This happens once per `(path, session)` pair — caliban does not
reload on file changes or unload when the model leaves the subtree.

The system prompt grows monotonically during a session. This is intentional:
operators reason about it as "everything the model has been told", not as a
sliding window.

## Path-scoped rules

Files under `.caliban/rules/<topic>.md` are loaded with optional
`paths:` glob frontmatter:

```markdown
---
paths:
  - "src/**/*.ts"
  - "tests/**/*.ts"
---

Always use `strict` TypeScript. Prefer `unknown` over `any`.
```

Rules without a `paths:` frontmatter are always-active and loaded at startup.
Rules with `paths:` frontmatter are activated lazily on the first file touch
matching any pattern in the set. Once activated, a rule stays in the prompt for
the rest of the session.

## `claude_md_excludes` for monorepos

Large monorepos often have directories whose `CLAUDE.md` should not be spliced
into every session. Add gitignore-style patterns to `settings.toml` to skip
them during the ancestor walk:

```toml
claude_md_excludes = [
  "node_modules/**",
  "vendor/**",
  "third_party/**/CLAUDE.md",
]
```

Patterns are evaluated **relative to the workspace root** (the cwd at startup),
not the absolute filesystem path. Last-match wins for a given path; `!`
negation is supported.

The same patterns can be supplied at runtime via the colon- or
newline-separated `CALIBAN_CLAUDE_MD_EXCLUDES` environment variable.

## Additional directories

`--additional-directories` (or `additional_directories` in `settings.toml`)
extends the set of paths the file tools can access. These directories do **not**
contribute CLAUDE.md content by default. Set
`CALIBAN_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1` to opt in — each added path then
performs its own ancestor walk, concatenated after the cwd walk in declaration
order.

```admonish note
Tier content is spliced via the `<project-claude-md>` and `<project-rule>` XML
tags in the system prompt. Use `/memory` to inspect which files were loaded and
their token counts.
```
