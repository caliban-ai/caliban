//! Clipboard image ingest via the `arboard` crate.
//!
//! Feature-gated behind `clipboard` so the rest of the crate can be used
//! headlessly (CI, sub-agent fleets) without pulling X11 / Wayland
//! development headers.

use std::io::Cursor;

use image::{ColorType, ImageEncoder, codecs::png::PngEncoder};

use crate::pipeline::{IngestError, IngestResult, Pipeline};

/// Errors raised by [`paste_image_from_clipboard`].
#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    /// `arboard` couldn't open the system clipboard.
    #[error("clipboard open failed: {0}")]
    OpenFailed(String),
    /// Clipboard does not contain an image.
    #[error("clipboard has no image")]
    NoImage,
    /// Failed to re-encode the clipboard pixels as PNG.
    #[error("clipboard PNG encode failed: {0}")]
    EncodeFailed(String),
    /// The downstream ingest pipeline rejected the image.
    #[error(transparent)]
    Ingest(#[from] IngestError),
}

/// Try to read an image from the system clipboard, run it through the
/// pipeline, and return an [`IngestResult`].
///
/// # Errors
///
/// Surfaces clipboard open failures, "no image on clipboard" empty states,
/// PNG re-encode errors, and pipeline errors (unsupported MIME, decode
/// failures).
pub fn paste_image_from_clipboard(pipeline: &Pipeline) -> Result<IngestResult, ClipboardError> {
    let mut cb =
        arboard::Clipboard::new().map_err(|e| ClipboardError::OpenFailed(e.to_string()))?;
    let img = match cb.get_image() {
        Ok(i) => i,
        Err(arboard::Error::ContentNotAvailable) => return Err(ClipboardError::NoImage),
        Err(e) => return Err(ClipboardError::OpenFailed(e.to_string())),
    };
    let arboard::ImageData {
        width,
        height,
        bytes,
    } = img;
    let w = u32::try_from(width).unwrap_or(u32::MAX);
    let h = u32::try_from(height).unwrap_or(u32::MAX);

    // arboard hands us RGBA8. Encode to PNG so the pipeline can decode + sniff.
    let mut png_bytes: Vec<u8> = Vec::new();
    PngEncoder::new(&mut png_bytes)
        .write_image(&bytes, w, h, ColorType::Rgba8.into())
        .map_err(|e| ClipboardError::EncodeFailed(e.to_string()))?;
    let _ = Cursor::new(&png_bytes); // appease unused-import lints across cfgs
    let result = pipeline.ingest(png_bytes, Some("image/png"))?;
    Ok(result)
}

/// A test-friendly variant: same logic but the caller supplies the raw
/// pixel bytes + dimensions, side-stepping the live clipboard. Used by the
/// crate's clipboard happy-path test.
///
/// # Errors
///
/// Same as [`paste_image_from_clipboard`] minus the clipboard-open errors.
pub fn ingest_clipboard_pixels(
    pipeline: &Pipeline,
    width: u32,
    height: u32,
    rgba_bytes: &[u8],
) -> Result<IngestResult, ClipboardError> {
    let mut png_bytes: Vec<u8> = Vec::new();
    PngEncoder::new(&mut png_bytes)
        .write_image(rgba_bytes, width, height, ColorType::Rgba8.into())
        .map_err(|e| ClipboardError::EncodeFailed(e.to_string()))?;
    let result = pipeline.ingest(png_bytes, Some("image/png"))?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_clipboard_pixels_round_trips_rgba_via_png() {
        // 4x4 RGBA mock clipboard image (red).
        let pixels: Vec<u8> = (0..16).flat_map(|_| [0xff, 0x00, 0x00, 0xff]).collect();
        let pipeline = Pipeline::new();
        let result = ingest_clipboard_pixels(&pipeline, 4, 4, &pixels).expect("clipboard ingest");
        assert_eq!(result.dims, (4, 4));
        assert_eq!(result.mime, "image/png");
        assert!(!result.was_downscaled);
        assert_eq!(result.sha256.len(), 64);
    }
}
