---
title: Checkpointing + /rewind
date: 2026-05-24
status: Proposed
author: john.ford2002@gmail.com
adr: adrs/0028-checkpointing-rewind.md
---

# Checkpointing + `/rewind` — Design

**Date:** 2026-05-24
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** `adrs/0028-checkpointing-rewind.md`

## Goal

Match Claude Code's per-prompt checkpoint + rewind. Every user prompt
produces a snapshot of the workspace files that *file-writing tools
touched during that prompt's turn(s)*; `/rewind` opens a menu listing
those checkpoints and offers five restore variants. Esc-Esc from an
empty input bar opens the same menu without typing the command.

After this spec ships, an operator can hit Esc-Esc, pick a prompt from
three hours ago, and have their working tree restored to that state —
without losing the conversation that happened after.

## Non-goals

- **Snapshotting non-file mutations.** Bash `rm`/`mv`/`cp`, git
  resets, package installs — all unchanged. Mirrors Claude Code's
  documented limitation.
- **Version-control replacement.** Safety net, not git.
- **Cross-machine sync.** Local-only; tied to one machine's session ID.
- **Retroactive snapshots.** Pre-0028 sessions get no checkpoints
  retroactively; new sessions get them from first prompt.
- **`/fork` (branch a session from a checkpoint).** Sub-agent fleet spec.

## Architecture

```
                       user types prompt
                              │
                              ▼
                ┌───────────────────────────────┐
                │ Hooks::before_run (NEW)       │
                │   ▶ CheckpointRecorder        │  ── capture session_id + cwd
                └───────┬───────────────────────┘
                        │ manifest = []
                        ▼
                  agent loop turns
                        │
                        │ each Tool::invoke for Write/Edit/NotebookEdit:
                        │   Hooks::after_tool (existing) ──▶ CheckpointRecorder
                        │     read pre-image (if exists)
                        │     write blob to objects/<sha256>
                        │     append entry to in-memory manifest
                        │
                        ▼
                ┌───────────────────────────────┐
                │ Hooks::after_run (NEW)        │
                │   ▶ flush manifest.json       │
                └───────────────────────────────┘
                              │
                              ▼
            ~/.claude/projects/<project_hash>/checkpoints/<session>/
              prompt-001/  manifest.json + objects/{sha256}
              prompt-002/  …
              index.json   (lazy summary)
```

Two cooperating types:

1. **`CheckpointRecorder`** — owns hashing, blob writes, manifest
   assembly. Lives in a new `caliban-checkpoints` crate.
2. **`CheckpointHook`** — a `Hooks` impl driving the recorder from
   the existing pre/post hook points plus two new lifecycle events
   (`before_run`/`after_run`).

Restore is the inverse: read the manifest, copy each blob back into
its tracked path. Conversation rewind truncates `App.messages` to the
checkpoint's `last_message_id` and re-saves via `SessionStore::save`.

## Crate structure (delta)

```
crates/caliban-checkpoints/        # NEW Layer-2 crate
└── src/
    ├── lib.rs                     # re-exports
    ├── error.rs                   # CheckpointError
    ├── store.rs                   # CheckpointStore (filesystem layout)
    ├── manifest.rs                # Manifest + Entry (serde)
    ├── recorder.rs                # CheckpointRecorder (in-memory state)
    ├── hook.rs                    # CheckpointHook: impl Hooks
    ├── restore.rs                 # restore_files / restore_conversation
    └── prune.rs                   # cleanup_period_days enforcement

crates/caliban-agent-core/src/hooks.rs   # add `before_run` + `after_run`
                                          # with default no-op impls

caliban/
├── src/main.rs                    # wire CheckpointHook into Hooks chain
└── src/tui/rewind_menu.rs         # NEW: per-prompt menu overlay
```

`caliban-checkpoints` depends on `caliban-agent-core`,
`caliban-provider`, `caliban-sessions`, `sha2`, `serde`, `serde_json`,
`chrono`. No tokio fs — writes happen inside the existing async hook
which runs on the tokio thread pool.

## Hook surface deltas

```rust
#[async_trait]
pub trait Hooks: Send + Sync {
    // …existing four methods…

    async fn before_run(&self, _ctx: &RunCtx<'_>) -> Result<()> { Ok(()) }
    async fn after_run(&self, _ctx: &RunCtx<'_>, _outcome: &RunOutcome) -> Result<()> { Ok(()) }
}

pub struct RunCtx<'a> {
    pub session_id: &'a str,
    pub workspace_root: &'a Path,
    pub user_message: &'a Message,
    pub prompt_index: u32,    // monotonic per session
    pub cancel: CancellationToken,
}
```

