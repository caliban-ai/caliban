# ADR 0011 · Sessions persisted to disk + interactive REPL

- **Status:** accepted
- **Date:** 2026-05-23

## Context

caliban's MVP was single-shot — every invocation started a fresh
conversation with no memory of previous runs. For real daily use, two
things matter: (a) being able to resume a conversation across
invocations, and (b) having an interactive prompt for iterative work
without re-invoking the binary each turn.

## Decision

**Sessions:** a `PersistedSession` (name, provider, model, messages,
total_usage, timestamps) saved as pretty-printed JSON under
`$XDG_DATA_HOME/caliban/sessions/<name>.json` (default
`~/.local/share/caliban/sessions/`). Names validated against
`[a-zA-Z0-9_-]+` with length 1..=64 to prevent path traversal and
platform-incompatible names. Atomic writes via
`tempfile::NamedTempFile::persist` so crashes mid-save can't corrupt
the file.

**REPL:** `caliban` with no prompt + TTY stdin enters an interactive
loop using `rustyline` for line editing + history persistence at
`~/.local/share/caliban/repl_history.txt`. Slash commands (`/help`,
`/exit`, `/quit`, `/clear`, `/sessions`, `/save`, `/usage`) provide
session-management without exiting. When entered with `--session`,
the REPL auto-saves after every turn.

**JSON over SQLite:** chosen for transparency. Users can `cat`/edit/
diff session files; debugging is easy; no migrations. Tradeoff: O(n)
list and slower large-history loads, but until sessions exceed
thousands of turns this is irrelevant.

## Consequences

- **Positive:** zero-friction resume of any past conversation.
  Sessions are inspectable / editable / git-trackable if a user wants.
  REPL gives an interactive UX without committing to a TUI.
- **Negative:** `rustyline` adds non-trivial dependencies. Concurrent
  writes to the same session (two caliban processes) → last-write-wins
  (documented; out of scope for a single-user MVP).
- **Revisit if:** session files grow large enough that JSON parse time
  is noticeable, or users want simultaneous multi-process access.
  Migration to SQLite would be straightforward — the SessionStore API
  is the abstraction boundary.
