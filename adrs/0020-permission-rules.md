# ADR 0020 · Permission rules layered on top of `Hooks`

- **Status:** accepted
- **Date:** 2026-05-23

## Context

caliban currently has no permission model. The `Hooks::before_tool`
extension point can already short-circuit a tool call with a
`HookDecision::Deny(msg)`, but nothing in the tree consults rules,
prompts the operator, or enforces a default policy. As we add more
"dangerous" tools (`BashTool` already executes arbitrary shell;
`WriteTool`, `EditTool`, `WebFetch`, future MCP tools), we need a
rule-based gate that matches the operator-facing UX of Claude Code
without inheriting its classifier complexity.

## Decision

### Implementation site

Permissions are a **layer on top of the existing `Hooks` trait** — not
a parallel system. We add a `PermissionsHook` that implements
`Hooks::before_tool` and consults a rule database. Composition with
other hooks (observability, debug logging) is handled by a small
`CompositeHooks` adapter; permissions just plug in as one entry.

### Rule schema

Each rule has three fields:

- `tool` — pattern string (glob-style; see Pattern matching).
- `action` — `Allow` | `Deny` | `Ask`.
- `comment` — optional free-text shown in the TUI prompt.

### Rule sources (priority high → low)

1. CLI flags `--allow <PAT>`, `--deny <PAT>`, `--ask <PAT>` (one-shot,
   repeatable).
2. Project file `<workspace>/.caliban/permissions.toml`.
3. User file `~/.config/caliban/permissions.toml`.
4. Built-in defaults (read-only tools `Allow`; everything else `Ask`).

Higher-priority rules shadow lower-priority ones. Within a single
source, first match wins, so users place narrow rules above the
catch-all.

### Pattern matching

Glob-style on `tool_name` plus an optional `:<first-arg-prefix>` suffix.

- `Bash` — bare tool name; matches any input.
- `Bash:git *` — bash whose `command` field starts with `git `.
- `Bash:*` — equivalent to `Bash` (explicit wildcard).
- `*` — matches every tool.

The "first arg" is tool-defined: for `Bash` it's the `command` field;
for `WebFetch` it's `url`; for `Read`/`Edit`/`Write` it's `path`. Tools
that don't declare a first-arg field are matched on tool name only.
Prefix-after-colon uses simple glob (`*`, `?`) on the stringified first
arg, not full regex — keeps the rule format inspectable.

### Ask action

`Ask` requires an interactive UI. The TUI provides a modal prompt
(allow once, allow permanently, deny once, deny permanently). In
non-interactive sessions (no TTY, no `--auto-allow`), `Ask` degrades to
`Deny` with a clear log message. `--auto-allow` is the documented
"escape hatch" for non-interactive runs and is loud about being
dangerous.

## Consequences

- **Positive:** mirrors the Claude Code rule format operators already
  know, without copying the classifier-heavy approach
  (`bashClassifier` / `yoloClassifier`). Reuses the existing `Hooks`
  contract — zero new core traits. Project + user files allow shared
  team policies committed to source control.
- **Negative:** glob matching on first-arg-prefix can be surprising
  (e.g. `Bash:rm *` does not match `Bash:sudo rm *`). Acceptable; the
  TUI prompt shows the rule that matched so users can see why a call
  was allowed/denied. Shadowed-rule warnings are deferred.
- **Revisit if:** prefix matching proves insufficient for real-world
  bash commands and operators are routinely surprised by `Allow`/`Deny`
  outcomes. Next step would be a classifier (LLM-graded
  command-intent), but we want concrete evidence before going there.
