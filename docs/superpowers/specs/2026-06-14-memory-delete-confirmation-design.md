# `/memory delete` confirmation gate

**Issue:** caliban-ai/caliban#112 — `/memory delete` should not silently, irreversibly remove a topic file.
**Date:** 2026-06-14

## Problem

`/memory delete <slug>` calls `loader.delete()` and permanently removes
`<auto_memory_dir>/<slug>.md` with no confirmation and no undo. The `immediate:true`
flip in #104 was intended for read-only / non-destructive commands.

Note on `immediate`: per `caliban/src/tui/slash.rs`, `immediate:true` means the command
*may fire mid-turn (it doesn't need the model)* — the user still presses Enter to submit.
Flipping it to `false` would **not** add a confirmation; it would only block `/memory`
while a turn is in flight. The fix therefore lives in the `delete` handler, not the flag.

`immediate` is also per-command, not per-subcommand, so there is no way to make only
`delete` non-immediate. The read subcommands (`""`/`list`/`show`) must stay responsive,
so `/memory` keeps `immediate:true` and the gate is implemented in handler logic.

## Approach

A two-step `--force` gate, mirroring the existing destructive-op idiom in `/init`
(`caliban/src/tui/slash/session.rs`, which refuses to overwrite `CLAUDE.md` without
`--force`). The command stays stateless, honoring the design note in `slash.rs`
("the caller acts on outcome — it never reaches into the command for follow-up state").

### Behavior

| Input | Result |
|---|---|
| `/memory delete` (no slug) | usage message (unchanged) |
| `/memory delete badslug!` | `bad slug: …` via `validate_slug` (as `edit` does) |
| `/memory delete foo` (file missing) | `no such topic: <path>` (as `edit` does) — no `--force` nag for an absent file |
| `/memory delete foo` (file exists) | **preview, no deletion:** `will permanently delete <path>` + `re-run with: /memory delete foo --force` |
| `/memory delete foo --force` | calls `loader.delete()`, emits `deleted topic 'foo'` |

- `--force` is order-independent (`delete --force foo` works).
- Applies identically to the `delete` and `rm` aliases.
- Path resolution and slug validation reuse the same calls as the `edit` arm
  (`validate_slug`, `loader.dir().join("{slug}.md")`) for consistency.

## Structure (for testability)

- A pure helper `parse_delete_args(rest: &str) -> ParsedDelete { slug: Option<String>, force: bool }`:
  splits `rest`, takes the first non-`--` token as `slug`, sets `force` if any token is
  `--force`. This is the bug-prone part (flag/slug ordering), so it carries the unit tests —
  matching the codebase's pure-helper convention (`compose_draft`, `is_immediate_slash`).
- The `delete | rm` arm becomes a thin shell: `parse_delete_args` → `validate_slug` →
  existence check → `if force { delete } else { preview }`.

`parse_kv_args` is not reused because it drops the bare slug token; a small dedicated
parser keeps slug + flag in one pass.

## Tests (TDD, RED first)

`parse_delete_args` unit tests:
- slug only → `{ slug: Some, force: false }`
- slug + `--force` → `{ slug: Some, force: true }`
- `--force` + slug (order) → `{ slug: Some, force: true }`
- missing slug → `{ slug: None, force: false }`
- `--force` only → `{ slug: None, force: true }`

The force gate is the single `if force` branch driven by the parsed `force` field.

## Acceptance criteria

- `/memory delete <slug>` cannot delete a file in one un-acknowledged step — the first
  invocation is a pure preview; deletion requires an explicit `--force` re-run. ✅
- Read-only `/memory` subcommands remain responsive (`immediate:true` unchanged). ✅
