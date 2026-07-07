//! Per-format output sinks for the headless driver (#165).
//!
//! The [`HeadlessDriver`](super::HeadlessDriver) used to scatter
//! `match self.config.output_format { Text | Json | StreamJson }` blocks
//! across every emission method. This module collapses that dispatch into a
//! single per-format [`FrameEncoder`] selected once in
//! `HeadlessDriver::new`, with one impl per format. The driver keeps ownership
//! of the writer and passes `&mut dyn Write` into each method; the encoder
//! relocates the per-format emission logic verbatim.
//!
//! The trait takes `&mut dyn Write` (rather than being generic over the
//! writer) so the driver can hold a plain `Box<dyn FrameEncoder>` — a writer
//! type parameter on the boxed encoder would tie the writer's borrow to the
//! driver's lifetime and break callers that read the buffer while the driver
//! is alive.
//!
//! Behavior is byte-for-byte identical to the prior inline match arms — this
//! is a pure relocation (DRY/SOLID, ADR 0025 protocol unchanged).

use std::io::Write;

use caliban_provider::ContentBlock;
use serde::Serialize;

use super::{
    HeadlessError, HeadlessRunConfig, HeadlessRunSummary, OutputFormat, content_blocks_to_json,
    events, parse_tool_input, render_verbose_tool_io,
};
use crate::stream_decode::ToolInput;

/// Shared NDJSON writer used by the stream-json encoder and the driver's
/// pre-result hook drain. Serializes `value`, writes it followed by a newline,
/// and flushes — the same contract the driver's old `write_ndjson` method had.
///
/// # Errors
/// Returns [`HeadlessError::Io`] on serialization or writer failure.
pub(crate) fn write_ndjson<T: Serialize>(
    w: &mut dyn Write,
    value: &T,
) -> Result<(), HeadlessError> {
    let json = serde_json::to_string(value).map_err(|e| HeadlessError::Io(e.to_string()))?;
    w.write_all(json.as_bytes())
        .map_err(|e| HeadlessError::Io(e.to_string()))?;
    w.write_all(b"\n")
        .map_err(|e| HeadlessError::Io(e.to_string()))?;
    w.flush().map_err(|e| HeadlessError::Io(e.to_string()))?;
    Ok(())
}

