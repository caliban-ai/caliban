//! Clipboard write via OSC-52 (IE3 Task 14).
//!
//! Emits the standard OSC-52 escape sequence so the host terminal
//! writes `text` to the system clipboard. Supported by kitty, iTerm2,
//! `WezTerm`, Ghostty, Alacritty, foot, modern Konsole, and (recent)
//! macOS Terminal.app. For terminals where OSC-52 is not honoured —
//! historically including some macOS Terminal.app configurations — a
//! follow-up TODO captures the `arboard` fallback path; v1 ships OSC-52
//! only so the binary stays linkable on headless hosts that lack the
//! X11/Wayland clipboard libraries `arboard` needs.
//!
//! Format (xterm `OSC 52 ; Pc ; Pd ST`):
//!
//! ```text
//! ESC ] 5 2 ; c ; <base64-of-text> BEL
//! ```
//!
//! `Pc = c` selects the system clipboard. Terminator is `BEL` (`\x07`).
//!
//! See `docs/TODO.md` § TUI ergonomics § IE3.

use std::io::Write;

use anyhow::Result;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;

/// Maximum payload length per OSC-52 sequence. Most terminals enforce a
/// length limit on OSC strings; 8 KiB is a common documented cap (xterm
/// is 100 KiB but kitty / `WezTerm` / Ghostty round closer to 8). Selections
/// longer than this are truncated rather than rejected — partial copy is
/// strictly better than silent failure.
const MAX_OSC52_BYTES: usize = 8 * 1024;

/// Build the OSC-52 escape sequence for `text`. Pure function so the
/// format is unit-testable without touching stdout or the terminal.
///
/// Truncates `text` to [`MAX_OSC52_BYTES`] bytes before base64-encoding
/// to keep within typical terminal OSC string caps. Truncation is at
/// byte boundaries; on a char boundary issue the trailing bytes are
/// dropped (the base64 encoder is bytewise so the encoded output is
/// still valid even if the underlying UTF-8 is truncated mid-character).
#[must_use]
pub(crate) fn osc52_payload(text: &str) -> String {
    let bytes = text.as_bytes();
    let to_encode = if bytes.len() > MAX_OSC52_BYTES {
        // Step back to the previous UTF-8 char boundary to keep the
        // pasted text valid even if truncated.
        let mut cut = MAX_OSC52_BYTES;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        &bytes[..cut]
    } else {
        bytes
    };
    let b64 = BASE64_STANDARD.encode(to_encode);
    format!("\x1b]52;c;{b64}\x07")
}

/// Emit an OSC-52 clipboard-write sequence for `text` to stdout, then
/// flush. Returns the IO error if write or flush fails. Best-effort:
/// callers should not abort on failure (clipboard write is a UX nicety,
/// not a correctness requirement).
///
/// Empty input is a no-op (no escape sequence emitted).
pub(crate) fn copy_to_clipboard(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let payload = osc52_payload(text);
    let mut out = std::io::stdout().lock();
    out.write_all(payload.as_bytes())?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IE3 Task 14: OSC-52 payload starts with `ESC ] 52 ; c ;`, ends
    /// with BEL, and contains the base64 of the input between.
    #[test]
    fn osc52_payload_round_trips_empty() {
        // Empty input is a no-op for the public API; the pure builder
        // still produces a well-formed (empty-body) sequence.
        let p = osc52_payload("");
        assert!(p.starts_with("\x1b]52;c;"));
        assert!(p.ends_with('\x07'));
    }

    #[test]
    fn osc52_payload_encodes_ascii() {
        let p = osc52_payload("hello");
        // base64("hello") == "aGVsbG8="
        assert_eq!(p, "\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn osc52_payload_encodes_utf8() {
        // 'カ' is 3 bytes (e3 82 ab); base64 -> "44Kr"
        let p = osc52_payload("カ");
        assert_eq!(p, "\x1b]52;c;44Kr\x07");
    }

    #[test]
    fn osc52_payload_truncates_huge_input_at_char_boundary() {
        // Build a string larger than MAX_OSC52_BYTES with a multi-byte
        // char near the truncation point.
        let mut text = "a".repeat(MAX_OSC52_BYTES - 1);
        text.push('カ'); // 3 bytes — push past the limit on a non-boundary
        text.push('カ');
        let p = osc52_payload(&text);
        // Decode the base64 body and ensure the bytes form valid UTF-8.
        let body = p
            .strip_prefix("\x1b]52;c;")
            .and_then(|s| s.strip_suffix('\x07'))
            .expect("well-formed envelope");
        let decoded = BASE64_STANDARD
            .decode(body)
            .expect("valid base64 round-trip");
        // The truncation MUST land on a char boundary so UTF-8 stays valid.
        assert!(std::str::from_utf8(&decoded).is_ok());
        // And the decoded length should be <= the cap.
        assert!(decoded.len() <= MAX_OSC52_BYTES);
    }

    #[test]
    fn copy_to_clipboard_is_noop_on_empty() {
        // Doesn't actually touch stdout for empty input (returns Ok early).
        copy_to_clipboard("").expect("empty input is Ok");
    }
}
