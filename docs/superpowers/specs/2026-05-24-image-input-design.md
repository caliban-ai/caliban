# Image / vision input — Design

**Date:** 2026-05-24
**Status:** Proposed
**Author:** john.ford2002@gmail.com
**Sub-project of:** caliban Rust agent harness
**Companion ADR:** `docs/adr/0039-image-and-vision-input.md`
**Depends on:** TUI input ergonomics spec
(`docs/superpowers/specs/2026-05-23-tui-input-ergonomics-design.md` — file
mention autocomplete + clipboard plumbing), model-router v2 spec
(`docs/superpowers/specs/2026-05-24-model-router-v2-design.md` — capability
filtering picks vision-capable routes).

## Goal

Let users attach images to messages in caliban and have them flow through
to whichever provider supports vision:

- **Paste from clipboard** in the TUI (`Ctrl+V` or platform paste binding).
- **`@path/to/image.png` mention** in the input box autocompletes to a
  filesystem image and attaches it as a content block.
- **Drag-and-drop** in terminals that surface paths to caliban
  (kitty/wezterm/iTerm2 escape sequences).
- **CLI `--image <path>`** for headless / scripted runs.

Images map to the right provider-specific multi-modal block on the wire:
Anthropic `image` content block (base64), OpenAI `image_url` (data URL),
Google `inline_data` (base64). Capability-filtering in the router ensures
they only reach providers with `Capabilities.vision == true`; non-vision
adapters get a text fallback.

The TUI displays thumbnails inline using kitty/sixel graphics where
available; otherwise a `[image: <dims> <kind>]` placeholder.

## Non-goals

- **Image generation / editing tools.** Output-side vision (the model
  *returning* an image) is out of scope. Anthropic and OpenAI both
  support input-only vision today; output-image generation is a future
  ADR.
- **Video.** Single-frame images only; gif is treated as a still
  (provider-defined behavior).
- **Audio / file types beyond image MIME.** A future "file attachment"
  spec covers PDF/audio.
- **OCR fallback on text-only providers.** If the route is non-vision and
  capability filtering can't find a vision route, we surface a text
  placeholder; we do not run a local OCR.
- **Editing images in caliban.** Crop, rotate, annotate — out of scope.
  We only ingest + downscale.
- **Server-side caching of image bytes.** Anthropic's image prompt-cache
  is honored if the route declares it; we do not maintain our own image
  cache.

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│  TUI / CLI input layer                                               │
│   ├── @path/file.png mention   (file-suggestion source from TUI spec)│
│   ├── Ctrl+V paste              (arboard::Clipboard::get_image)      │
│   └── DnD escape sequences      (terminal-specific decoder)          │
└─────────────────────────┬────────────────────────────────────────────┘
                          │ ImageAttachment { bytes, mime, dims }
                          ▼
┌──────────────────────────────────────────────────────────────────────┐
│  caliban-images crate (NEW)                                          │
│   ├── decode + validate (image crate)                                │
│   ├── enforce size cap (5 MB pre-base64; downscale if larger)        │
│   ├── compute SHA-256 (id + dedupe within turn)                      │
│   └── emit ImageBlock (caliban-provider IR)                          │
└─────────────────────────┬────────────────────────────────────────────┘
                          │ ContentBlock::Image(ImageBlock { … })
                          ▼
┌──────────────────────────────────────────────────────────────────────┐
│  caliban-agent-core / caliban-provider                               │
│   ImageBlock { source: ImageSource, mime: String, sha256: String,    │
│                dims: (u32,u32), cache_control: Option<…> }           │
│   ImageSource::Base64(String)  | ImageSource::Url(Url)               │
└─────────────────────────┬────────────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────────────┐
│  caliban-model-router (capability filter)                            │
│   has_image → require capabilities.vision                            │
└─────────────────────────┬────────────────────────────────────────────┘
                          │
            ┌─────────────┼─────────────┐
            ▼             ▼             ▼
       Anthropic       OpenAI        Gemini
       image block     image_url     inline_data
       base64          (data:…)      base64
