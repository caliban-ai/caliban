//! Stdin reader for headless mode — text + stream-json with a hard cap.

use std::io::Read;

use crate::headless::HeadlessError;
use crate::headless::events::InputFrame;

/// Maximum stdin payload accepted by headless mode (10 MiB).
///
/// Matches the documented Claude Code limit. Larger payloads return
/// [`HeadlessError::StdinTooLarge`], which the binary maps to exit code 78
/// (`EX_CONFIGURATION_ERROR`).
pub(crate) const STDIN_CAP_BYTES: u64 = 10 * 1024 * 1024;

/// Read all of stdin, enforcing the 10 MiB cap.
///
/// # Errors
/// - [`HeadlessError::StdinTooLarge`] when more than [`STDIN_CAP_BYTES`]
///   bytes are available.
/// - [`HeadlessError::Io`] on read failure.
pub(crate) fn read_stdin_capped<R: Read>(reader: &mut R) -> Result<String, HeadlessError> {
    let mut buf = Vec::new();
    // `take` enforces the cap server-side; an additional byte tells us we
    // actually hit the limit (rather than legitimately ending on the boundary).
    let cap_plus_one = STDIN_CAP_BYTES + 1;
    reader
        .take(cap_plus_one)
        .read_to_end(&mut buf)
        .map_err(|e| HeadlessError::Io(e.to_string()))?;
    if u64::try_from(buf.len()).unwrap_or(u64::MAX) > STDIN_CAP_BYTES {
        return Err(HeadlessError::StdinTooLarge {
            limit_bytes: STDIN_CAP_BYTES,
        });
    }
    String::from_utf8(buf).map_err(|e| HeadlessError::Io(format!("stdin not utf-8: {e}")))
}

/// Parse a single NDJSON line as an [`InputFrame`].
///
/// Lines that are entirely whitespace return `Ok(None)` so callers can
/// treat blank-separated NDJSON as valid input.
///
/// # Errors
/// Returns [`HeadlessError::InputParse`] on a malformed line.
pub(crate) fn parse_input_line(line: &str) -> Result<Option<InputFrame>, HeadlessError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(trimmed)
        .map(Some)
        .map_err(|e| HeadlessError::InputParse(e.to_string()))
}

/// Parse a full NDJSON stdin payload into a vec of frames.
///
/// # Errors
/// Returns the first parse error encountered.
pub(crate) fn parse_stream_json_payload(input: &str) -> Result<Vec<InputFrame>, HeadlessError> {
    let mut frames = Vec::new();
    for line in input.lines() {
        if let Some(frame) = parse_input_line(line)? {
            frames.push(frame);
        }
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_stdin_under_cap_succeeds() {
        let payload = "hello world";
        let mut cur = std::io::Cursor::new(payload);
        let out = read_stdin_capped(&mut cur).unwrap();
        assert_eq!(out, payload);
    }

    #[test]
    fn read_stdin_at_boundary_succeeds() {
        let cap = usize::try_from(STDIN_CAP_BYTES).expect("cap fits in usize");
        let payload = vec![b'x'; cap];
        let mut cur = std::io::Cursor::new(payload);
        let out = read_stdin_capped(&mut cur).unwrap();
        assert_eq!(out.len(), cap);
    }

    #[test]
    fn read_stdin_over_cap_errors() {
        let cap_plus_one = usize::try_from(STDIN_CAP_BYTES + 1).expect("cap fits in usize");
        let payload = vec![b'x'; cap_plus_one];
        let mut cur = std::io::Cursor::new(payload);
        let err = read_stdin_capped(&mut cur).unwrap_err();
        assert!(matches!(err, HeadlessError::StdinTooLarge { .. }));
    }

    #[test]
    fn parse_input_line_skips_blank() {
        assert!(parse_input_line("").unwrap().is_none());
        assert!(parse_input_line("   ").unwrap().is_none());
    }

    #[test]
    fn parse_input_line_user_frame() {
        let f = parse_input_line(r#"{"type":"user","content":"hi"}"#)
            .unwrap()
            .unwrap();
        assert!(matches!(f, InputFrame::User { .. }));
    }

    #[test]
    fn parse_input_line_malformed_errors() {
        let err = parse_input_line(r#"{"type":"user","content":"#).unwrap_err();
        assert!(matches!(err, HeadlessError::InputParse(_)));
    }

    #[test]
    fn parse_stream_json_payload_multiple_frames() {
        let payload = r#"{"type":"user","content":"a"}
{"type":"user","content":"b"}
{"type":"control","subtype":"interrupt"}
"#;
        let frames = parse_stream_json_payload(payload).unwrap();
        assert_eq!(frames.len(), 3);
        assert!(matches!(frames[0], InputFrame::User { .. }));
        assert!(matches!(frames[1], InputFrame::User { .. }));
        assert!(matches!(frames[2], InputFrame::Control { .. }));
    }
}