Default no-ops keep existing `Hooks` impls (`NoopHooks`,
`PermissionsHook`) unchanged. The `Agent::run` loop invokes these at
the start/end of each run.

These events also partially unblock **B. Hooks & extensibility** rows
(`SessionStart`/`SessionEnd`/`UserPromptSubmit`) — but that's a
happy side-effect; the surface here is the minimum checkpointing
needs.

## What counts as a checkpoint mutation

A file is recorded in the current prompt's manifest iff all of:

- tool name is `Write`, `Edit`, `NotebookEdit`, or (future) `MultiEdit`
  (`MultiEdit` counts as a sequence of `Edit`s).
- `before_tool` resolved to `Allow`.
- `after_tool` carried `Ok(_)`.
- The resolved path lies under `workspace_root` (non-workspace paths
  surface a one-time toast `[checkpoint: skipped non-workspace path]`).

Bash `rm`/`mv`/`cp` are explicitly **not** tracked. The rewind-menu
footer documents this:

```
ℹ Bash and external writes are NOT checkpointed.
  /rewind only restores files touched by Write/Edit/NotebookEdit.
```

Plan-mode prompts produce empty manifests (plan rejects mutating
tools). We still emit `prompt-N/manifest.json` with `entries: []` and
`kind: "plan"` so the prompt is selectable for conversation rewind.

## Storage layout

```
~/.claude/projects/<project_dir_hash>/
  checkpoints/
    <session_id>/
      index.json                  ← summary: prompt index → {title, kind, created}
      prompt-001/
        manifest.json             ← Entries: [{path, sha256, mode, size, last_message_id, …}]
        objects/<sha256>          ← raw pre-image bytes (content-addressed)
      prompt-002/  …
```

- `project_dir_hash` = `sha256(canonical_workspace_root)[..16]`.
  Matches Claude Code's mapping so users find the dir alongside
  Claude Code's. Override via `CALIBAN_CHECKPOINT_ROOT`.
- Blobs are content-addressed by sha256 of the pre-image bytes.
  Within a prompt, repeat touches of the same file dedupe to one
  blob. Across prompts, cross-prompt dedup is lazy via a future
  hard-link sweep — not v1.
- Manifest entry shape:

  ```json
  {
    "path": "src/foo.rs",
    "sha256": "a1b2c3...",
    "mode": 644,
    "size": 1234,
    "exists_pre": true,
    "last_message_id": "msg_01...",
    "tool_name": "Edit",
    "tool_use_id": "toolu_01...",
    "error": null
  }
  ```

- `exists_pre: false` records "prompt created this file" — restore
  deletes it.

## Restore flow

Five menu options mapped to recorder calls:

| Menu option                  | Files                       | Conversation                               |
| ---------------------------- | --------------------------- | ------------------------------------------ |
| Restore code                 | overwrite from manifest     | unchanged                                  |
| Restore conversation         | unchanged                   | truncate to `last_message_id`              |
| Restore both (Enter default) | overwrite from manifest     | truncate to `last_message_id`              |
| Summarize from here          | unchanged                   | `Summarizing` compactor over after-slice   |
| Summarize up to here         | unchanged                   | `Summarizing` compactor over before-slice  |

```rust
pub struct RestoreOptions {
    pub files: bool,
    pub conversation: ConversationRestoreMode,
}

pub enum ConversationRestoreMode {
    None, TruncateAtPrompt, SummarizeFromHere, SummarizeUpToHere,
}

pub async fn restore(
    store: &CheckpointStore,
    session: &mut PersistedSession,
    workspace_root: &Path,
    prompt_index: u32,
    options: RestoreOptions,
) -> Result<RestoreOutcome, RestoreError>;
```

### File restore

For each manifest entry:

1. `exists_pre: false` → delete the file (ignore NotFound).
2. `exists_pre: true` → write blob from `objects/<sha256>` to
   `entry.path`, restoring `mode`.

Files touched in *later* prompts that aren't in this manifest are
**not** rolled back. The menu footer makes this explicit:

```
This restores files prompt-005 touched (3 files). It does NOT roll
back later prompts. Use 'Restore both' to also truncate the
conversation.
```

### Conversation restore

