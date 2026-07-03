# `--workspace` fences file writes by default

**Date:** 2026-07-03
**Ticket:** [#237](https://github.com/caliban-ai/caliban/issues/237) (`--workspace` does not sandbox file writes without `--restrict-paths`)
**Amends:** ADR [0010](../adr/0010-workspace-root.md) (WorkspaceRoot path resolution + **opt-in** restricted mode)
**Status:** Approved (design)

## Problem

`--workspace <dir>` sets the path root for the file/shell tools but does **not** confine mutations to it. Path
containment only activates when the separate `--restrict-paths` flag is passed, which is **off by default**.
So `--workspace B` alone lets the file tools (Write/Edit/MultiEdit/NotebookEdit) read and modify files
**outside** `B`. Combined with `--no-permissions` (which auto-approves every tool call), an automated run has
**no** path fence at all — an agent can write anywhere on the host with no prompt.

Confirmed by QA dogfooding (finding F2, 2026-06-19): a run launched with `--workspace <evals> --no-permissions`
(but without `--restrict-paths`) appended `.venv/` to the **caliban repo's own** `.gitignore` — a file well
outside the workspace — and `git add`ed it. Re-running with `--restrict-paths` stayed cleanly inside the
workspace.

The mechanism to fix this already exists (`WorkspaceRoot::restricted()` + `resolve()`'s
`restrict_to_root && !canon.starts_with(root)` check); it is simply off by default. This ticket flips the
default.

## Decision

**`--workspace` implies path restriction to the workspace root**, for all file/shell tools. `--no-restrict-paths`
is a new explicit opt-out that restores the previous unfenced behavior. This amends ADR 0010's "opt-in
restricted mode": restriction becomes the default whenever a workspace is explicitly chosen, because setting
`--workspace` signals an intent to scope the agent to that directory.

Non-goals: changing the interactive default when **no** `--workspace` is given (that stays unfenced), and any
change to the OS-level sandbox (ADR 0032) or permission rules (ADR 0020/0029/0045).

## Resolution rule

A single pure helper decides containment from the parsed args:

```rust
// caliban/src/args.rs
pub(crate) fn should_restrict(args: &Args) -> bool {
    !args.no_restrict_paths && (args.restrict_paths || args.workspace.is_some())
}
```

Truth table:

| `--workspace` | `--restrict-paths` | `--no-restrict-paths` | restrict? | note |
|:-:|:-:|:-:|:-:|---|
| set | — | — | **true** | the fix (was `false`) |
| set | — | set | false | explicit opt-out |
| set | set | — | true | unchanged |
| — | set | — | true | restrict to cwd (unchanged) |
| — | — | — | false | interactive default (unchanged) |
| — | — | set | false | no-op opt-out |
| * | set | set | rejected | clap `conflicts_with` at parse time |

## Components

### 1. CLI flag (`caliban/src/args.rs`)
- Add `--no-restrict-paths: bool` with `conflicts_with = "restrict_paths"`.
- Add `should_restrict(&Args) -> bool` (the rule above).
- Update `--workspace` and `--restrict-paths` help text to state the new implication ("`--workspace` restricts
  file/shell tools to that directory; pass `--no-restrict-paths` to opt out").

### 2. Registry wiring (`caliban/src/startup/compose.rs`)
- `build_registry` replaces `let root = if args.restrict_paths { workspace.restricted() } else { workspace };`
  with `let root = if crate::args::should_restrict(args) { workspace.restricted() } else { workspace };`.
- **Safety warning:** when the run ends up effectively unfenced (`!should_restrict(args)`) **and**
  `args.no_permissions` is set, emit one `tracing::warn` at startup that file mutations are not path-confined
  (the F2 danger combo — auto-approve + no fence). Cheap, non-fatal, visible in headless logs.

### 3. ADR (`docs/adr/`)
- New ADR (next number) amending ADR 0010: records the default flip (opt-in → default-on-under-`--workspace`),
  the `--no-restrict-paths` opt-out, and the rationale. Annotate 0010's status line + index row with
  "restricted-mode default amended by 00NN" (bidirectional link), mirroring the `0005 ← 0042` precedent.

### 4. Docs (`docs/guide/`)
- Update the CLI/permissions reference: `--workspace` now fences writes; `--no-restrict-paths` opts out.

## Error handling

- A write/edit/read targeting an absolute path outside the fenced root returns `ToolError::invalid_input`
  ("path … is outside workspace root …") — the existing `resolve()` behavior, now reached by default.
- `--restrict-paths --no-restrict-paths` together is a clap parse error (exit 2), never a silent precedence.
- The `--no-permissions`-unfenced case is a warning, not an error (the operator may have opted out
  deliberately with `--no-restrict-paths`).

## Testing

- **Unit (`should_restrict`, args.rs):** the full truth table above (workspace-only ⇒ true; workspace +
  `--no-restrict-paths` ⇒ false; `--restrict-paths` alone ⇒ true; nothing ⇒ false; `--no-restrict-paths`
  alone ⇒ false).
- **Clap:** `--restrict-paths --no-restrict-paths` fails to parse (`try_parse` returns `Err`).
- **Containment (`caliban-tools-builtin` workspace tests):** a `restricted()` root rejects a Write to an
  absolute path outside the root (add if not already covered).
- **F2 regression (compose-level):** with `Args` parsed from `--workspace B` (no `--restrict-paths`),
  `should_restrict` is true and the registry's root is restricted — the exact scenario that leaked in F2.
- Full CI-mirror gate green (fmt, clippy `-D warnings`, build, test).

## Acceptance criteria (from the ticket)

- [x] File tools cannot mutate paths outside `--workspace` in headless automation by default (Components 1–2).
- [x] A test covers that a write to an absolute path outside the workspace is rejected/contained (Testing).
- [x] The default change is documented (ADR amendment + guide) and reversible via `--no-restrict-paths`.