```

The IR change is small: `ContentBlock` gains an `Image(ImageBlock)`
variant. Existing handlers ignore it unless they care about images
(provider adapters, the TUI renderer, the cache normalizer).

## Crate structure

```
crates/
├── caliban-images/             (NEW)
│   └── src/
│       ├── lib.rs              # ImageAttachment + Pipeline
│       ├── decode.rs           # mime sniffing, dims, frame check
│       ├── downscale.rs        # image::imageops::resize (Lanczos3)
│       ├── clipboard.rs        # arboard wrapper (feature = "tui")
│       └── dnd.rs              # terminal DnD escape decoding
├── caliban-provider/           (modified)
│   └── src/lib.rs              # ImageBlock IR + ContentBlock::Image
├── caliban-provider-anthropic/ (modified) # serialize Image variant
├── caliban-provider-openai/    (modified)
├── caliban-provider-google/    (modified)
├── caliban-provider-ollama/    (modified — stub; vision = false)
├── caliban-agent-core/         (modified) # ImageBlock through Message
└── caliban-tui/                (modified) # paste, render, file mention
```

### Cargo deps (additions)

```toml
# In caliban-images:
image       = { version = "0.25", default-features = false, features = ["png", "jpeg", "gif", "webp"] }
sha2        = { workspace = true }
base64      = { workspace = true }
arboard     = { version = "3", optional = true, default-features = false }
mime        = "0.3"
infer       = "0.16"          # robust MIME sniff from leading bytes
url         = { workspace = true }

[features]
default = ["clipboard"]
clipboard = ["dep:arboard"]
```

`image` is pinned to the `png/jpeg/gif/webp` features only — we
explicitly disable `bmp`, `tiff`, `dds`, `tga`, etc. to keep the
attack surface narrow. AVIF and HEIC are tracked but not in v1.

## IR change — `ContentBlock::Image`

```rust
// caliban-provider/src/content.rs

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageBlock {
    pub source: ImageSource,
    pub mime: String,                       // e.g. "image/png"
    pub sha256: String,                     // 64-char hex
    pub dims: (u32, u32),                   // (width, height)
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { data: String },                // raw base64 (no `data:` prefix)
    Url    { url: Url },                    // for adapters that prefer URLs
}

pub enum ContentBlock {
    Text(TextBlock),
    Image(ImageBlock),
    /* … existing variants … */
}
```

The IR carries `sha256` (computed once at ingest) so:

- Provider adapters dedupe identical images across messages.
- Checkpointing references images by hash instead of inlining bytes
  twice (see "Checkpointing" below).
- `RouterStats.token.usage` records `image_count` keyed by sha256 for
  cost accounting.

## Ingest pipeline (`caliban-images::Pipeline`)

```rust
pub struct Pipeline {
    pub max_bytes: usize,            // 5 MiB default
    pub downscale_target: u32,       // 1568 px on longest edge (Anthropic recommendation)
    pub allowed_mime: &'static [&'static str],
}