```rust
fn truncate_at_prompt(session: &mut PersistedSession, last_message_id: &str) {
    if let Some(idx) = session.messages.iter().position(|m| m.id.as_deref() == Some(last_message_id)) {
        session.messages.truncate(idx + 1);
    }
}
```

Providers without server-issued IDs (some OpenAI compat paths, local
Ollama) fall back to counting user messages by index.

### Summarize variants

Both call the existing `SummarizingCompactor` on a slice of
`session.messages` and replace the slice with the summary message —
no new summarizer.

## TUI integration

### `/rewind` slash command

Adds `/rewind` to `SLASH_COMMANDS`. Renders the `RewindMenu` overlay:

```
┌─ Rewind ────────────────────────────────────────────────────────────┐
│ ▶ #007  "fix the failing test"          14:32   2 files             │
│   #006  "now refactor the parser"       14:18   5 files             │
│   #005  "(plan) sketch a parser"        14:05   plan-only           │
│   #004  "add the lexer"                 13:51   3 files             │
│ ─────────────────────────────────────────────────────────────────── │
│ Selected: prompt-007 — "fix the failing test"                       │
│   [c] restore code   [v] restore conversation   [b] restore both    │
│   [s] summarize from here   [S] summarize up to here   [Esc] close  │
│ ℹ Bash and external writes are NOT checkpointed.                    │
└─────────────────────────────────────────────────────────────────────┘
```

Enter = `b` (restore both). Plan-only prompts disable `c` with a hint.
After a restore: confirmation toast (`[rewound to prompt-007 — 2
files restored, conversation truncated]`), transcript redraws, menu
closes.

### Esc-Esc

When `InputMode::Idle` and the buffer is empty, two Esc presses within
400ms open the menu. The 400ms threshold lives in
`App::last_esc_at: Option<Instant>`. Single Esc continues to close
menus / cancel turns. Esc-Esc does **not** fire from non-Idle modes —
this precedence contract is owned by ADR 0027.

## Pruning policy

Tied to `cleanupPeriodDays` (default 30). At session-load time,
`caliban-checkpoints::prune::run(&store)` walks `checkpoints/*` and
removes any `<session_id>/` dir whose `updated_at` is older than the
threshold *and* whose parent session has been removed by
`SessionStore::prune`. The two prunes are coupled so we never orphan
checkpoints.

Safety knobs:

- `CALIBAN_CHECKPOINT_DISABLED=1` skips both recording and pruning.
- `CALIBAN_CHECKPOINT_MAX_BYTES` (default 5 GiB per project) caps
  total `objects/` size; on overflow, the oldest prompt's blobs are
  dropped first (the manifest stays as a ⚠-marked marker).
- `CALIBAN_CHECKPOINT_MAX_FILE_BYTES` (default 16 MiB) caps pre-image
  reads; larger files record as `partial: true` with an `error` and
  cannot be restored.

## Failure handling

- **Pre-image read fails** (unreadable): record entry with `error:
  "<message>"`; restore skips with a logged warning.
- **Blob write fails** (disk full, EACCES): downgrade to in-memory log
  entry; manifest marked `partial: true`; toast on next prompt; rewind
  menu badges with ⚠.
- **Manifest write fails**: log + drop state; no checkpoint for that
  prompt.
- **Restore overwrite fails**: abort and surface error; partial
  restores are NOT rolled back (no second backup). Documented in
  menu confirmation.

## Public API sketches

```rust
pub struct CheckpointStore { root: PathBuf, max_bytes: u64 }
pub struct Manifest { pub kind: ManifestKind, pub entries: Vec<Entry>, pub partial: bool }
pub enum ManifestKind { Files, Plan, Cleared }
pub struct Entry { pub path: PathBuf, pub sha256: String, pub mode: u32,
                   pub size: u64, pub exists_pre: bool,
                   pub last_message_id: Option<String>,
                   pub tool_name: String, pub tool_use_id: String,
                   pub error: Option<String> }

impl CheckpointStore {
    pub fn open(project_dir: &Path, session_id: &str) -> Result<Self, CheckpointError>;
    pub fn list_prompts(&self) -> Result<Vec<PromptSummary>, CheckpointError>;
    pub fn load_manifest(&self, prompt_index: u32) -> Result<Manifest, CheckpointError>;
    pub fn read_blob(&self, sha: &str) -> Result<Vec<u8>, CheckpointError>;
}

pub struct CheckpointHook { /* recorder + store + inner Hooks */ }
#[async_trait]
impl Hooks for CheckpointHook { /* before_run / after_run / after_tool */ }
```

