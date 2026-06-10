//! Client-side rendering for `caliban agents attach <id>` (#79).
//!
//! Reads the worker's per-agent socket — newline-delimited `TurnEvent`
//! JSON (see #78) — and renders a readable transcript. Read-only:
//! inbound user messages are out of scope (#81).

use std::io::Write;

use caliban_agent_core::TurnEvent;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};

/// Render a single `TurnEvent` to `out` as a readable transcript fragment.
/// Returns the bytes to write. Kept pure (no I/O) so it is unit-testable.
pub(crate) fn render_event(ev: &TurnEvent) -> String {
    match ev {
        TurnEvent::AssistantTextDelta { text, .. } => text.clone(),
        TurnEvent::ToolCallStart { name, .. } => format!("\n\u{1f527} {name}\n"),
        TurnEvent::ToolCallEnd { is_error, .. } => {
            if *is_error {
                "   \u{2192} (error)\n".to_string()
            } else {
                "   \u{2192} ok\n".to_string()
            }
        }
        TurnEvent::RunEnd {
            stopped_for,
            turn_count,
            ..
        } => {
            format!("\n[done: {stopped_for:?} after {turn_count} turns]\n")
        }
        // Thinking deltas, turn boundaries, and tool-input deltas are not
        // rendered in the attach transcript (kept concise).
        _ => String::new(),
    }
}

/// Drive an attach stream: read NDJSON `TurnEvent` lines from `reader`,
/// render each to `out`. Malformed lines are skipped (a best-effort note is
/// written). Returns when the stream reaches EOF (agent finished / detached).
pub(crate) async fn stream_attach<R, W>(reader: R, out: &mut W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: Write,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<TurnEvent>(&line) {
            Ok(ev) => {
                let s = render_event(&ev);
                if !s.is_empty() {
                    out.write_all(s.as_bytes())?;
                    out.flush()?;
                }
            }
            Err(e) => {
                // Don't abort the whole attach on one bad line.
                let _ = writeln!(out, "[caliban attach: unparsable event: {e}]");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn renders_text_and_runend_from_ndjson() {
        // Two events as the worker would emit them (internally tagged).
        let ndjson = concat!(
            r#"{"type":"AssistantTextDelta","turn_index":0,"content_block_index":0,"text":"hello "}"#,
            "\n",
            r#"{"type":"AssistantTextDelta","turn_index":0,"content_block_index":0,"text":"world"}"#,
            "\n",
            r#"{"type":"RunEnd","final_messages":[],"total_usage":{"input_tokens":0,"output_tokens":0},"turn_count":1,"stopped_for":"EndOfTurn"}"#,
            "\n",
        );
        let mut out: Vec<u8> = Vec::new();
        stream_attach(Cursor::new(ndjson), &mut out).await.unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("hello world"), "got: {s:?}");
        assert!(s.contains("done"), "got: {s:?}");
    }

    #[tokio::test]
    async fn skips_malformed_lines_without_aborting() {
        let ndjson = "not json\n{\"type\":\"AssistantTextDelta\",\"turn_index\":0,\"content_block_index\":0,\"text\":\"ok\"}\n";
        let mut out: Vec<u8> = Vec::new();
        stream_attach(Cursor::new(ndjson), &mut out).await.unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("ok"), "got: {s:?}");
        assert!(s.contains("unparsable"), "got: {s:?}");
    }
}
