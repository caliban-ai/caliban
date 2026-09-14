# Checkpoints & Rewind

Caliban takes a per-prompt snapshot of every file that a file-writing tool
touched during that prompt's turns. If you don't like the result, `/rewind`
lets you pick any prior prompt and restore the files, the conversation, or
both — without losing the history of what happened in between.

## What gets snapshotted

The checkpoint recorder fires on `Write`, `Edit`, `MultiEdit`, and
`NotebookEdit`. Before any of these tools mutates a file for the first time
within a prompt, caliban reads the pre-image and stores it content-addressed
under the per-prompt blob directory.

```admonish note title="Bash mutations are not tracked"
Commands run via `Bash` (including `rm`, `mv`, `cp`, and arbitrary subprocess
writes) are not captured in the checkpoint. The `/rewind` overlay surfaces this
in its footer. Bash-created files that a `Write`/`Edit` later touches are
recorded from that point forward.
```

Plan-mode prompts (which reject mutating tools) emit an empty manifest so they
are still selectable as conversation-rewind targets.

```admonish note title="When checkpointing is on"
Checkpoints are recorded only for an **interactive TUI** session started without an
inline prompt. `-p` headless runs, `caliban "prompt"` runs, and driven serve runs record
none. The checkpoint directory is keyed by the `--session` name. Without `--session`, every
unnamed TUI run shares the `tui-ephemeral` checkpoint directory for that working directory.
```

## Disk layout

```text
~/.local/share/caliban/projects/<cwd-hash>/checkpoints/<session>/
  prompt-001/
    manifest.json
    blobs/<sha256>.bin
  prompt-002/
    manifest.json
    blobs/<sha256>.bin
  …
```

`<cwd-hash>` is the first 16 hex characters of `sha256(canonical_cwd)`.
Override the root with `CALIBAN_CHECKPOINT_ROOT`. Disable recording entirely
with `CALIBAN_CHECKPOINT_DISABLED=1`.

Each `manifest.json` records:

| Field             | Description                                                              |
|-------------------|-------------------------------------------------------------------------|
| `prompt_index`    | Monotonic prompt counter within the session (1-based)                    |
| `kind`            | `files` (normal), `plan` (plan-mode, no blobs), `cleared` (pruned)      |
| `title`           | First ~80 chars of the user message                                      |
| `created_at`      | UTC timestamp                                                            |
| `entries`         | Array of file entries (path, sha256, mode, size, `exists_pre`, tool)    |
| `partial`         | `true` if some blob writes failed                                        |

For each entry, `exists_pre: false` means the file was created by the prompt
(restore will delete it). Blobs are content-addressed — the same pre-image
across two prompts is stored once.

## Triggering `/rewind`

Open the rewind overlay from the TUI in two ways:

- Type `/rewind` at the prompt.
- Press **Esc Esc** (two Esc presses within 400 ms) when the input buffer is
  empty.

The overlay lists prompts newest-first. Move the cursor with **↑/↓** (or
**j/k**), then press an action key. The overlay closes and the action runs
against the selected checkpoint; the outcome is reported as a toast. There is
no Enter default: every action has its own key.

## Rewind actions

| Key   | Action                     | Effect                                                         |
|-------|----------------------------|----------------------------------------------------------------|
| `c`   | Restore code only          | Overwrite tracked files; leave conversation intact             |
| `v`   | Restore conversation only  | Truncate messages; leave files intact                          |
| `b`   | Restore both               | Overwrite tracked files **and** truncate conversation          |
| `s`   | Summarize from here        | Run the compactor on the messages *after* the checkpoint       |
| `S`   | Summarize up to here       | Run the compactor on the messages *up to* the checkpoint       |
| `f`   | Fork                       | Write a **new** session branched at the checkpoint; see [Forking a session](#forking-a-session) |

"Truncate conversation" removes all messages after the selected prompt's last
assistant message, so the conversation ends at that point in time.

```admonish tip
The two summarize options feed the same `SummarizingCompactor` used by
`/compact`. They're useful when you want to keep the context clean after
rolling back — for example, summarize everything before the rewind point so the
model retains the overall arc without the failed detour.
```

## Forking a session

The restore actions rewind the current session in place. **Fork** (`f`) is the
non-destructive alternative. It creates a brand-new session whose conversation
is truncated at the selected checkpoint, using the same rule as `v`, and leaves
the current session, its checkpoint history, and the working tree untouched.

- The fork gets a fresh identity: a new name, new timestamps, and zeroed
  cumulative usage. The todo list and plan-mode state carry over from the source.
- The name is derived from the current session: `<session>-fork<N>-<id>`, where
  `<N>` is the checkpoint's prompt index and `<id>` is a short unique suffix.
  The name is sanitized to letters, digits, `-`, and `_`, and capped at 64
  characters.
- The fork is saved and flushed right away. It shows up in the `/resume` list,
  and you can open it with `caliban --session <name>`.
- Forking needs session persistence. Without it, for example when you did not
  start caliban with `--session`, the overlay shows a toast asking you to
  start with `--session` instead of forking.

```admonish note title="Files are not forked"
There is one working tree, so a fork branches only the conversation. To roll the
files back too, run `c` (restore code) on the same checkpoint.
```

## Storage limits and pruning

`CALIBAN_CHECKPOINT_MAX_FILE_BYTES` (default 16 MiB) is the largest file whose
pre-image is captured. A larger file is recorded in the manifest, but it cannot
be restored.

```admonish warning title="Byte cap and age pruning are not enforced yet"
The `caliban-checkpoint` crate implements a per-project blob cap
(`CALIBAN_CHECKPOINT_MAX_BYTES`, default 5 GiB) and age-based pruning
(`CALIBAN_CLEANUP_PERIOD_DAYS`, default 30). When the cap is exceeded, the oldest
prompt blobs are dropped and each manifest is kept as a `cleared` marker. The
`caliban` binary does not call either routine yet, so checkpoint storage currently
grows until you remove it manually.
```
