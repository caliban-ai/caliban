# ADR 0035 · Auto-memory (model-written notes)

- **Status:** accepted
- **Date:** 2026-05-24
- **Author:** john.ford2002@gmail.com
- **Spec:** `docs/superpowers/specs/2026-05-24-auto-memory-design.md`

## Context

`caliban-memory`'s third tier (the auto tier, XML tag `auto-memory-index`)
currently bootstraps an empty `MEMORY.md` with a conventions block and
splices it into the prompt — but no machinery exists for *writing*
memory back. Claude Code's auto-memory feature is operator-visible
gold: the model accumulates per-project user/feedback/project/reference
facts across sessions, and re-loads them as part of the system prompt
each turn. Closing this gap is one of the highest-leverage rows in the
parity matrix because every other feature (skills, slash commands,
hook handlers) gets compounded by long-running memory.

The on-disk layout the user already maintains under
`~/.claude/projects/<sanitized-cwd>/memory/` is well-defined: an index
`MEMORY.md` + one markdown file per topic, each with YAML frontmatter
declaring `name` / `description` / `metadata.type`. We adopt it
verbatim under `~/.caliban/projects/<sanitized-cwd>/memory/`.

## Decision

### Three artifacts together implement auto-memory

1. **Loader extension** in `caliban-memory` — reads `MEMORY.md` (first
   200 lines / 25 KB), strips HTML comments, splices it into the
   prompt under `<auto-memory path="…" topic_count="N">…</auto-memory>`.
2. **`TopicLoader`** in `caliban-memory::auto` — lists / reads / writes
   / deletes topic files (sibling `.md` of `MEMORY.md`); does atomic
   write + index-line update in a single call so the model can't half-
   commit.
3. **Built-in `auto-memory` skill** bundled in `caliban-skills` — its
   body is the protocol manual (when to read, when to write, the four
   types, anti-examples). With `disable_model_invocation: false`, the
   skill is always available + always loaded into the system prompt.

### Two new built-in tools, `ReadMemoryTopic` and `WriteMemoryTopic`

We do **not** reuse `Read`/`Write` for memory access because (a) memory
paths are sandboxed to the memory dir (path-traversal guard) and (b)
writes need to atomically update both the topic file *and* the index
line — that's a single tool call, not two `Write`s. Both tools live in
`caliban-tools-builtin` under a new `memory.*` permission category
(allowed by default).

### MEMORY.md is splice-only; topic files are on-demand

The index is small enough to splice every turn (200 lines / 25 KB cap).
Topic files can be hundreds of KB collectively; they're pulled by slug
on demand via `ReadMemoryTopic`. `[[slug]]` cross-references between
topics are *informational* breadcrumbs — the loader does not
auto-follow them.

### HTML-comment stripping is done at splice time

`<!-- -->` blocks in `MEMORY.md` are stripped from the spliced prompt
(but stay on disk). This lets us keep the auto-injected
`CONVENTIONS_BLOCK` HTML-comment-fenced so it doesn't fight with
operator-authored content. The strip is greedy (regex), which means a
fenced code block containing `<!-- -->` will lose the comment in the
spliced view — documented limitation, low-impact.

### Four memory types, model decides at write time

`user` / `feedback` / `project` / `reference`. The skill body
documents heuristics + anti-examples; the model classifies inline.
We deliberately avoid a typed classifier service — the model is in
the best position to judge what to save, and we don't want a hidden
ML layer between the user's intent and the on-disk artifact.

### No automatic pruning

Memories persist until manually removed. `/memory rm <slug>` and
`/memory rebuild-index` cover the manual-curation path. Automatic
forgetting is a research problem that we explicitly punt on.

### `CALIBAN_DISABLE_AUTO_MEMORY=1` is both a privacy kill switch
### *and* a determinism switch for CI

When set, no `<auto-memory>` block is spliced *and* the auto-memory
skill is dropped from the system prompt. This guarantees that headless
runs and CI workflows produce identical prompts regardless of
on-disk memory state.

### The on-disk format is the source of truth

We do not invent a database or sqlite layer. Markdown + YAML
frontmatter is human-readable, git-friendly, and aligns with how
operators already mentally model `CLAUDE.md`. The trade-off — file
locking concurrency, parsing overhead — is acceptable at the scales
auto-memory actually sees (tens of topic files, kilobytes each).

### Atomic writes via tempfile + rename

`WriteMemoryTopic` writes to `<slug>.md.tmp` then renames; index-line
update is part of the same operation. Failure mid-write leaves the
prior content intact. Failure between topic-write and index-update
leaves an orphan topic file — `rebuild-index` repairs it.

## Consequences

- **Positive:** Closes a tier-5-priority row in the parity matrix that
  compounds the value of every long-running session. Operators get
  Claude Code's "wow it remembered" UX out of the box. The on-disk
  format means operators can manually curate memory with their
  favorite text editor. Composes with skills (the protocol *is* a
  skill) so the system documents itself.
- **Negative:** Two new built-in tools to maintain + a new permission
  category. The auto-memory skill body is a maintenance surface (15
  CI test asserts it doesn't drift). HTML-comment stripping is a
  hidden behavior that may surprise operators. No automatic pruning
  means MEMORY.md grows unbounded on long-running projects — operator
  hygiene is required.
- **Revisit if:** The 200-line / 25 KB cap turns out to be too small
  in practice (operators routinely brush against the truncation
  warning); a richer indexer that summarizes topic files into the
  splice may be needed. If concurrent writes from background subagents
  prove racy, add file locks (`fs2::FileExt::try_lock_exclusive`). If
  the markdown+frontmatter parsing overhead shows up in startup
  profiles, add a per-topic cache keyed by `mtime`.
