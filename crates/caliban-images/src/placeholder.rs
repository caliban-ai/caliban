//! Text placeholder rendering for non-graphics terminals.

/// Render `[image: WxH MIME, KB]` for a non-graphics terminal.
///
/// `filename` is an optional hint shown in brackets when known.
#[must_use]
pub fn text_placeholder(
    dims: (u32, u32),
    mime: &str,
    bytes_len: usize,
    filename: Option<&str>,
) -> String {
    let kb = bytes_len / 1024;
    let kind = mime
        .strip_prefix("image/")
        .unwrap_or(mime)
        .to_ascii_uppercase();
    match filename {
        Some(f) => format!("[image: {}x{} {kind}, {kb} KB, {f}]", dims.0, dims.1),
        None => format!("[image: {}x{} {kind}, {kb} KB]", dims.0, dims.1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dims_mime_size() {
        let s = text_placeholder((1024, 768), "image/png", 234 * 1024, None);
        assert_eq!(s, "[image: 1024x768 PNG, 234 KB]");
    }

    #[test]
    fn includes_filename_when_present() {
        let s = text_placeholder((640, 480), "image/jpeg", 50 * 1024, Some("photo.jpg"));
        assert_eq!(s, "[image: 640x480 JPEG, 50 KB, photo.jpg]");
    }

    #[test]
    fn handles_unknown_mime() {
        let s = text_placeholder((1, 1), "application/x-thing", 1024, None);
        assert!(s.contains("APPLICATION/X-THING") || s.contains("application/x-thing"));
    }
}
