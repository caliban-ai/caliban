//! Client-side rendering for `caliban agents attach <id>` (#79).
//!
//! Reads the worker's per-agent socket — newline-delimited `TurnEvent`
//! JSON (see #78) — and renders a readable transcript.
//!
//! Also defines [`AttachInbound`], the inbound frame type sent by an attached
//! operator *to* the worker over the same per-agent socket (ADR 0047 / #81).
//! Outbound is `TurnEvent` NDJSON; inbound is `AttachInbound` NDJSON.
//! The two never share a direction.

use std::io::Write;

use caliban_agent_core::TurnEvent;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};

/// A frame an attached operator sends INBOUND to a running worker over the
/// per-agent socket (ADR 0047 / #81). Outbound is `TurnEvent` NDJSON (#79);
/// inbound is `AttachInbound` NDJSON. The two never share a direction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub(crate) enum AttachInbound {
    /// Operator sends a user message to inject into the run.
    UserMessage { text: String },
    /// Operator signals end-of-input: the run should finish after this.
    EndInput,
}

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

/// Read operator input lines from `reader` and write them to `writer` as
/// `AttachInbound::UserMessage` NDJSON frames (one per non-empty line). On
/// EOF (operator Ctrl+D), write a final `AttachInbound::EndInput` frame so
/// the agent finishes. Used by `agents attach` to drive an interactive
/// sub-agent (ADR 0047 / #81). Returns on EOF or write error.
pub(crate) async fn stdin_to_frames<R, W>(reader: R, mut writer: W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue; // skip blank lines — don't send empty user messages
        }
        write_frame(&mut writer, &AttachInbound::UserMessage { text: line }).await?;
    }
    // EOF → tell the agent to finish.
    write_frame(&mut writer, &AttachInbound::EndInput).await
}

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &AttachInbound,
) -> std::io::Result<()> {
    let mut buf = serde_json::to_vec(frame).map_err(std::io::Error::other)?;
    buf.push(b'\n');
    writer.write_all(&buf).await?;
    writer.flush().await
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

    // --- stdin_to_frames ---

    #[tokio::test]
    async fn stdin_to_frames_sends_usermessages_then_endinput() {
        let input = b"hello\nworld\n";
        let mut out: Vec<u8> = Vec::new();
        stdin_to_frames(std::io::Cursor::new(input), &mut out)
            .await
            .unwrap();
        let frames: Vec<AttachInbound> = out
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).expect("valid NDJSON"))
            .collect();
        assert_eq!(
            frames,
            vec![
                AttachInbound::UserMessage {
                    text: "hello".into()
                },
                AttachInbound::UserMessage {
                    text: "world".into()
                },
                AttachInbound::EndInput,
            ]
        );
    }

    #[tokio::test]
    async fn stdin_to_frames_skips_blank_lines() {
        let input = b"a\n\n  \nb\n";
        let mut out: Vec<u8> = Vec::new();
        stdin_to_frames(std::io::Cursor::new(input), &mut out)
            .await
            .unwrap();
        let frames: Vec<AttachInbound> = out
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).expect("valid NDJSON"))
            .collect();
        assert_eq!(
            frames,
            vec![
                AttachInbound::UserMessage { text: "a".into() },
                AttachInbound::UserMessage { text: "b".into() },
                AttachInbound::EndInput,
            ]
        );
    }

    #[tokio::test]
    async fn stdin_to_frames_empty_input_sends_only_endinput() {
        // Immediate EOF (operator attaches and Ctrl+D's without typing):
        // exactly one EndInput, no UserMessage. Pins the EndInput write
        // OUTSIDE the read loop.
        let mut out: Vec<u8> = Vec::new();
        stdin_to_frames(std::io::Cursor::new(b"" as &[u8]), &mut out)
            .await
            .unwrap();
        let frames: Vec<AttachInbound> = out
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).expect("valid NDJSON"))
            .collect();
        assert_eq!(frames, vec![AttachInbound::EndInput]);
    }

    // --- AttachInbound serde ---

    #[test]
    fn attach_inbound_round_trips() {
        // UserMessage variant
        let msg = AttachInbound::UserMessage {
            text: "hello agent".into(),
        };
        let json = serde_json::to_string(&msg).expect("serialize UserMessage");
        let back: AttachInbound = serde_json::from_str(&json).expect("deserialize UserMessage");
        assert_eq!(msg, back);
        // Internal "type" tag must be present.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "UserMessage");
        assert_eq!(v["text"], "hello agent");

        // EndInput variant
        let end = AttachInbound::EndInput;
        let json2 = serde_json::to_string(&end).expect("serialize EndInput");
        let back2: AttachInbound = serde_json::from_str(&json2).expect("deserialize EndInput");
        assert_eq!(end, back2);
        let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
        assert_eq!(v2["type"], "EndInput");
    }
}