impl Pipeline {
    pub fn ingest(&self, bytes: Vec<u8>, hint_mime: Option<&str>)
        -> Result<ImageBlock, ImageError>;
}
```

Algorithm:

1. **MIME sniff** — `infer::get(&bytes)`; if `None`, fall back to
   `hint_mime`. If neither yields a supported type, return
   `ImageError::UnsupportedMime`.
2. **Validate** — `image::load_from_memory_with_format(...)` decodes;
   yields `(w, h, ColorType)`. Catch decoder panics with a
   `std::panic::catch_unwind` guard — `image` is normally panic-safe
   but defense in depth costs little.
3. **Size cap** — if `bytes.len() > max_bytes`:
   - downscale via `image::imageops::resize(..., Lanczos3)` to the
     largest size such that `longest_edge <= downscale_target`.
   - re-encode with the original MIME (or `image/jpeg` quality 85 for
     PNG > 5 MB after resize — operator override available).
   - emit a `WARN`-level `tracing` event with original / new dims +
     bytes; TUI surfaces a "[downscaled]" badge on the thumbnail.
4. **Hash** — `sha256(bytes)` after any resize.
5. **Encode** — base64 the (possibly resized) bytes; build `ImageBlock`.

The pipeline runs synchronously on the input task; for very large images
(>10 MB, downscale path) we `tokio::task::spawn_blocking` to keep the
TUI responsive.

### Supported MIME types

`image/png`, `image/jpeg`, `image/gif`, `image/webp`. Anything else is
rejected at sniff time. The list is mirrored in
`caliban-provider::vision::SUPPORTED_IMAGE_MIME` so adapters can validate
inbound images at serialization time as a belt-and-braces check.

### Size and dimension caps

| Knob               | Default       | Source                            |
| ------------------ | ------------- | --------------------------------- |
| `max_bytes`        | 5 MiB         | pre-base64; matches Anthropic limit |
| `downscale_target` | 1568 px       | Anthropic recommended max edge    |
| `max_count_per_message` | 20       | matches OpenAI/Anthropic ceilings |
| `max_total_bytes_per_message` | 30 MiB | safety net                |

Configurable via `[images]` in `caliban.toml`:

```toml
[images]
max_bytes = 5_000_000
downscale_target = 1568
max_count_per_message = 20
max_total_bytes_per_message = 30_000_000
```

## Input surfaces

### Clipboard paste

`arboard::Clipboard::new()` then `get_image()`. The TUI input layer
binds `Ctrl+V` (configurable; macOS users want `Cmd+V` if the
terminal forwards it). On `get_image() == Ok(img)`:

1. Convert `arboard::ImageData { bytes: ColorImage, width, height }`
   to a PNG via `image::ImageBuffer::from_raw(...).write_to(...,
   ImageFormat::Png)`.
2. Feed bytes through `Pipeline::ingest` with `hint_mime = Some("image/png")`.
3. Insert an inline image token at the cursor in the input buffer
   (`{img:<sha256-12>}`); the TUI's renderer expands this to a
   thumbnail when drawing the prompt line.

On `get_image() == Err(NoImage)` we fall through to the text-paste
handler (existing behavior).

### `@path/file.png` mention

The TUI file-mention autocomplete (existing source, from the TUI
ergonomics spec) gains image-aware behavior:

- When the resolved path ends in a supported image extension, the
  insertion is a `{img:<sha>}` token instead of a `@path/file.png` text
  span.
- The file is read and passed through `Pipeline::ingest` synchronously
  on insertion (so the user sees an immediate thumbnail).
- Errors (file too big, unsupported type, decode failure) show in the
  status line and the `@path/file.png` literal stays in the buffer for
  the user to fix.

### Drag-and-drop

Terminals signal DnD via escape sequences; we parse:

- **kitty / wezterm:** `OSC 52` followed by file path list.
- **iTerm2:** `ESC ] 1337 ; File = ... BEL` with inline base64.
- **bracketed paste with `file://` URLs:** Linux GTK terminals.

Detection logic in `caliban-images::dnd`. On match, each path is fed
through the `@path/file.png` pipeline. Unrecognized DnD events fall
through to the standard input handler.

### CLI `--image <path>`

```
caliban -p "describe this image" --image diagram.png --image screenshot.jpg
```

`--image` can repeat; each value is fed through `Pipeline::ingest`
before the request is built. In `--print` headless mode this is the
primary input surface.

## TUI rendering

### Thumbnails inline

Detection cascade (cached in `caliban-tui::caps::GRAPHICS_PROTOCOL`):

