//! Image ingest pipeline for the caliban agent harness.
//!
//! Implements ADR 0039: clipboard / `@path` / drag-and-drop attach, MIME
//! sniffing, decode and Lanczos3 downscale, SHA-256 fingerprinting,
//! session blob storage, graphics-protocol detection for the TUI, and a
//! per-image token-cost heuristic.
//!
//! Companion spec: `docs/superpowers/specs/2026-05-24-image-input-design.md`.

pub mod blob;
#[cfg(feature = "clipboard")]
pub mod clipboard;
pub mod cost;
pub mod dnd;
pub mod graphics;
pub mod pipeline;
pub mod placeholder;
pub mod routing;

pub use blob::{BlobStore, BlobStoreError};
#[cfg(feature = "clipboard")]
pub use clipboard::{ClipboardError, ingest_clipboard_pixels, paste_image_from_clipboard};
pub use cost::image_to_tokens;
pub use dnd::{DragDropPayload, parse_drag_drop_escape};
pub use graphics::{GraphicsProtocol, detect_graphics_protocol, render_for_protocol};
pub use pipeline::{IngestError, IngestResult, Pipeline, sha256_hex, sniff_mime};
pub use placeholder::text_placeholder;
pub use routing::{rewrite_for_text_fallback, strict_routing_enabled};

/// Maximum image size in bytes before downscaling (5 MiB).
pub const DEFAULT_MAX_BYTES: usize = 5 * 1024 * 1024;
/// Default longest-edge resize target (Anthropic recommendation).
pub const DEFAULT_DOWNSCALE_TARGET: u32 = 1568;
/// Supported image MIME types (closed allowlist).
pub const SUPPORTED_MIME: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Returns `true` if `mime` is one of the allowed image MIME types.
#[must_use]
pub fn is_supported_mime(mime: &str) -> bool {
    SUPPORTED_MIME.iter().any(|m| m.eq_ignore_ascii_case(mime))
}

/// Returns `true` if `path` has an image-flavored file extension.
#[must_use]
pub fn path_is_image_like(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}