/// A per-format output sink. Each method relocates one of the driver's former
/// `match self.config.output_format` arms. The driver holds a
/// `Box<dyn FrameEncoder>` and passes `&mut self.writer` plus the arm's data
/// into each call.
pub(crate) trait FrameEncoder {
    /// `system/init` frame (stream-json only; no-op otherwise).
    fn system_init(
        &mut self,
        w: &mut dyn Write,
        cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError>;

    /// `user` echo frame (stream-json only; gating on `replay_user_messages`
    /// is done by the driver, the `StreamJson` check stays here for the no-op
    /// formats).
    fn user_echo(
        &mut self,
        w: &mut dyn Write,
        prompt: &str,
        cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError>;

    /// Per-event `hook_event` frame (stream-json only; no-op otherwise).
    fn hook_event(
        &mut self,
        w: &mut dyn Write,
        frame: &events::HookEvent,
    ) -> Result<(), HeadlessError>;

    /// Assistant text delta. Text writes the bytes (and tracks cursor column);
    /// stream-json with partial messages writes a `text` frame; json no-op.
    fn text_delta(
        &mut self,
        w: &mut dyn Write,
        text: &str,
        cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError>;

    /// Assistant thinking delta (stream-json + partial messages only).
    fn thinking_delta(
        &mut self,
        w: &mut dyn Write,
        text: &str,
        cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError>;

    /// A completed tool call. The driver has already taken the buffered tool
    /// input (when buffering applies) and passes it in; the encoder relocates
    /// the per-format emission.
    #[allow(clippy::too_many_arguments)]
    fn tool_call(
        &mut self,
        w: &mut dyn Write,
        tool_use_id: &str,
        buffered: Option<ToolInput>,
        is_error: bool,
        content: &[ContentBlock],
        cfg: &HeadlessRunConfig,
        dispatch_ms: Option<u64>,
    ) -> Result<(), HeadlessError>;

    /// The full assistant `message` frame (stream-json without partial
    /// messages only).
    fn assistant_message(
        &mut self,
        w: &mut dyn Write,
        content: &[ContentBlock],
        cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError>;

    /// A model-mismatch warning. Stream-json emits a `warning` frame; text/json
    /// print to stderr.
    fn model_mismatch(
        &mut self,
        w: &mut dyn Write,
        requested: &str,
        actual: &str,
        cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError>;

    /// Per-event trailing flush (text mode only) so deltas reach the terminal.
    fn flush_text(&mut self, w: &mut dyn Write) -> Result<(), HeadlessError>;

    /// The terminal `result` frame. The driver builds the frame (and runs the
    /// pre-result hook drain) and hands it here for the per-format tail.
    fn result(
        &mut self,
        w: &mut dyn Write,
        frame: &events::ResultFrame,
        s: &HeadlessRunSummary,
    ) -> Result<(), HeadlessError>;
}

/// `--output-format text` sink. Plain assistant text to the writer; verbose
/// tool I/O and stop notes to stderr. Carries the cursor-column flag that used
/// to be threaded through `handle_event`.
pub(crate) struct TextEncoder {
    /// Whether the last byte written to `w` was a newline. Drives the
    /// per-event flush and the trailing newline in `result`. Starts `true`
    /// (column zero) to match the driver's old `at_column_zero = true` init.
    at_column_zero: bool,
}

impl TextEncoder {
    pub(crate) fn new() -> Self {
        Self {
            at_column_zero: true,
        }
    }
}

impl FrameEncoder for TextEncoder {
    fn system_init(
        &mut self,
        _w: &mut dyn Write,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn user_echo(
        &mut self,
        _w: &mut dyn Write,
        _prompt: &str,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn hook_event(
        &mut self,
        _w: &mut dyn Write,
        _frame: &events::HookEvent,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn text_delta(
        &mut self,
        w: &mut dyn Write,
        text: &str,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        w.write_all(text.as_bytes())
            .map_err(|e| HeadlessError::Io(e.to_string()))?;
        self.at_column_zero = text.ends_with('\n');
        Ok(())
    }

    fn thinking_delta(
        &mut self,
        _w: &mut dyn Write,
        _text: &str,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn tool_call(
        &mut self,
        _w: &mut dyn Write,
        _tool_use_id: &str,
        buffered: Option<ToolInput>,
        is_error: bool,
        content: &[ContentBlock],
        cfg: &HeadlessRunConfig,
        _dispatch_ms: Option<u64>,
    ) -> Result<(), HeadlessError> {
        // `--verbose` text mode: dump the full, untruncated tool
        // call to stderr (stdout stays the assistant answer, so
        // pipes/`$(...)` capture is unaffected). Mirrors the
        // driver's existing stderr convention for warnings.
        if cfg.verbose
            && let Some(buf) = buffered
        {
            let input = parse_tool_input(&buf.json);
            let block = render_verbose_tool_io(&buf.name, &input, is_error, content);
            eprintln!("{block}");
        }
        Ok(())
    }

    fn assistant_message(
        &mut self,
        _w: &mut dyn Write,
        _content: &[ContentBlock],
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn model_mismatch(
        &mut self,
        _w: &mut dyn Write,
        requested: &str,
        actual: &str,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        eprintln!(
            "{}",
            crate::stream_decode::model_mismatch_text(requested, actual)
        );
        Ok(())
    }

    fn flush_text(&mut self, w: &mut dyn Write) -> Result<(), HeadlessError> {
        if !self.at_column_zero {
            // Ensure deltas are flushed; final newline is added at run end.
            w.flush().map_err(|e| HeadlessError::Io(e.to_string()))?;
        }
        Ok(())
    }

    fn result(
        &mut self,
        w: &mut dyn Write,
        _frame: &events::ResultFrame,
        s: &HeadlessRunSummary,
    ) -> Result<(), HeadlessError> {
        // Ensure trailing newline after streamed assistant text.
        if !s.final_text.is_empty() && !s.final_text.ends_with('\n') {
            w.write_all(b"\n")
                .map_err(|e| HeadlessError::Io(e.to_string()))?;
        }
        w.flush().map_err(|e| HeadlessError::Io(e.to_string()))?;
        // Text mode prints no result frame, so a non-success terminal
        // stop (max-turns, cancelled, budget) is otherwise completely
        // silent. Surface a one-line diagnostic on stderr (#175).
        if let Some(note) = super::text_mode_stop_note(s.subtype, s.turns) {
            eprintln!("{note}");
        }
        Ok(())
    }
}

/// `--output-format json` sink. Emits nothing until the single terminal
/// `result` object; every streaming method is a no-op.
pub(crate) struct JsonEncoder;

impl FrameEncoder for JsonEncoder {
    fn system_init(
        &mut self,
        _w: &mut dyn Write,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn user_echo(
        &mut self,
        _w: &mut dyn Write,
        _prompt: &str,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn hook_event(
        &mut self,
        _w: &mut dyn Write,
        _frame: &events::HookEvent,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn text_delta(
        &mut self,
        _w: &mut dyn Write,
        _text: &str,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn thinking_delta(
        &mut self,
        _w: &mut dyn Write,
        _text: &str,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn tool_call(
        &mut self,
        _w: &mut dyn Write,
        _tool_use_id: &str,
        _buffered: Option<ToolInput>,
        _is_error: bool,
        _content: &[ContentBlock],
        _cfg: &HeadlessRunConfig,
        _dispatch_ms: Option<u64>,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn assistant_message(
        &mut self,
        _w: &mut dyn Write,
        _content: &[ContentBlock],
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn model_mismatch(
        &mut self,
        _w: &mut dyn Write,
        requested: &str,
        actual: &str,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        eprintln!(
            "{}",
            crate::stream_decode::model_mismatch_text(requested, actual)
        );
        Ok(())
    }

    fn flush_text(&mut self, _w: &mut dyn Write) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn result(
        &mut self,
        w: &mut dyn Write,
        frame: &events::ResultFrame,
        _s: &HeadlessRunSummary,
    ) -> Result<(), HeadlessError> {
        let json = serde_json::to_string(frame).map_err(|e| HeadlessError::Io(e.to_string()))?;
        w.write_all(json.as_bytes())
            .map_err(|e| HeadlessError::Io(e.to_string()))?;
        w.write_all(b"\n")
            .map_err(|e| HeadlessError::Io(e.to_string()))?;
        Ok(())
    }
}

/// `--output-format stream-json` sink. Emits NDJSON frames for every event.
pub(crate) struct StreamJsonEncoder;

impl FrameEncoder for StreamJsonEncoder {
    fn system_init(
        &mut self,
        w: &mut dyn Write,
        cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        let frame = events::system_init(
            &cfg.session_id,
            &cfg.model_summary,
            cfg.tools.clone(),
            cfg.plugins.clone(),
            cfg.setting_sources.clone(),
            cfg.bare_mode,
            &cfg.cwd,
            &cfg.permission_mode,
        );
        write_ndjson(w, &frame)
    }

    fn user_echo(
        &mut self,
        w: &mut dyn Write,
        prompt: &str,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        let content = serde_json::json!([{ "type": "text", "text": prompt }]);
        let frame = events::user_echo(content);
        write_ndjson(w, &frame)
    }

    fn hook_event(
        &mut self,
        w: &mut dyn Write,
        frame: &events::HookEvent,
    ) -> Result<(), HeadlessError> {
        write_ndjson(w, frame)
    }

    fn text_delta(
        &mut self,
        w: &mut dyn Write,
        text: &str,
        cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        if cfg.include_partial_messages {
            write_ndjson(w, &events::text_delta(text))?;
        }
        Ok(())
    }

    fn thinking_delta(
        &mut self,
        w: &mut dyn Write,
        text: &str,
        cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        if cfg.include_partial_messages {
            write_ndjson(w, &events::thinking_delta(text))?;
        }
        Ok(())
    }

    fn tool_call(
        &mut self,
        w: &mut dyn Write,
        tool_use_id: &str,
        buffered: Option<ToolInput>,
        is_error: bool,
        content: &[ContentBlock],
        _cfg: &HeadlessRunConfig,
        dispatch_ms: Option<u64>,
    ) -> Result<(), HeadlessError> {
        // Pair the `tool_use` frame with the matching
        // `tool_result`: emit the deferred tool_use now that
        // the input JSON has finished streaming. Parse the
        // accumulated JSON; on parse failure fall back to a
        // string so the frame is never silently dropped.
        if let Some(buf) = buffered {
            let input = parse_tool_input(&buf.json);
            write_ndjson(w, &events::tool_use(tool_use_id, &buf.name, input))?;
        }
        let content_value = content_blocks_to_json(content);
        write_ndjson(
            w,
            &events::tool_result(tool_use_id, is_error, content_value, dispatch_ms),
        )?;
        Ok(())
    }

    fn assistant_message(
        &mut self,
        w: &mut dyn Write,
        content: &[ContentBlock],
        cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        if !cfg.include_partial_messages {
            let content_value = content_blocks_to_json(content);
            write_ndjson(w, &events::assistant_message(content_value))?;
        }
        Ok(())
    }

    fn model_mismatch(
        &mut self,
        w: &mut dyn Write,
        requested: &str,
        actual: &str,
        _cfg: &HeadlessRunConfig,
    ) -> Result<(), HeadlessError> {
        let frame = events::warning_model_mismatch(requested, actual);
        write_ndjson(w, &frame)
    }

    fn flush_text(&mut self, _w: &mut dyn Write) -> Result<(), HeadlessError> {
        Ok(())
    }

    fn result(
        &mut self,
        w: &mut dyn Write,
        frame: &events::ResultFrame,
        _s: &HeadlessRunSummary,
    ) -> Result<(), HeadlessError> {
        write_ndjson(w, frame)
    }
}

/// Select the encoder matching `format`. Called once in `HeadlessDriver::new`.
pub(crate) fn for_format(format: OutputFormat) -> Box<dyn FrameEncoder> {
    match format {
        OutputFormat::Text => Box::new(TextEncoder::new()),
        OutputFormat::Json => Box::new(JsonEncoder),
        OutputFormat::StreamJson => Box::new(StreamJsonEncoder),
    }
}
