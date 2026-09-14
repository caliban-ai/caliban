# Sessions & Persistence

Every conversation caliban has with a model is a *session*: a named, timestamped record of messages, token usage, and active todos. Sessions persist automatically so you can stop at any point and pick up exactly where you left off.

## Starting a named session

```bash
caliban --session my-project
```

If `my-project` already exists on disk, caliban resumes it. If not, a new empty session is created. Session names must match `[a-zA-Z0-9_-]+` and be between 1 and 64 characters.

## Resuming a previous session

Three flags handle resume:

| Flag | Meaning |
|------|---------|
| `--session NAME` | Load or create the session named `NAME`. Works in the TUI and in prompt runs. |
| `-c` / `--continue` | Resume the most recently updated session. Prompt runs only. |
| `-r NAME` / `--resume NAME` | Resume an existing named session. Prompt runs only. Exits `66` if the session does not exist, and takes precedence over `--session`. |

`-c` and `-r` conflict, so pass at most one of them.

```admonish warning title="--continue / --resume do not reach the TUI"
`-c` and `-r` are read only by prompt runs: `-p` headless mode or a prompt given as an
argument. An interactive `caliban -c` or `caliban -r NAME` launches the TUI **without**
loading the session. To reopen a session interactively, use `caliban --session NAME`.
```

```bash
caliban -c -p "Where were we?"             # continue the latest session, one-shot
caliban -r my-project "Run the tests again" # resume a named session with a prompt
caliban --session my-project                 # reopen it in the TUI
```

## Resume semantics

When caliban opens an existing session it restores the full message history and accumulated token usage. The model and provider recorded in the session file are used unless overridden by `--model` or `--provider` on the command line. Plan-mode state and the todo list are also restored.

```admonish note title="Last-write-wins"
Two caliban processes writing to the same session file concurrently will race. Caliban does not lock session files — run one interactive instance per session name at a time.
```

## Suppressing persistence

To run a session entirely in memory without writing to disk, pass `--no-save`:

```bash
caliban --no-save
```

The session still functions normally for the duration of the run; nothing is written when it ends.

## Overriding the sessions directory

By default, sessions are stored under your platform's data directory (see [Files & Directories](../reference/paths.md) for the per-OS table). You can point caliban at a different directory for the duration of a run:

```bash
caliban --sessions-dir /path/to/sessions --session my-project
```

`CALIBAN_SESSIONS_DIR` is not a recognized env var for this flag — use `--sessions-dir` directly.

## Session file format

Each session is a pretty-printed JSON file at `<sessions-dir>/<NAME>.json`. Fields include `name`, `provider`, `model`, `messages`, `total_usage`, `created_at`, `updated_at`, `todos`, and `plan_mode`. Files are written atomically (via a debounced background writer with a 250 ms window) to prevent corruption from crashes mid-save.

You can inspect, diff, or even git-track session files directly — the format is intentionally human-readable.

## Listing sessions from the TUI

Inside the TUI, `/resume` lists all known sessions sorted by last-modified date. An optional substring filter narrows the list:

```text
/resume                  # show all sessions
/resume my-proj          # show sessions whose name contains "my-proj"
```

Each row shows the session name, turn count, total token usage, and last-modified time. To open a listed session, exit and re-launch with `caliban --session <NAME>`.

```admonish tip title="Quick pick"
Keep a stable `--session` name per project. `caliban --session <name>` is the one
command that reopens a conversation both interactively and in prompt runs.
```
