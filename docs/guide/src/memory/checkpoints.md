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

The overlay lists prompts newest-first. Navigate with arrow keys, confirm with
Enter.

## Restore options

| Option                     | Default | Effect                                                         |
|----------------------------|---------|----------------------------------------------------------------|
| Restore both               | Enter   | Overwrite tracked files **and** truncate conversation          |
| Restore code only          |         | Overwrite tracked files; leave conversation intact             |
| Restore conversation only  |         | Truncate messages; leave files intact                          |
| Summarize from here        |         | Run the compactor on the messages *after* the checkpoint       |
| Summarize up to here       |         | Run the compactor on the messages *up to* the checkpoint       |

"Truncate conversation" removes all messages after the selected prompt's last
assistant message, so the conversation ends at that point in time.

```admonish tip
The two summarize options feed the same `SummarizingCompactor` used by
`/compact`. They're useful when you want to keep the context clean after
rolling back — for example, summarize everything before the rewind point so the
model retains the overall arc without the failed detour.
```

## Storage limits and pruning

`CALIBAN_CHECKPOINT_MAX_BYTES` caps total blob storage per project (default
5 GiB). When the cap is exceeded, oldest prompt blobs are dropped first; the
manifest is kept as a `cleared` marker so the prompt remains selectable for
conversation rewind (but file restore is no longer possible).

A checkpoint directory is removed only when `cleanupPeriodDays` (default 30)
has elapsed since its last update **and** the corresponding session is being
pruned by the session store. Checkpoints are never orphaned while a session is
still resumable.
