//! The end-to-end image ingest pipeline: sniff → decode → downscale → hash.

use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use caliban_provider::{ImageBlock, ImageSource};
use image::{ImageFormat, ImageReader, imageops::FilterType};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{DEFAULT_DOWNSCALE_TARGET, DEFAULT_MAX_BYTES, SUPPORTED_MIME, is_supported_mime};

/// Errors raised by the ingest pipeline.
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    /// MIME could not be inferred or was outside the allowlist.
    #[error("unsupported image MIME (allowed: {})", SUPPORTED_MIME.join(", "))]
    UnsupportedMime,
    /// Decode failed — corrupt or truncated input.
    #[error("image decode failed: {0}")]
    DecodeFailed(String),
    /// Re-encode after downscale failed.
    #[error("image re-encode failed: {0}")]
    EncodeFailed(String),
}

/// Image ingest pipeline.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Pipeline {
    /// Pre-base64 size cap. Above this (or above dimension cap), the image
    /// is downscaled + re-encoded.
    pub max_bytes: usize,
    /// Longest-edge target pixel count for downscale.
    pub downscale_target: u32,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
            downscale_target: DEFAULT_DOWNSCALE_TARGET,
        }
    }
}

/// Result of running the pipeline on raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestResult {
    /// The (possibly resized) bytes.
    pub bytes: Vec<u8>,
    /// MIME of [`Self::bytes`].
    pub mime: String,
    /// SHA-256 of [`Self::bytes`].
    pub sha256: String,
    /// (width, height) of the (possibly resized) image.
    pub dims: (u32, u32),
    /// `true` if the pipeline had to downscale + re-encode.
    pub was_downscaled: bool,
}

impl IngestResult {
    /// Build a [`caliban_provider::ImageBlock`] with `Base64` source.
    #[must_use]
    pub fn into_block(self) -> ImageBlock {
        let data = BASE64.encode(&self.bytes);
        ImageBlock {
            source: ImageSource::Base64 {
                media_type: self.mime,
                data,
            },
            cache_control: None,
            sha256: Some(self.sha256),
            dims: Some(self.dims),
        }
    }
}

impl Pipeline {
    /// Create a pipeline with default caps.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Run the pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::UnsupportedMime`] if the input bytes' inferred
    /// MIME (or `hint_mime`, as fallback) is not in the allowlist.
    /// Returns [`IngestError::DecodeFailed`] if the bytes aren't a valid image
    /// the `image` crate can read. Returns [`IngestError::EncodeFailed`] when
    /// the post-resize encode step fails.
    pub fn ingest(
        &self,
        bytes: Vec<u8>,
        hint_mime: Option<&str>,
    ) -> Result<IngestResult, IngestError> {
        // 1. MIME sniff
        let mime = sniff_mime(&bytes, hint_mime).ok_or(IngestError::UnsupportedMime)?;

        // 2. Decode + dims
        let format = mime_to_format(&mime).ok_or(IngestError::UnsupportedMime)?;
        let reader = ImageReader::with_format(Cursor::new(&bytes), format);
        let img = reader
            .decode()
            .map_err(|e| IngestError::DecodeFailed(e.to_string()))?;
        let (w, h) = (img.width(), img.height());

        // 3. Size cap → downscale + re-encode if needed
        let too_big = bytes.len() > self.max_bytes || w.max(h) > self.downscale_target;
        if too_big {
            let target = self.downscale_target.max(1);
            let longest = w.max(h);
            let scale = f64::from(target) / f64::from(longest);
            let width_f = (f64::from(w) * scale).round();
            let height_f = (f64::from(h) * scale).round();
            // f64 → u32 with a safety floor at 1.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let nw = (width_f as u32).max(1);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let nh = (height_f as u32).max(1);

            let resized = img.resize_exact(nw, nh, FilterType::Lanczos3);
            let mut buf: Vec<u8> = Vec::new();
            // For very large PNGs, switch to JPEG-85 on re-encode to keep
            // the result under the size cap.
            let out_format =
                if matches!(format, ImageFormat::Png) && bytes.len() > self.max_bytes * 2 {
                    ImageFormat::Jpeg
                } else {
                    format
                };
            resized
                .write_to(&mut Cursor::new(&mut buf), out_format)
                .map_err(|e| IngestError::EncodeFailed(e.to_string()))?;
            let new_mime = format_to_mime(out_format).to_string();
            tracing::warn!(
                target: "caliban::images",
                orig_bytes = bytes.len(),
                new_bytes = buf.len(),
                orig_dims = ?(w, h),
                new_dims = ?(nw, nh),
                "downscaled image",
            );
            let sha256 = sha256_hex(&buf);
            return Ok(IngestResult {
                bytes: buf,
                mime: new_mime,
                sha256,
                dims: (nw, nh),
                was_downscaled: true,
            });
        }

        // 4. Hash unchanged bytes
        let sha256 = sha256_hex(&bytes);
        Ok(IngestResult {
            bytes,
            mime,
            sha256,
            dims: (w, h),
            was_downscaled: false,
        })
    }
}

/// Sniff the image MIME type from a leading byte signature.
///
/// Returns `None` if neither the inferred type nor `hint_mime` is in the
/// allowlist.
#[must_use]
pub fn sniff_mime(bytes: &[u8], hint_mime: Option<&str>) -> Option<String> {
    if let Some(t) = infer::get(bytes) {
        let m = t.mime_type();
        if is_supported_mime(m) {
            return Some(m.to_string());
        }
    }
    match hint_mime {
        Some(m) if is_supported_mime(m) => Some(m.to_string()),
        _ => None,
    }
}