## Tests (enumerated)

1. **Captures Write pre-image** — manifest has one entry; blob bytes
   equal the original file content.
2. **Captures Edit pre-image** — similar.
3. **Captures NotebookEdit pre-image** — gated on tool existence;
   skipped if absent.
4. **`exists_pre: false`** — Write creates new file; restore deletes it.
5. **Skips Bash** — `Bash:rm a.txt` does not appear in manifest.
6. **Skips non-workspace paths** — Write to `/tmp/foo` ignored; toast
   once per session.
7. **Plan-mode empty manifest** — `kind: "plan"`, `entries: []`.
8. **Manifest persistence** — flush + reload round-trips.
9. **Blob deduplication within a prompt** — two Edits on the same
   file → one blob, one manifest entry, first `tool_use_id` wins.
10. **Restore code overwrites files** — change file then restore →
    file matches pre-image.
11. **Restore code deletes created files** — `exists_pre: false` entry
    removes the file.
12. **Restore conversation truncates** — session of 6 messages
    truncates correctly at prompt-2's marker.
13. **Restore both** — files first, then session save.
14. **Summarize from here** — fixture `SummarizingCompactor` records
    its inputs; assert called with expected slice.
15. **Pruning removes orphans** — checkpoint older than 30d whose
    session is also pruned → removed.
16. **`CALIBAN_CHECKPOINT_MAX_BYTES`** — cap set; oldest prompt
    blobs dropped on overflow.
17. **`CALIBAN_CHECKPOINT_DISABLED`** — `before_run` is no-op; no
    manifest.
18. **Esc-Esc opens menu** — two Esc within 400ms with empty buffer.
19. **Esc-Esc with non-empty buffer** — first Esc clears input, second
    is no-op (no menu).
20. **`/rewind` opens menu** — slash dispatcher routes to overlay.
21. **`before_run`/`after_run` default no-ops** — existing `NoopHooks`
    and `PermissionsHook` still compile and pass their tests.

## Risks

- **Disk I/O on hot path.** Mitigation: 16 MiB pre-image cap (configurable);
  files exceeding it are marked partial and unrestorable.
- **Parallel tool dispatch races.** Mitigation: recorder holds a
  `BTreeMap<PathBuf, Entry>` keyed by canonical path; first writer
  wins for the pre-image (correct — we want the *original*).
- **Symlinks escaping workspace.** Mitigation: canonicalize and require
  workspace prefix; else skip with toast.
- **Session-ID renames orphan past checkpoints.** Mitigation:
  documented; orphans pruned on next session-cleanup pass.
- **`SummarizingCompactor` invalidates prompt-cache markers.**
  Mitigation: set `session.cache_invalidated_at` flag; agent loop
  starts a fresh cache prefix on next turn.
- **Esc-Esc collides with single-Esc cancel.** Mitigation: 400ms
  window preserves single-Esc behavior for any user not
  rapid-firing.

## Acceptance criteria

- `cargo build --workspace` clean; clippy clean; fmt clean.
- ≥21 new tests passing.
- `caliban-checkpoints` exports `CheckpointStore`, `CheckpointRecorder`,
  `CheckpointHook`, `Manifest`, `Entry`, `restore`, `RestoreOptions`.
- `caliban-agent-core::hooks` gains `before_run` + `after_run` with
  default no-op impls.
- Binary registers `CheckpointHook` by default; `CALIBAN_CHECKPOINT_DISABLED=1`
  opts out.
- Gap-matrix updates:
  - **C.** Auto-checkpoint per prompt + `/rewind` → ✅
  - **C.** Esc-Esc / fork-from-checkpoint → ✅ (Esc-Esc only;
    fork-from-checkpoint stays 🔴, handled in sub-agent fleet spec).
  - **M.** `/rewind` → ✅
- README adds a "Checkpointing" section: storage layout, Bash
  limitation, `cleanupPeriodDays` interplay, kill switch.
- ADR 0028 in `accepted` status.

## Cross-spec dependencies

- **ADR 0027 (TUI ergonomics)** owns the Esc-Esc precedence contract
  and the modal-overlay infrastructure the rewind menu reuses.
- **ADR 0016 (Parallel tool dispatch)** — recorder must be cancel-safe
  across parallel `after_tool` invocations. Covered under Risks.
- The two new hook events here overlap with future Tier-1 work on the
  broader hook surface. This spec ships only what checkpointing needs;
  the rest tracks under a Tier-1 follow-up.
