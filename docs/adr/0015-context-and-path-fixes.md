# ADR 0015 · Context preservation + path conventions (~/dev fix)

- **Status:** accepted
- **Date:** 2026-05-23

## Context

Real-use testing surfaced four issues bundled into one fix:

1. The TUI's ephemeral REPL (no `--session`) silently dropped every
   turn's `final_messages`, so each new prompt only saw the system
   prompt + the latest user message. Models had no memory of prior
   turns in the same REPL session.
2. `WorkspaceRoot::resolve` didn't expand `~`. When models invoked
   `Bash` with `cwd: "~/dev"` or `Read({"path":"~/notes.md"})` the
   path resolution failed with "No such file or directory." The
   model misinterpreted the error as "directory doesn't exist."
3. The TUI's tool-call input summary truncated the partial-JSON stream
   at 80 chars, sometimes hiding closing braces and making patterns
   look different than they were.
4. The default system prompt didn't tell the model that `~` is
   supported in tool paths.

## Decision

1. Add `messages: Vec<Message>` to the TUI's `App`. Initialize from
   session if any, else empty. Update from `RunEnd`'s `final_messages`
   each turn. `/clear` wipes both the in-memory history and the
   session's persisted messages.
2. `WorkspaceRoot::resolve` expands a leading `~` or `~/` to
   `dirs::home_dir()`. Affects all path arguments to all tools.
   The `Bash` command string is unchanged — the shell handles `~`
   expansion there.
3. At `ToolCallEnd`, parse the accumulated input as JSON and render
   `key="value", key=value` pairs. Fall back to raw truncation on
   parse failure.
4. Add a path-conventions bullet to the default system prompt.

## Consequences

- **Positive:** Ephemeral REPL now feels like a real conversation
  rather than a series of disconnected one-shots. `~/foo` paths
  work transparently. Tool-call summaries are readable. The
  system prompt's conventions are accurate.
- **Negative:** `App::messages` and `session.messages` are now two
  copies in `--session` mode (kept in sync at `RunEnd`). `/clear`
  is destructive to session-stored messages — documented.
- **Revisit if:** The double-keeping causes correctness bugs (e.g.,
  divergence after a mid-flight panic). The cleanest long-term
  refactor would be to make `App` hold an `Arc<RwLock<Session>>`
  and treat session as the single source of truth, with the
  ephemeral case using a synthetic in-memory session.
