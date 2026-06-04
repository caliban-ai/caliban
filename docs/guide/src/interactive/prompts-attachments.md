# Prompts, Attachments & Images

This chapter covers how to compose prompts, reference files, and send images to the model — whether you are working interactively in the TUI or driving caliban from the command line.

## Writing prompts

In the TUI, type your prompt in the input area and press `Enter` to submit. For a multi-line prompt, press `\` followed by `Enter` to insert a newline, then `Enter` alone on a blank line to submit.

For longer drafts, press `Ctrl+G` to open the current input buffer in `$VISUAL` / `$EDITOR` / `vi`. Caliban reads the saved file back when the editor exits.

In headless mode, pass the prompt via a positional argument, `--prompt TEXT`, or pipe from stdin using `-`:

```bash
caliban "Explain the diff"
caliban --prompt "Explain the diff"
git diff | caliban -p -
```

## `@path` file references

Type `@` in the TUI input bar to open the file suggestion menu (gitignore-aware). Continue typing to narrow by path. The selected file is read and attached to your prompt as a text block at submit time.

You can also type `@path/to/file` directly without the menu. Any `@`-reference that resolves to an image-like extension (`.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`) is handled by the image pipeline rather than as text — see [Images](#images) below.

```admonish note title="Shell escape for quick commands"
Leading `!` at the start of the input bar runs the rest of the line as a shell command via the `Bash` tool (subject to permission rules). The result is not added to the conversation history.
```

## Attachment size limits

Two flags control how large `@`-attachments can be:

| Flag | Env var | Default | Meaning |
|------|---------|---------|---------|
| `--max-attach-bytes` | `CALIBAN_MAX_ATTACH_BYTES` | 262144 (256 KB) | Maximum size of a single `@`-attachment |
| `--attach-budget-bytes` | `CALIBAN_ATTACH_BUDGET_BYTES` | 1048576 (1 MB) | Aggregate cap across all attachments in one message |

If a single file exceeds `--max-attach-bytes` or the total across all files exceeds `--attach-budget-bytes`, caliban rejects the attachment with a clear error before sending anything to the model.

```bash
# Raise limits for a large codebase session
caliban \
  --max-attach-bytes 524288 \
  --attach-budget-bytes 4194304 \
  --session big-project
```

## Images

Caliban supports image input via three entry points:

1. **`@path`** — reference an image file by path in the TUI or via `--prompt "@screenshot.png explain this"` in headless mode.
2. **Clipboard paste** — paste an image from the clipboard directly into the TUI input bar (platform clipboard integration required; built with the `clipboard` feature).
3. **Drag-and-drop** — drag an image file into a supporting terminal emulator; caliban parses the DnD escape sequence and ingests the file.

Supported MIME types: `image/png`, `image/jpeg`, `image/gif`, `image/webp`.

### Ingest pipeline

Before sending an image to a model, caliban runs it through an ingest pipeline:

1. **MIME sniff** — infers type from magic bytes; rejects anything outside the allowlist.
2. **Decode + dimension check** — decodes the image to verify it is not corrupt.
3. **Downscale** — if the file exceeds 5 MiB (pre-base64) or the longest edge exceeds 1568 px, caliban downscales using Lanczos3 resampling. A `[downscaled]` badge appears in the TUI. The 1568 px target matches Anthropic's recommended longest edge for cost-efficient vision inputs.
4. **SHA-256 fingerprint** — deduplicated images are not re-sent within a session.

The pipeline is configurable via `[images]` in `caliban.toml`:

```toml
[images]
max_bytes = 5242880          # 5 MiB pre-base64 cap
downscale_target = 1568      # longest-edge px target
```

### Capability routing

By default, caliban will refuse to send an image to a model that does not have vision capability, surfacing a clear `RouterError::NoCandidate` rather than silently dropping the image. Set `CALIBAN_STRICT_ROUTING=false` to opt into degraded behavior where image content is replaced with a text placeholder and the request proceeds.

### Session storage

Images are stored as blobs under `<sessions-dir>/<session>/blobs/<sha256>.bin`. Session JSON files carry only a `BlobRef` (the SHA-256), keeping transcripts small and git-diffable.

## Graphics protocol detection

When the TUI renders an image inline, it detects the terminal's graphics protocol once at session start using the following cascade:

1. `CALIBAN_GRAPHICS` env var — values: `kitty`, `iterm`, `sixel`, `none`.
2. `$TERM_PROGRAM` — `iTerm.app` and `WezTerm` → iTerm2 protocol.
3. `$TERM` — contains `kitty` → Kitty protocol; contains `sixel` → DEC sixel.
4. Fallback — text placeholder `[image: WxH MIME filename]`.

Override detection explicitly when caliban picks the wrong protocol:

```bash
CALIBAN_GRAPHICS=kitty caliban --session vision-work
```

```admonish tip title="No vision route configured?"
If you see a `RouterError::NoCandidate` error when pasting images, confirm that your active provider and model support vision. Check the active route with `caliban router debug` or `/config` in the TUI.
```
