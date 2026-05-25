//! Terminal drag-and-drop escape decoding.
//!
//! Terminals signal drop events with terminal-specific escape sequences:
//!
//! - **kitty / wezterm:** bracketed paste of `file://` URLs or a literal
//!   absolute path followed by a newline.
//! - **iTerm2:** `ESC ] 1337 ; File = name=...;inline=1;size=... : <base64> BEL`
//!   inline-file protocol.
//!
//! We parse both shapes into a [`DragDropPayload`] for the TUI input layer
//! to consume.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

/// Result of a successful escape-sequence parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DragDropPayload {
    /// A file-path drop (kitty / wezterm / GNOME `file://` URI).
    Path(String),
    /// An iTerm2 inline-file drop carrying bytes directly.
    InlineFile {
        /// Optional filename hint from the iTerm2 envelope.
        name: Option<String>,
        /// The raw decoded bytes.
        bytes: Vec<u8>,
    },
}

/// Parse a drag-and-drop escape sequence.
///
/// Returns `None` if the input does not match any known drag-and-drop
/// signature. The input is the raw bytes the terminal delivered, e.g.
/// through bracketed paste or directly via the input stream. Covers all
/// known signal shapes (kitty / wezterm / iTerm2 / GTK).
#[must_use]
pub fn parse_drag_drop_escape(input: &str) -> Option<DragDropPayload> {
    // iTerm2: ESC ] 1337 ; File = ... : <base64> BEL
    // ESC = \x1b, BEL = \x07
    if let Some(rest) = strip_prefix_lit(input, "\x1b]1337;File=") {
        let (params_and_data, _) = split_once_byte(rest, b'\x07')?;
        let (params, b64) = params_and_data.split_once(':')?;
        let name = parse_iterm2_name(params);
        let bytes = BASE64.decode(b64.trim().as_bytes()).ok()?;
        return Some(DragDropPayload::InlineFile { name, bytes });
    }

    // kitty / wezterm / GTK: bare path or file:// URI delivered with newline
    // terminator. We accept either a single line or a leading-whitespace-
    // stripped fragment.
    let line = input.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    if let Some(path) = line.strip_prefix("file://") {
        return Some(DragDropPayload::Path(url_decode(path)));
    }
    // Heuristic: absolute path → treat as DnD.
    if line.starts_with('/') || (line.len() > 2 && line.as_bytes()[1] == b':') {
        return Some(DragDropPayload::Path(line.to_string()));
    }
    None
}

fn strip_prefix_lit<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.strip_prefix(prefix)
}

fn split_once_byte(s: &str, b: u8) -> Option<(&str, &str)> {
    let idx = s.as_bytes().iter().position(|&x| x == b)?;
    Some((&s[..idx], &s[idx + 1..]))
}

fn parse_iterm2_name(params: &str) -> Option<String> {
    for kv in params.split(';') {
        if let Some((k, v)) = kv.split_once('=')
            && k.eq_ignore_ascii_case("name")
        {
            // The name is base64-encoded in the iTerm2 protocol.
            return BASE64
                .decode(v.trim().as_bytes())
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok());
        }
    }
    None
}

fn url_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(char::from(byte));
                i += 3;
                continue;
            }
        }
        out.push(char::from(bytes[i]));
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_bare_path() {
        let drop = "/Users/me/diagram.png\n";
        let payload = parse_drag_drop_escape(drop).expect("payload");
        assert_eq!(
            payload,
            DragDropPayload::Path("/Users/me/diagram.png".to_string())
        );
    }

    #[test]
    fn kitty_file_uri() {
        let drop = "file:///tmp/screenshot%20x.png\n";
        let payload = parse_drag_drop_escape(drop).expect("payload");
        assert_eq!(
            payload,
            DragDropPayload::Path("/tmp/screenshot x.png".to_string())
        );
    }

    #[test]
    fn iterm2_inline_file_parses() {
        // name=base64("hi.png") = "aGkucG5n"; body base64("PNG-bytes")
        let body = BASE64.encode(b"PNG-bytes");
        let drop = format!("\x1b]1337;File=name=aGkucG5n;inline=1;size=9:{body}\x07");
        let payload = parse_drag_drop_escape(&drop).expect("payload");
        match payload {
            DragDropPayload::InlineFile { name, bytes } => {
                assert_eq!(name.as_deref(), Some("hi.png"));
                assert_eq!(bytes, b"PNG-bytes");
            }
            DragDropPayload::Path(p) => panic!("expected InlineFile, got Path({p})"),
        }
    }

    #[test]
    fn unrelated_input_returns_none() {
        assert!(parse_drag_drop_escape("just typing some text").is_none());
        assert!(parse_drag_drop_escape("").is_none());
    }
}
