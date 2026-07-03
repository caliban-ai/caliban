---
name: auto-memory
description: "Persistent memory across sessions — read at start, write on learnings. Use to recall and persist user/project/feedback/reference facts per-project."
disable_model_invocation: false
metadata:
  builtin: true
  always_available: true
---

# Auto-memory

You have access to per-project memory under the caliban data directory —
`$XDG_DATA_HOME/caliban/projects/<sanitized-cwd>/memory/` (defaulting to
`~/.local/share/caliban/projects/<sanitized-cwd>/memory/`, honored uniformly on
Linux, macOS, and Windows; overridable via `CALIBAN_MEMORY_DIR`). You don't need
to construct that path yourself — the `ReadMemoryTopic` / `WriteMemoryTopic`
tools resolve it. The index, `MEMORY.md`, is already in your system prompt (the
`<auto-memory-index>` block above).

## When to READ a topic file

Use the `ReadMemoryTopic` tool with the topic's slug when:

- The user references a topic you don't fully remember from the index
  ("our email convention", "the deploy steps").
- A `[[slug]]` cross-reference appears in another topic you just read.
- The user mentions a person, system, or convention that might be
  documented.

## When to WRITE a topic file

Use the `WriteMemoryTopic` tool when the user provides one of:

1. **user** — durable facts about themselves (role, preferences, habits).
2. **feedback** — a correction or rule that should apply to future work
   ("use personal email for `~/dev/personal/**`", "skip plan review").
3. **project** — durable project facts not already documented in the repo
   ("PR labels live in `.github/labels.yaml`", "deploys go through Argo").
4. **reference** — stable external IDs / URLs (AWS account, GCP project,
   internal portal URLs, API quotas).

`WriteMemoryTopic` is atomic: it writes the topic file *and* updates the
`MEMORY.md` index in one call. You cannot half-commit.

### DO NOT save

- Transient task state ("currently debugging foo.rs:42").
- Facts already in the repo (don't duplicate `CLAUDE.md` or `README.md`).
- Single-session debug traces or HEAD SHAs.
- Personally identifying information the user did not ask you to remember.

### Format

The tool accepts:

- `name` — kebab-case slug, no slashes, no leading dot.
- `description` — one-line summary, ≤ 120 chars.
- `type` — one of `user`, `feedback`, `project`, `reference`.
- `body` — markdown. You may use `[[other-slug]]` to cross-reference
  siblings (purely informational — these are not auto-resolved).

After a successful write, the topic is available immediately via
`ReadMemoryTopic` and shows up in `/memory list`.
