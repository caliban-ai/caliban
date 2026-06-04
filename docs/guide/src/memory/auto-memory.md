# Auto-Memory

Auto-memory is the agent-writable third tier of caliban's memory model. At the
start of each session, caliban splices a per-project index file (`MEMORY.md`)
into the system prompt. During the session, the agent uses two built-in tools —
`ReadMemoryTopic` and `WriteMemoryTopic` — to read and write Markdown topic
files that persist knowledge across sessions.

## Directory layout

```text
~/.local/share/caliban/projects/<sanitized-cwd>/memory/
  MEMORY.md              ← index, spliced into every session (≤ 200 lines / 25 KB)
  build-commands.md      ← topic file
  api-conventions.md     ← topic file
  deploy-checklist.md    ← topic file
  …
```

The `<sanitized-cwd>` slug is derived from the canonical workspace path (e.g.
`/Users/jf/dev/caliban` → `Users-jf-dev-caliban`). Override the directory with
`CALIBAN_AUTO_MEMORY_DIRECTORY` or `CALIBAN_MEMORY_DIR`.

## The index file (`MEMORY.md`)

`MEMORY.md` is the only file loaded eagerly each session. It must stay under
200 lines / 25 KB so it fits comfortably inside the splice budget. Caliban
bootstraps an empty `MEMORY.md` with a conventions block the first time the
memory directory is accessed.

A typical index looks like:

```markdown
# Memory index

- [build-commands](build-commands.md) — project: `cargo build --release`; binary lands in `target/release/`
- [api-conventions](api-conventions.md) — feedback: prefer the built-in HTTP helper over shelling out to curl
- [deploy-checklist](deploy-checklist.md) — project: run migrations before flipping the feature flag
```

HTML comments (`<!-- … -->`) in `MEMORY.md` are stripped from the spliced
prompt but kept on disk. The auto-injected conventions block is wrapped in HTML
comments for this reason — it stays on disk for authoring guidance but does not
consume token budget.

## Topic file format

Each topic file is a Markdown file with YAML frontmatter:

```markdown
---
name: sprint-mode
description: "user prefers consolidated design proposals + spec + plan + implementation in one pass"
metadata:
  node_type: memory
  type: feedback
---

User prefers a single-pass workflow: design proposal, spec, plan, and
implementation delivered together without a human review checkpoint in
between.
```

| Frontmatter field      | Required | Description                                              |
|------------------------|----------|----------------------------------------------------------|
| `name`                 | yes      | Kebab-case slug matching the filename stem               |
| `description`          | yes      | One-line summary (≤ 120 chars); appears in the index     |
| `metadata.type`        | yes      | One of `user`, `feedback`, `project`, `reference`        |
| `metadata.node_type`   | no       | Always `memory` when written by the agent                |

**Slug rules:** non-empty, no path separators, no `..`, no leading `.`.

## Memory types

| Type        | Use for                                                            |
|-------------|-------------------------------------------------------------------|
| `user`      | Durable facts about the user (role, timezone, preferences)        |
| `feedback`  | Corrections or workflow preferences issued by the user            |
| `project`   | Durable project facts not already captured in the repo            |
| `reference` | Stable external context (account IDs, API endpoints, quotas)      |

The agent classifies each topic at write time. There is no automated
classifier — the model is best positioned to judge what to save.

## Built-in tools

| Tool               | Permission category | Description                                               |
|--------------------|---------------------|----------------------------------------------------------|
| `ReadMemoryTopic`  | `memory.*` (allow)  | Read a topic file by slug                                |
| `WriteMemoryTopic` | `memory.*` (allow)  | Write/update a topic file and update the index atomically |

Both tools are sandboxed to the memory directory — path traversal attempts are
rejected at the tool level.

`WriteMemoryTopic` performs an atomic write:

1. Write topic body + frontmatter to `<slug>.md.tmp`.
2. Rename to `<slug>.md` (atomic on the same filesystem).
3. Rewrite `MEMORY.md` with an updated index line for the slug (same
   tmp-then-rename approach).

A crash between steps 2 and 3 leaves an orphan topic file. Run
`/memory rebuild-index` to repair it.

## Managing memory

| Command                      | Effect                                              |
|------------------------------|-----------------------------------------------------|
| `/memory`                    | Show active tiers, paths, and token counts          |
| `/memory rm <slug>`          | Delete a topic file and remove its index line       |
| `/memory rebuild-index`      | Rebuild `MEMORY.md` from the topic files on disk    |

There is no automatic pruning. Memories persist until manually removed.

```admonish warning title="MEMORY.md growth"
The index grows without bound on long-running projects. Periodically review
it with `/memory` and remove stale topics with `/memory rm <slug>` to keep it
under the 200-line / 25 KB splice limit.
```

## Cross-references between topics

Topic bodies may contain `[[slug]]` cross-references, for example
`[[parity-gap-matrix]]`. These are informational breadcrumbs — caliban does not
auto-follow them. The agent can follow a reference by calling `ReadMemoryTopic`
with the referenced slug.

## Disable for CI

Set `CALIBAN_DISABLE_AUTO_MEMORY=1` to drop the auto-memory tier from the
splice and suppress the auto-memory skill. This guarantees identical system
prompts regardless of on-disk memory state. `--bare` sets the same flag
automatically.