1. `$TERM` / `$TERM_PROGRAM` heuristics.
2. Live capability probe — send `\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\`
   (kitty query), wait 100ms for a response; same for sixel.
3. Fallback: text placeholder.

Rendering paths:

| Protocol | Renderer                                             |
| -------- | ---------------------------------------------------- |
| kitty    | `\x1b_Ga=T,f=100,...\x1b\\` with base64 PNG payload  |
| sixel    | `image -> sixel` via the `sixel` crate                |
| iTerm2   | `\x1b]1337;File=inline=1;...\a` with base64           |
| none     | `[image: 1024x768 PNG  diagram.png]` (filename if known) |

The transcript line height for an image is capped at 8 cells; larger
images render at `image-scaled-to-8-rows`. A `Ctrl+O` transcript viewer
(future) will show full-resolution.

### Input-buffer thumbnails

Inline `{img:<sha>}` tokens render as a 2-row x 4-col tile in the input
area, with the index and a short hash. Backspace next to a tile
removes the whole attachment. The TUI renders the prompt line in two
stripes (upper = image tiles, lower = text) when any image is present.

## Capability filtering

Lives in the model router (v2 spec §"Capability filtering"). Summary:

- `derived_needs(req)` checks `req.messages.iter().any(has_image_block)`;
  if true, `requires.vision = Some(true)` is added to the candidate
  filter.
- Routes whose declared `Capabilities.vision == false` (per
  `Provider::capabilities(model)`) are dropped.
- If the candidate set ends up empty, the request short-circuits with
  `RouterError::NoCandidate { needs: { vision: true, .. } }`. The
  CLI/TUI surfaces this with "no vision-capable route configured for
  purpose `<P>`; configure one or remove the image".

### Text fallback (when capability filtering disabled)

In `--strict-routing=false` mode (env var `CALIBAN_STRICT_ROUTING=false`,
default `true`), if no vision route is available the router falls back
to a non-vision route and the image is replaced with:

> `[image attached — provider does not support vision; dims: 1024x768; filename: diagram.png]`

Operators who want loud failures keep the default; CI/headless users who
want degraded-but-progressing keep going.

## Provider adapter mapping

### Anthropic

```json
{ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0..." } }
```

`cache_control` markers are preserved on the image block when set.

### OpenAI (Responses API)

```json
{ "type": "input_image", "image_url": "data:image/png;base64,iVBORw0..." }
```

`detail` field is not exposed by caliban v1; we send the default (`auto`).
`image_url` with an HTTP URL is supported (`ImageSource::Url`) so
operators can reference S3 objects without re-uploading.

### Google Gemini

```json
{ "inline_data": { "mime_type": "image/png", "data": "iVBORw0..." } }
```

Gemini also supports `file_data` for Cloud-Storage URIs; v1 emits
`inline_data` only.

### Ollama

`Capabilities.vision = false` for v1. Some Ollama models (LLaVA) do
support images via a non-standard `images: [base64]` field; we leave
this for a v1.1 follow-up once an Ollama model selection mechanism
exposes vision-capable models.

## Cost accounting

The router (v2) records:

```
token.usage { kind = "image", route_id, sha256, dims }     counter+1 per image
token.usage { kind = "image_input_tokens", route_id }      counter+=N  (approx, see below)
```

Anthropic and OpenAI return token usage that includes image tokens
implicitly — we read the response's `usage.input_tokens` as-is. For
diagnostic-only "what was this image worth", we estimate
`image_input_tokens = ceil(w * h / 750)` (Anthropic's documented
heuristic for default-detail); this is approximate and clearly labelled.
The actual billed-tokens come from `usage`.

The `/usage` overlay grows an "Images" row:

```
Images:  3 attached  est. 1,847 vision tokens  3.0 MiB on wire
```

## Checkpointing

Sessions (`caliban-sessions`) carry images as `ImageBlock` content with
`ImageSource::Base64`. To keep `session.json` sizes manageable:

- Each unique `sha256` is written once to
  `<session_dir>/blobs/<sha256>.bin` (raw, post-downscale).
- Inside `session.json`, the image content block is serialized with
  `ImageSource::BlobRef { sha256 }` (a third variant, session-only — never
  sent to providers). On load, the session loader resolves
  `BlobRef → Base64` by reading the blob file.

`ImageSource::BlobRef` is gated by a `#[serde(skip_serializing_if = ...)]`
to ensure it never accidentally serializes out to the wire to a provider.

A `caliban session gc <id>` command (future) prunes blobs not
referenced by any persisted message; v1 ships without GC and just leaves
blobs alongside the session.

## Permissions

Image attachments are permission-free at the *attach* layer (it's user
input, like text). They become subject to the `Image(<purpose>)` rule
when sent to a provider, which is implicit on `MainLoop` purposes (the
default Allow rule covers it). Operators who want explicit policy can
add:

```toml
[[permissions.deny]]
match = "image"
purposes = ["sub_agent"]   # don't ship images to background sub-agents
```

This is a small extension of the existing rule grammar (ADR 0020) — a
new `match = "image"` predicate; lifted from the in-spec extensions.

## Testing strategy

15 enumerated tests:

**`caliban-images` unit tests:**

1. `ingest_png_under_cap_passes_through`
2. `ingest_oversized_png_downscales_to_target`
3. `ingest_rejects_unsupported_mime_with_clear_error`
4. `ingest_computes_stable_sha256_across_ingestions`
5. `ingest_recovers_from_truncated_jpeg_with_decode_error`
6. `pipeline_respects_per_message_count_cap`
7. `clipboard_paste_round_trip_png` (gated on `feature = "clipboard"`,
   skipped in CI without DISPLAY)
8. `dnd_kitty_escape_parsed_into_path_list`

**Provider adapter serialization:**

9. `anthropic_image_block_serializes_to_base64_source`
10. `openai_image_block_serializes_to_data_url`
11. `gemini_image_block_serializes_to_inline_data`

**Router capability filtering (covered by model-router v2 tests too, but
asserted end-to-end here):**

12. `request_with_image_drops_non_vision_routes`
13. `request_with_image_and_no_vision_route_returns_no_candidate`
14. `strict_routing_false_replaces_image_with_text_placeholder`

**Session + checkpointing:**

15. `session_serializes_image_as_blob_ref_not_base64`
16. `session_load_round_trips_blob_back_to_base64`

**TUI:**

17. `at_mention_of_png_path_inserts_image_token`
18. `terminal_caps_probe_falls_back_to_text_placeholder_when_no_protocol`

Total: ~18 tests across crates.

## Risks

- **Decoder vulnerabilities.** The `image` crate has had CVEs.
  Mitigation: pin a recent version; restrict features to png/jpeg/gif/
  webp; wrap decode in `catch_unwind`; consider running ingest in a
  sandboxed child once OS-sandbox lands.
- **Clipboard libraries are platform-touchy.** `arboard` is robust but
  X11/Wayland clipboards have edge cases (selection vs. clipboard).
  Mitigation: feature-gate behind `clipboard`; document
  `WAYLAND_DISPLAY` requirement; surface clipboard-empty as a polite
  no-op.
- **Terminal graphics protocols are heterogeneous.** Detection probes
  hang on some terminals. Mitigation: 100ms hard timeout; cache the
  result per session; offer `CALIBAN_GRAPHICS=kitty|sixel|none` override.
- **Cost surprise.** A single 4K screenshot can burn ~2k input tokens
  on Anthropic. Mitigation: downscale to 1568 px by default; surface
  `est. tokens` in `/usage`; warn at attach time when est > 5k.
- **Image identity across edits.** A user editing a screenshot and
  pasting it again should re-ingest, not hit the dedupe path. SHA-256
  over post-resize bytes handles this — different pixels, different
  hash — but operators may be surprised when pixel-identical screenshots
  dedupe. Mitigation: document the hash semantics.
- **Provider drift.** OpenAI/Gemini may change image API shape. Mit:
  each adapter owns its serialization; integration tests recorded
  against pinned API versions (`wiremock` fixtures).
- **Anthropic image prompt-cache** — caching image content requires
  `cache_control` on the image block. We honor it when present on the
  IR; we do *not* auto-cache. Operators opt in via a future
  `request.metadata.cache_images = true` knob.

## Acceptance criteria

- `cargo build --workspace --features images/clipboard` clean;
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- ≥15 new tests passing across crates.
- `caliban-provider::ImageBlock` + `ContentBlock::Image` shipped; all
  four adapters serialize correctly.
- TUI: clipboard paste, `@path/file.png` mention, kitty/sixel/text
  thumbnail rendering all functional in a manual test on macOS Terminal
  (text fallback), iTerm2 (inline), and kitty (graphics).
- CLI `--image <path>` repeatable and works in `--print` mode.
- Router capability filter drops non-vision routes when an image is
  present; `--strict-routing=false` substitutes the text placeholder
  with the documented format.
- `/usage` overlay shows the "Images" row with attached count, est.
  tokens, on-wire bytes.
- Session persistence: images write to `<session>/blobs/<sha>.bin`;
  reload reconstitutes the base64 view; on-wire serialization never
  emits `BlobRef`.
- Matrix E "image / vision input" row moves 🔴 → ✅ in the PR that
  lands this work.
- ADR 0039 in `accepted` status.
