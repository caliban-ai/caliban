# ADR 0028 · Checkpointing + `/rewind`

- **Status:** accepted
- **Date:** 2026-05-24
- **Spec:** `docs/superpowers/specs/2026-05-24-checkpointing-design.md`

## Context

Claude Code's checkpoint + `/rewind` feature lets operators try an
aggressive multi-tool prompt knowing they can undo the file changes
without losing the conversation that followed. caliban has neither
piece: no per-prompt snapshot, no rewind menu, no Esc-Esc shortcut.
The **C. Memory & checkpointing** section of
`docs/parity-gap-matrix.md` flags this as 🔴 in three rows; **M.
Slash command coverage** flags `/rewind` as 🔴.

The natural place to wire snapshots is the `Hooks` trait — it already
fires `before_tool`/`after_tool` where we need to read pre-images.
The natural place to wire restore is the session store — truncating
message history is the same shape as a session edit.

Key trade-offs: scope (file-tool edits only, mirroring Claude Code's
Bash exclusion — capturing arbitrary subprocess side-effects is
intractable); storage layout (mirror Claude Code's
`~/.claude/projects/<project_dir_hash>/checkpoints/<session>/` so
operators with both tools recognize the shape; override via
`CALIBAN_CHECKPOINT_ROOT`); manifest + content-addressed pre-images
over whole-tree `cp -a` (cheaper, inspectable with `ls`+`cat`,
cross-prompt dedup is a future hard-link sweep, not v1).

## Decision

### Two new lifecycle hook events

`Hooks` gains `before_run` and `after_run` with default no-op impls
— existing consumers compile unchanged. These are the minimum events
checkpointing needs; broader hook-surface parity (Tier 1) will expand
the trait further but stays compatible with this addition.

### A new Layer-2 crate `caliban-checkpoints`

Recorder, store, hook impl, and restore logic live in a new crate
depending on `caliban-agent-core`, `caliban-provider`,
`caliban-sessions`. Keeps agent-core's compile time and dep surface
unchanged.

### Manifest-based, content-addressed pre-image store

For each touched file, record the pre-image once (keyed by sha256)
with metadata in `prompt-N/manifest.json` and blobs under
`prompt-N/objects/<sha256>`. Newly-created files record with
`exists_pre: false` — restore deletes them. Blob storage (not git,
not a database) because operators already trust the filesystem,
it's trivially inspectable, and cross-prompt dedup can be added
later as a background hard-link sweep without a schema change.

Only `Write`/`Edit`/`NotebookEdit`/(future)`MultiEdit` trigger
recording. Bash, WebFetch, MCP, and external writes are documented
out of scope; the rewind menu surfaces this in its footer.

### Plan-mode prompts emit empty manifests

Plan mode rejects mutating tools, so manifests come out empty. We
still emit `prompt-N/manifest.json` with `kind: "plan"` and `entries:
[]` so the prompt is selectable for conversation rewind, keeping
cursor positioning sensible across plan/non-plan prompts.

### Five restore variants

`/rewind` menu offers: restore code, restore conversation, restore
both (Enter default), summarize from here, summarize up to here. The
summarize variants drive the existing `SummarizingCompactor` on a
slice of `session.messages` — no new summarizer.

### Esc-Esc trigger, precedence owned by ADR 0027

When `InputMode::Idle` and `buffer.is_empty()`, two Esc presses
within 400ms open the rewind menu. Single Esc continues to close
modes / cancel turns. The interaction precedence is owned by ADR 0027.

### Pruning is tied to session pruning

A checkpoint directory is removed only when `cleanupPeriodDays`
(default 30) has elapsed since its last update *and* the
corresponding session is being pruned by `SessionStore::prune`. The
two operations are coupled so we never orphan checkpoints while the
session is still resumable.

`CALIBAN_CHECKPOINT_MAX_BYTES` (default 5 GiB per project) caps total
blob size; on overflow, oldest prompt blobs drop first.

## Consequences

- **Positive.** Three 🔴 rows move to ✅ in one initiative. The two
  new hook events are reusable — any future hook-surface work (Tier
  1) inherits the contract. The content-addressed blob layout is
  small enough to ship in one PR and expressive enough to grow into
  cross-prompt dedup later. Claude Code parity on the storage path
  makes a future "migrate Claude Code checkpoints into caliban" tool
  a one-evening project.
- **Negative.** Per-tool disk I/O on the hot path (pre-image read for
  every Write/Edit). The 16 MiB cap keeps it bounded but at the cost
  of unrestorable large files. One more workspace crate. Bash
  mutations remain unobservable — documented but still a footgun.
- **Revisit if:** operators demand Bash tracking (could overlay a
  filesystem-watcher-based recorder, significant complexity); or if
  storage I/O becomes a bottleneck (could move pre-image reads into
  a `tokio::spawn` shadowing the agent loop).
- **Out of scope, enabled here:** `/fork` (branch from checkpoint),
  cross-machine sync, per-tool-call (not per-prompt) granularity.

## References

- Spec: `docs/superpowers/specs/2026-05-24-checkpointing-design.md`
- Hook trait: `crates/caliban-agent-core/src/hooks.rs`
- Summarizer: `crates/caliban-agent-core/src/compact.rs::SummarizingCompactor`
- Session store: `crates/caliban-sessions/src/store.rs`
- Companion ADRs: 0027 (TUI ergonomics — owns Esc-Esc precedence and
  overlay primitives), 0021 (Sub-agents — will carry `/fork` later).
- Parity reference: `docs/claude-code-capability-inventory.md` §11.