fn mime_to_format(mime: &str) -> Option<ImageFormat> {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::WebP),
        _ => None,
    }
}

fn format_to_mime(fmt: ImageFormat) -> &'static str {
    match fmt {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Lowercase hex SHA-256.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ColorType, ImageEncoder, codecs::png::PngEncoder};

    /// Build a minimal valid PNG of given dims.
    fn make_png(w: u32, h: u32) -> Vec<u8> {
        let pixels = vec![0u8; (w * h * 3) as usize];
        let mut buf = Vec::new();
        PngEncoder::new(&mut buf)
            .write_image(&pixels, w, h, ColorType::Rgb8.into())
            .expect("png encode");
        buf
    }

    fn make_jpeg(w: u32, h: u32) -> Vec<u8> {
        let pixels = vec![128u8; (w * h * 3) as usize];
        let mut buf = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85)
            .write_image(&pixels, w, h, ColorType::Rgb8.into())
            .expect("jpeg encode");
        buf
    }

    fn make_gif(w: u32, h: u32) -> Vec<u8> {
        let pixels = vec![0u8; (w * h * 4) as usize];
        let mut buf = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut buf);
            let frame = image::Frame::new(image::RgbaImage::from_raw(w, h, pixels).expect("frame"));
            enc.encode_frame(frame).expect("gif encode");
        }
        buf
    }

    fn make_webp(w: u32, h: u32) -> Vec<u8> {
        let pixels = vec![64u8; (w * h * 3) as usize];
        let img = image::RgbImage::from_raw(w, h, pixels).expect("rgb img");
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::WebP)
            .expect("webp encode");
        buf
    }

    #[test]
    fn sniffs_png() {
        let png = make_png(2, 2);
        let mime = sniff_mime(&png, None).expect("png mime");
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn sniffs_jpeg() {
        let jpeg = make_jpeg(2, 2);
        let mime = sniff_mime(&jpeg, None).expect("jpeg mime");
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn sniffs_gif() {
        let gif = make_gif(2, 2);
        let mime = sniff_mime(&gif, None).expect("gif mime");
        assert_eq!(mime, "image/gif");
    }

    #[test]
    fn sniffs_webp() {
        let webp = make_webp(2, 2);
        let mime = sniff_mime(&webp, None).expect("webp mime");
        assert_eq!(mime, "image/webp");
    }

    #[test]
    fn rejects_unsupported_mime() {
        // Plain text — not an image.
        let txt = b"hello world".to_vec();
        let res = Pipeline::new().ingest(txt, None);
        assert!(matches!(res, Err(IngestError::UnsupportedMime)));
    }

    #[test]
    fn ingest_under_cap_passes_through() {
        let png = make_png(64, 32);
        let orig_len = png.len();
        let result = Pipeline::new().ingest(png, None).expect("ingest");
        assert!(!result.was_downscaled);
        assert_eq!(result.dims, (64, 32));
        assert_eq!(result.bytes.len(), orig_len);
    }

    #[test]
    fn ingest_oversized_dims_downscales_preserving_aspect() {
        // 2000x1000 → max edge > 1568 default target → must downscale.
        let png = make_png(2000, 1000);
        let result = Pipeline::new().ingest(png, None).expect("ingest");
        assert!(result.was_downscaled, "expected downscale path");
        let (w, h) = result.dims;
        assert_eq!(w, 1568, "longest edge should hit target");
        // Aspect ratio (2:1) preserved within ±1px.
        assert!(
            (i64::from(h) - i64::from(w / 2)).abs() <= 1,
            "aspect not preserved: {w}x{h}",
        );
    }

    #[test]
    fn ingest_oversized_bytes_re_encodes_smaller() {
        // 2500x2500 generates plenty of bytes; the result post-resize must
        // be strictly smaller in dims.
        let png = make_png(2500, 2500);
        let orig_len = png.len();
        let result = Pipeline::new().ingest(png, None).expect("ingest");
        assert!(result.was_downscaled);
        assert!(
            result.bytes.len() < orig_len,
            "re-encoded image not smaller: {} → {}",
            orig_len,
            result.bytes.len()
        );
        assert_eq!(result.dims, (1568, 1568));
    }

    #[test]
    fn sha256_dedupes_identical_images() {
        let png_a = make_png(16, 16);
        let png_b = make_png(16, 16);
        let ra = Pipeline::new().ingest(png_a, None).expect("a");
        let rb = Pipeline::new().ingest(png_b, None).expect("b");
        assert_eq!(ra.sha256, rb.sha256);
        let png_c = make_png(17, 17);
        let rc = Pipeline::new().ingest(png_c, None).expect("c");
        assert_ne!(ra.sha256, rc.sha256);
    }

    #[test]
    fn truncated_image_yields_decode_error() {
        let png = make_png(16, 16);
        // Keep PNG signature so MIME sniff passes; truncate the rest.
        let mut truncated = png;
        truncated.truncate(20);
        let res = Pipeline::new().ingest(truncated, Some("image/png"));
        assert!(matches!(res, Err(IngestError::DecodeFailed(_))));
    }

    #[test]
    fn ingest_result_into_block_carries_sha256_and_dims() {
        let png = make_png(8, 8);
        let result = Pipeline::new().ingest(png, None).expect("ingest");
        let sha = result.sha256.clone();
        let dims = result.dims;
        let block = result.into_block();
        assert_eq!(block.sha256.as_deref(), Some(sha.as_str()));
        assert_eq!(block.dims, Some(dims));
        match block.source {
            ImageSource::Base64 { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert!(!data.is_empty());
            }
            _ => panic!("expected Base64 source"),
        }
    }
}
