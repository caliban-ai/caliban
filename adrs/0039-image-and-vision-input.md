# ADR 0039 · Image + vision input

- **Status:** proposed
- **Date:** 2026-05-24
- **Spec:** `docs/superpowers/specs/2026-05-24-image-input-design.md`
- **Author:** john.ford2002@gmail.com

## Context

Vision is table-stakes for any modern coding assistant: users want to
paste a screenshot of a stack trace, drop a Figma export, or `@path` a
generated chart and get it through to a vision-capable model. Caliban's
current `ContentBlock` IR is text-only; the TUI input layer has no paste
handler beyond text; the provider adapters serialize text content
exclusively. Closing this requires changes across five crates plus a
new ingest crate, but each change is small and the design carries no
provider lock-in. Capability filtering (model-router v2) already
contemplates a vision predicate — this ADR makes it real.

## Decision

### `ContentBlock` IR gains an `Image` variant; `ImageBlock` is provider-agnostic

The IR carries `{ source, mime, sha256, dims, cache_control }`. `source`
is `Base64 { data }` or `Url { url }`. Provider adapters own the
serialization to their native shape (Anthropic `image`, OpenAI
`image_url`, Google `inline_data`). This keeps the IR free of provider-
specific knobs and lets us add a new provider's image shape by writing
exactly the adapter, with no IR churn.

### Ingest is a separate crate, `caliban-images`

Clipboard reads, DnD escape parsing, MIME sniffing, decode validation,
size cap enforcement, downscale, and SHA-256 fingerprinting all live in
one crate. The TUI and CLI both depend on it; the model-router pulls
nothing from it (router only sees the already-built `ImageBlock` IR).
Crate separation matters because the `image` decoder family is the
biggest CVE surface in the dependency tree — keeping it behind a single
boundary makes audits and feature-gating tractable.

### MIME allowlist is closed: png, jpeg, gif, webp

We explicitly disable bmp/tiff/dds/tga and friends in the `image` crate
feature flags. AVIF/HEIC are tracked but not v1. The list mirrors what
all three vision providers (Anthropic, OpenAI, Google) support; expanding
later is a config-flag change, not an API change.

### Default size cap: 5 MiB pre-base64, downscale to 1568 px on longest edge

5 MiB matches Anthropic's documented limit; 1568 px matches Anthropic's
recommended longest edge for cost-efficient inputs. Over-cap images are
downscaled (Lanczos3) with a `WARN`-level trace and a "[downscaled]"
badge on the TUI thumbnail. Operators can override via `[images]` in
`caliban.toml`.

### Capability filtering is mandatory; `CALIBAN_STRICT_ROUTING=false` opts out

By default, an image-bearing request that has no vision-capable route
fails with `RouterError::NoCandidate`. Operators who want degraded
behavior (CI, headless) set `CALIBAN_STRICT_ROUTING=false`; the router
replaces image content with a documented text placeholder and continues.
We pick "strict by default" because silent vision drop is a worse
failure mode than a clear error pointing at the missing route.

### Sessions store images as blob refs, never as inline base64

`session.json` carries `ImageSource::BlobRef { sha256 }`; the actual
bytes live in `<session>/blobs/<sha>.bin`. `BlobRef` has `#[serde(skip_…)]`
guarding against accidental wire serialization. This keeps session
files small, makes git history of `.caliban/sessions` readable, and
sets up the future `session gc` command.

### TUI graphics protocol is detected once per session, with a text fallback

We probe kitty/sixel/iTerm2 capability via short escape sequences with
a 100ms timeout, cache the result, and fall through to a
`[image: WxH MIME filename]` placeholder otherwise. Probe results are
overridable via `CALIBAN_GRAPHICS=kitty|sixel|iterm|none`. Probes hang on
some terminals; the timeout is the safety valve.

### Cost accounting reads provider `Usage`, not local estimates, but estimates surface in `/usage` for diagnostics

Anthropic and OpenAI return token usage including image tokens. We bill
from what providers report. Locally we *also* compute a labelled
estimate (`ceil(w * h / 750)` for Anthropic-style billing) so the
`/usage` overlay can answer "what's this image roughly worth"
*before* the call returns.

## Consequences

- **Positive.** Closes matrix E "image / vision input" with one PR.
  The IR change is a small additive variant on `ContentBlock`; existing
  handlers are unaffected (default-match arm). Capability filtering in
  the router (already designed in v2) gets its first real consumer.
  Pasting a screenshot into caliban "just works" with the right route
  configured. The `caliban-images` crate establishes a pattern for
  future media types (PDF, audio).
- **Negative.** Five crates touched. `image` crate dependency adds
  decoder CVE surface; we constrain it but cannot eliminate it. The
  TUI gains a graphics-protocol detection path that has been a recurring
  source of bugs in other tools — we mitigate with caching + override
  but accept some carry. Cost surprise is real for large screenshots;
  the 1568 px downscale default helps but does not eliminate it.
  Strict-by-default routing will trip operators who configured a non-
  vision route as their default — clear error message + docs are the
  mitigation. Session blob storage adds a directory layout we must
  GC eventually.
- **Revisit if:** Output-side vision (image generation) becomes a real
  capability across providers — extend the IR with `ImageGeneration` /
  similar. If the `image` crate accumulates serious CVEs, sandbox the
  ingest path in a separate process. If users routinely hit the
  per-message count cap (20), expose it directly in the TUI rather
  than via `caliban.toml`.
