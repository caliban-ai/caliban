# ADR 0010 · WorkspaceRoot path resolution + opt-in restricted mode

- **Status:** accepted
- **Date:** 2026-05-23

## Context

caliban's built-in tools (Read/Write/Edit/Bash/Glob/Grep) accept paths
from model-generated tool calls. Two extremes for path handling are
both wrong: (a) reject all absolute paths — breaks legitimate use cases
like reading `/etc/hostname` for diagnostics; (b) accept any path
unconditionally — lets a model accidentally read or overwrite arbitrary
files.

## Decision

Tools share a `WorkspaceRoot` type that resolves relative paths against
a canonical root directory. Two modes:

- **Permissive (default):** Relative paths resolve under the root.
  Absolute paths are accepted as-is.
- **Restricted (opt-in via `.restricted()`):** Resolved paths must
  start with the canonical root after canonicalization. Path traversal
  via `..` is normalized away before the prefix check, so escape
  attempts (`../escape`) are rejected with `ToolError::InvalidInput`.

The CLI surface (Layer 4) chooses the mode; the default is permissive
because caliban runs with the operator's permissions in their own
environment. Restricted mode is intended for sandboxed-agent scenarios
(future: agent-as-service, untrusted-task delegation).

## Consequences

- **Positive:** Single shared resolver across all six tools; no
  per-tool path-handling logic. Restricted mode provides a meaningful
  safety boundary when needed. `..` traversal attacks are defeated by
  canonicalize-then-prefix-check.
- **Negative:** Permissive default means the model can read/write
  anywhere the harness process can. Acceptable for the personal-use
  context; documented as such.
- **Revisit if:** caliban gains a "delegated agent" mode where one
  caliban instance runs sub-tasks on behalf of another, requiring
  per-task sandboxing.
