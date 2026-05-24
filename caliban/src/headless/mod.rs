//! Headless / print-mode driver for caliban (ADR 0025).
//!
//! Sibling to the TUI driver; consumes the same `AgentBuilder` +
//! `TurnEventStream` from `caliban-agent-core` but renders events as
//! plain text, a single JSON `result` object, or NDJSON `stream-json`
//! frames.
//!
//! The driver is intentionally pure with respect to I/O above the
//! "write to a `Writer`" boundary so it can be unit-tested without a
//! pseudo-TTY. The binary wires it to `stdout` via `BufWriter`.

// Forward-facing scaffolding: several types and methods are surfaced now
// so the public protocol stays stable, but a few are only exercised by
// the test suite (or by features that land in follow-up ADRs — `api_retry`
// is wired by ADR 0033, `EventKind`/`parse_str` by the upcoming router
// integration). Allow dead-code in this module so we don't have to gate
// each one with `#[cfg(test)]`.
#![allow(dead_code)]

pub(crate) mod budget;
pub(crate) mod cli;
pub(crate) mod events;
pub(crate) mod hooks_sink;
pub(crate) mod input;
pub(crate) mod schema;
pub(crate) mod session_loader;

use std::io::Write;
use std::sync::Arc;

use caliban_agent_core::{Agent, StopCondition, TurnEvent};
use caliban_provider::{ContentBlock, Message};
use futures::StreamExt as _;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub(crate) use budget::BudgetTracker;
pub(crate) use cli::{InputFormat, OutputFormat};
pub(crate) use events::ResultSubtype;
pub(crate) use hooks_sink::{HeadlessHookSink, HookEventBuffer, new_event_buffer};
pub(crate) use schema::JsonSchema;

/// Errors specific to the headless driver. The binary maps these to
/// `sysexits.h`-style process exit codes (see [`exit_code_for`]).
#[derive(Debug, Error)]
pub(crate) enum HeadlessError {
    /// `--max-turns` exceeded.
    #[error("max turns ({0}) exceeded")]
    MaxTurnsExceeded(u32),
    /// `--max-budget-usd` exceeded.
    #[error("max budget exceeded (configured: {limit:?} USD)")]
    BudgetExceeded {
        /// Configured limit (always `Some` when we surface this).
        limit: Option<f64>,
    },
    /// Stdin payload exceeded the 10 MiB cap.
    #[error("stdin payload exceeded {limit_bytes} bytes")]
    StdinTooLarge {
        /// Cap in bytes.
        limit_bytes: u64,
    },
    /// I/O error.
    #[error("io error: {0}")]
    Io(String),
    /// Failed to parse a stream-json input frame.
    #[error("input parse error: {0}")]
    InputParse(String),
    /// Failed to parse `--json-schema`.
    #[error("schema parse error: {0}")]
    SchemaParse(String),
    /// Failed to validate the assistant's final reply against the schema.
    #[error("schema validation failed: {0}")]
    SchemaValidation(String),
    /// `--resume <id>` named a session that does not exist.
    #[error("no session named `{0}` to resume")]
    ResumeNotFound(String),
    /// `--continue` requested but no sessions are present.
    #[error("no sessions to continue")]
    NoSessionsToContinue,
    /// Session-store I/O / parse failure.
    #[error("session load error: {0}")]
    SessionLoad(String),
    /// Provider / agent-core error surfaced by the run.
    #[error("run error: {0}")]
    Run(String),
    /// The run was cancelled (Ctrl-C / SIGTERM).
    #[error("cancelled")]
    Cancelled,
    /// Generic configuration error (bad combination of flags).
    #[error("configuration error: {0}")]
    Configuration(String),
}

/// Map a [`HeadlessError`] to a process exit code per ADR 0025.
#[must_use]
pub(crate) fn exit_code_for(err: &HeadlessError) -> i32 {
    match err {
        HeadlessError::MaxTurnsExceeded(_) => 130,
        HeadlessError::BudgetExceeded { .. } => 137,
        HeadlessError::StdinTooLarge { .. } | HeadlessError::Configuration(_) => 78,
        HeadlessError::ResumeNotFound(_) | HeadlessError::NoSessionsToContinue => 66,
        HeadlessError::InputParse(_) | HeadlessError::SchemaParse(_) => 64,
        HeadlessError::SchemaValidation(_) => 2,
        HeadlessError::Cancelled => 124,
        HeadlessError::Run(_) | HeadlessError::SessionLoad(_) | HeadlessError::Io(_) => 1,
    }
}

/// All headless-specific knobs distilled into a single struct so the
/// driver is independent of `clap` parsing.
#[derive(Debug, Clone)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent CLI flag from ADR 0025"
)]
pub(crate) struct HeadlessRunConfig {
    /// Selected stdout format.
    pub(crate) output_format: OutputFormat,
    /// Selected stdin format.
    pub(crate) input_format: InputFormat,
    /// Hard cap on agent turns. `None` defers to `AgentConfig::max_turns`.
    pub(crate) max_turns: Option<u32>,
    /// Budget tracker — Arc'd because callers may want to inspect it after
    /// the run.
    pub(crate) budget: Arc<BudgetTracker>,
    /// Optional pre-loaded JSON schema for structured output.
    pub(crate) json_schema: Option<JsonSchema>,
    /// Emit assistant `text_delta` frames in stream-json mode.
    pub(crate) include_partial_messages: bool,
    /// Emit `hook_event` frames in stream-json mode.
    pub(crate) include_hook_events: bool,
    /// Echo user prompts back as `user` frames.
    pub(crate) replay_user_messages: bool,
    /// `--bare` flag in effect.
    pub(crate) bare_mode: bool,
    /// `--fallback-model` (passes through for router v2; ignored otherwise).
    pub(crate) fallback_model: Option<String>,
    /// Session identifier surfaced in frames.
    pub(crate) session_id: String,
    /// Settings source-chain ("managed", "user", "project", …).
    pub(crate) setting_sources: Vec<String>,
    /// Tool names (alphabetical) for the `system/init` frame.
    pub(crate) tools: Vec<String>,
    /// "<provider>/<model>" summary.
    pub(crate) model_summary: String,
    /// Current working directory at run start.
    pub(crate) cwd: String,
    /// Optional buffer of hook events accumulated by an outer
    /// [`HeadlessHookSink`]. The driver drains this after each turn.
    pub(crate) hook_buffer: Option<HookEventBuffer>,
}

impl HeadlessRunConfig {
    /// Convenience: a minimal config suitable for unit tests.
    #[must_use]
    pub(crate) fn minimal(output_format: OutputFormat) -> Self {
        Self {
            output_format,
            input_format: InputFormat::Text,
            max_turns: None,
            budget: BudgetTracker::new(None),
            json_schema: None,
            include_partial_messages: false,
            include_hook_events: false,
            replay_user_messages: false,
            bare_mode: false,
            fallback_model: None,
            session_id: "test-session".into(),
            setting_sources: Vec::new(),
            tools: Vec::new(),
            model_summary: "mock/mock".into(),
            cwd: ".".into(),
            hook_buffer: None,
        }
    }
}

/// The output produced by running [`HeadlessDriver::run`]. Returned by
/// value so the binary can decide whether to write to stdout/stderr and
/// pick the appropriate exit code.
#[derive(Debug, Clone)]
pub(crate) struct HeadlessRunSummary {
    /// Result subtype to surface in the final frame.
    pub(crate) subtype: ResultSubtype,
    /// Final assistant text (best-effort).
    pub(crate) final_text: String,
    /// Number of turns completed.
    pub(crate) turns: u32,
    /// Cumulative input tokens.
    pub(crate) total_input_tokens: u32,
    /// Cumulative output tokens.
    pub(crate) total_output_tokens: u32,
    /// Cumulative cost USD (placeholder until ADR 0033).
    pub(crate) total_cost_usd: f64,
    /// Structured output payload (when `--json-schema` succeeded).
    pub(crate) structured_output: Option<serde_json::Value>,
    /// Error message (when `subtype == Error`).
    pub(crate) error: Option<String>,
}

/// Stateful headless driver. Owns the writer and the run config; takes
/// ownership of the message vector and the agent.
pub(crate) struct HeadlessDriver<W: Write> {
    writer: W,
    config: HeadlessRunConfig,
}

impl<W: Write> HeadlessDriver<W> {
    /// Construct a new driver writing to `writer`.
    pub(crate) fn new(writer: W, config: HeadlessRunConfig) -> Self {
        Self { writer, config }
    }

    /// Emit the `system/init` frame (stream-json mode only). No-op for
    /// other formats. Always safe to call.
    ///
    /// # Errors
    /// Returns [`HeadlessError::Io`] on writer failure.
    pub(crate) fn emit_init(&mut self) -> Result<(), HeadlessError> {
        if !matches!(self.config.output_format, OutputFormat::StreamJson) {
            return Ok(());
        }
        let frame = events::system_init(
            &self.config.session_id,
            &self.config.model_summary,
            self.config.tools.clone(),
            self.config.setting_sources.clone(),
            self.config.bare_mode,
            &self.config.cwd,
        );
        self.write_ndjson(&frame)
    }

    /// Echo a user prompt back as a `user` frame, when both
    /// `--replay-user-messages` and stream-json output are in effect.
    ///
    /// # Errors
    /// Returns [`HeadlessError::Io`] on writer failure.
    pub(crate) fn emit_user_echo(&mut self, prompt: &str) -> Result<(), HeadlessError> {
        if !self.config.replay_user_messages
            || !matches!(self.config.output_format, OutputFormat::StreamJson)
        {
            return Ok(());
        }
        let content = serde_json::json!([{ "type": "text", "text": prompt }]);
        let frame = events::user_echo(content);
        self.write_ndjson(&frame)
    }

    /// Drain the hook-event buffer (if any) and emit each event as a
    /// `hook_event` frame. No-op for non-stream-json output or when
    /// hook events are disabled.
    ///
    /// # Errors
    /// Returns [`HeadlessError::Io`] on writer failure.
    pub(crate) fn flush_hook_events(&mut self) -> Result<(), HeadlessError> {
        if !self.config.include_hook_events
            || !matches!(self.config.output_format, OutputFormat::StreamJson)
        {
            return Ok(());
        }
        let Some(buf) = &self.config.hook_buffer else {
            return Ok(());
        };
        let drained: Vec<events::HookEvent> = {
            let mut guard = buf.lock().expect("hook buffer lock poisoned");
            std::mem::take(&mut *guard)
        };
        for frame in &drained {
            self.write_ndjson(frame)?;
        }
        Ok(())
    }

    /// Run the agent loop, encoding events as configured.
    ///
    /// The driver:
    ///   1. Emits `system/init` (stream-json only).
    ///   2. Optionally echoes the most recent user message.
    ///   3. Pulls events off the stream until `RunEnd` or cancellation.
    ///   4. Emits `text` / `text_delta` / `tool_use` / `tool_result` /
    ///      `message` frames per format.
    ///   5. Validates structured output (if `--json-schema`).
    ///   6. Returns a [`HeadlessRunSummary`].
    ///
    /// # Errors
    /// Returns the first fatal error encountered. Successful runs return
    /// a summary whose `subtype` indicates the terminal condition.
    #[allow(
        clippy::too_many_lines,
        reason = "linear event-loop is clearer in one body"
    )]
    pub(crate) async fn run(
        &mut self,
        agent: Arc<Agent>,
        messages: Vec<Message>,
        cancel: CancellationToken,
    ) -> Result<HeadlessRunSummary, HeadlessError> {
        self.emit_init()?;
        // Echo the trailing user message, if any.
        if let Some(last_user) = messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, caliban_provider::Role::User))
        {
            let text = extract_user_text(last_user);
            self.emit_user_echo(&text)?;
        }

        let max_turns_was_zero = self.config.max_turns == Some(0);
        if max_turns_was_zero {
            // Short-circuit: a 0 turn limit is a deterministic max-turns event.
            let summary = HeadlessRunSummary {
                subtype: ResultSubtype::MaxTurns,
                final_text: String::new(),
                turns: 0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cost_usd: 0.0,
                structured_output: None,
                error: None,
            };
            self.emit_result(&summary)?;
            return Err(HeadlessError::MaxTurnsExceeded(0));
        }

        let mut stream = agent.stream_until_done(messages, cancel);
        let mut final_text = String::new();
        let mut turns: u32 = 0;
        let mut at_column_zero = true;

        while let Some(event_result) = stream.next().await {
            let event = event_result.map_err(|e| HeadlessError::Run(e.to_string()))?;
            self.handle_event(event, &mut final_text, &mut turns, &mut at_column_zero)?;
            self.flush_hook_events()?;
            // Budget check (the hook sink may push frames first).
            if self.config.budget.is_exceeded() {
                let (i_tok, o_tok) = self.config.budget.total_tokens();
                let summary = HeadlessRunSummary {
                    subtype: ResultSubtype::BudgetExceeded,
                    final_text: final_text.clone(),
                    turns,
                    total_input_tokens: u32::try_from(i_tok).unwrap_or(u32::MAX),
                    total_output_tokens: u32::try_from(o_tok).unwrap_or(u32::MAX),
                    total_cost_usd: self.config.budget.total_cost_usd(),
                    structured_output: None,
                    error: None,
                };
                self.emit_result(&summary)?;
                return Err(HeadlessError::BudgetExceeded {
                    limit: self.config.budget.max_usd(),
                });
            }
        }

        // Structured-output validation.
        let (structured_output, schema_error) = match &self.config.json_schema {
            Some(schema) => match schema::extract_json_object(&final_text) {
                Some(candidate) => match schema.validate(&candidate) {
                    Ok(()) => (Some(candidate), None),
                    Err(e) => (None, Some(e)),
                },
                None => (
                    None,
                    Some("could not extract a JSON object from the assistant reply".to_string()),
                ),
            },
            None => (None, None),
        };

        let (i_tok, o_tok) = self.config.budget.total_tokens();
        let summary = HeadlessRunSummary {
            subtype: if schema_error.is_some() {
                ResultSubtype::Error
            } else {
                ResultSubtype::Success
            },
            final_text: final_text.clone(),
            turns,
            total_input_tokens: u32::try_from(i_tok).unwrap_or(u32::MAX),
            total_output_tokens: u32::try_from(o_tok).unwrap_or(u32::MAX),
            total_cost_usd: self.config.budget.total_cost_usd(),
            structured_output,
            error: schema_error.clone(),
        };
        self.emit_result(&summary)?;
        if let Some(e) = schema_error {
            return Err(HeadlessError::SchemaValidation(e));
        }
        Ok(summary)
    }

    fn handle_event(
        &mut self,
        event: TurnEvent,
        final_text: &mut String,
        turns: &mut u32,
        at_column_zero: &mut bool,
    ) -> Result<(), HeadlessError> {
        match event {
            TurnEvent::AssistantTextDelta { text, .. } => {
                final_text.push_str(&text);
                match self.config.output_format {
                    OutputFormat::Text => {
                        self.writer
                            .write_all(text.as_bytes())
                            .map_err(|e| HeadlessError::Io(e.to_string()))?;
                        *at_column_zero = text.ends_with('\n');
                    }
                    OutputFormat::StreamJson if self.config.include_partial_messages => {
                        self.write_ndjson(&events::text_delta(&text))?;
                    }
                    _ => {}
                }
            }
            TurnEvent::ToolCallStart {
                tool_use_id, name, ..
            } => {
                if matches!(self.config.output_format, OutputFormat::StreamJson) {
                    self.write_ndjson(&events::tool_use(
                        &tool_use_id,
                        &name,
                        serde_json::Value::Null,
                    ))?;
                }
            }
            TurnEvent::ToolCallEnd {
                tool_use_id,
                is_error,
                content,
                ..
            } => {
                if matches!(self.config.output_format, OutputFormat::StreamJson) {
                    let content_value = content_blocks_to_json(&content);
                    self.write_ndjson(&events::tool_result(&tool_use_id, is_error, content_value))?;
                }
            }
            TurnEvent::TurnEnd {
                assistant_message,
                usage,
                ..
            } => {
                *turns += 1;
                self.config.budget.record(&usage, 0.0);
                if matches!(self.config.output_format, OutputFormat::StreamJson)
                    && !self.config.include_partial_messages
                {
                    let content_value = content_blocks_to_json(&assistant_message.content);
                    self.write_ndjson(&events::assistant_message(content_value))?;
                }
            }
            TurnEvent::RunEnd { stopped_for, .. } => {
                if let StopCondition::MaxTurnsReached(n) = stopped_for {
                    // The run was bounded by max_turns; emit the result and signal.
                    let (i_tok, o_tok) = self.config.budget.total_tokens();
                    let summary = HeadlessRunSummary {
                        subtype: ResultSubtype::MaxTurns,
                        final_text: final_text.clone(),
                        turns: *turns,
                        total_input_tokens: u32::try_from(i_tok).unwrap_or(u32::MAX),
                        total_output_tokens: u32::try_from(o_tok).unwrap_or(u32::MAX),
                        total_cost_usd: self.config.budget.total_cost_usd(),
                        structured_output: None,
                        error: None,
                    };
                    self.emit_result(&summary)?;
                    return Err(HeadlessError::MaxTurnsExceeded(n));
                }
                if let StopCondition::Cancelled = stopped_for {
                    let (i_tok, o_tok) = self.config.budget.total_tokens();
                    let summary = HeadlessRunSummary {
                        subtype: ResultSubtype::Cancelled,
                        final_text: final_text.clone(),
                        turns: *turns,
                        total_input_tokens: u32::try_from(i_tok).unwrap_or(u32::MAX),
                        total_output_tokens: u32::try_from(o_tok).unwrap_or(u32::MAX),
                        total_cost_usd: self.config.budget.total_cost_usd(),
                        structured_output: None,
                        error: None,
                    };
                    self.emit_result(&summary)?;
                    return Err(HeadlessError::Cancelled);
                }
            }
            _ => {}
        }
        if matches!(self.config.output_format, OutputFormat::Text) && !*at_column_zero {
            // Ensure deltas are flushed; final newline is added at run end.
            self.writer
                .flush()
                .map_err(|e| HeadlessError::Io(e.to_string()))?;
        }
        Ok(())
    }

    fn emit_result(&mut self, s: &HeadlessRunSummary) -> Result<(), HeadlessError> {
        let frame = events::result_frame(
            s.subtype,
            &s.final_text,
            &self.config.session_id,
            s.total_cost_usd,
            s.turns,
            s.total_input_tokens,
            s.total_output_tokens,
            s.structured_output.clone(),
            s.error.clone(),
        );
        match self.config.output_format {
            OutputFormat::Text => {
                // Ensure trailing newline after streamed assistant text.
                if !s.final_text.is_empty() && !s.final_text.ends_with('\n') {
                    self.writer
                        .write_all(b"\n")
                        .map_err(|e| HeadlessError::Io(e.to_string()))?;
                }
                self.writer
                    .flush()
                    .map_err(|e| HeadlessError::Io(e.to_string()))?;
            }
            OutputFormat::Json => {
                let json =
                    serde_json::to_string(&frame).map_err(|e| HeadlessError::Io(e.to_string()))?;
                self.writer
                    .write_all(json.as_bytes())
                    .map_err(|e| HeadlessError::Io(e.to_string()))?;
                self.writer
                    .write_all(b"\n")
                    .map_err(|e| HeadlessError::Io(e.to_string()))?;
            }
            OutputFormat::StreamJson => {
                self.write_ndjson(&frame)?;
            }
        }
        Ok(())
    }

    fn write_ndjson<T: serde::Serialize>(&mut self, value: &T) -> Result<(), HeadlessError> {
        let json = serde_json::to_string(value).map_err(|e| HeadlessError::Io(e.to_string()))?;
        self.writer
            .write_all(json.as_bytes())
            .map_err(|e| HeadlessError::Io(e.to_string()))?;
        self.writer
            .write_all(b"\n")
            .map_err(|e| HeadlessError::Io(e.to_string()))?;
        self.writer
            .flush()
            .map_err(|e| HeadlessError::Io(e.to_string()))?;
        Ok(())
    }
}

/// Flatten content blocks into a serializable JSON array.
fn content_blocks_to_json(blocks: &[ContentBlock]) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text(t) => serde_json::json!({ "type": "text", "text": t.text }),
            ContentBlock::ToolUse(u) => serde_json::json!({
                "type": "tool_use",
                "id": u.id,
                "name": u.name,
                "input": u.input,
            }),
            ContentBlock::ToolResult(r) => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": r.tool_use_id,
                "is_error": r.is_error,
                "content": content_blocks_to_json(&r.content),
            }),
            ContentBlock::Thinking(t) => serde_json::json!({
                "type": "thinking",
                "thinking": t.thinking,
            }),
            ContentBlock::Image(_) => serde_json::json!({ "type": "image" }),
        })
        .collect();
    serde_json::Value::Array(arr)
}

fn extract_user_text(msg: &Message) -> String {
    let mut out = String::new();
    for b in &msg.content {
        if let ContentBlock::Text(t) = b {
            out.push_str(&t.text);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_adr_table() {
        assert_eq!(exit_code_for(&HeadlessError::MaxTurnsExceeded(0)), 130);
        assert_eq!(
            exit_code_for(&HeadlessError::BudgetExceeded { limit: Some(0.01) }),
            137,
        );
        assert_eq!(
            exit_code_for(&HeadlessError::StdinTooLarge {
                limit_bytes: 10 * 1024 * 1024
            }),
            78,
        );
        assert_eq!(exit_code_for(&HeadlessError::Cancelled), 124);
        assert_eq!(
            exit_code_for(&HeadlessError::ResumeNotFound("x".into())),
            66,
        );
        assert_eq!(exit_code_for(&HeadlessError::NoSessionsToContinue), 66);
        assert_eq!(
            exit_code_for(&HeadlessError::Configuration("bad".into())),
            78,
        );
        assert_eq!(exit_code_for(&HeadlessError::Run("e".into())), 1);
        assert_eq!(
            exit_code_for(&HeadlessError::SchemaValidation("e".into())),
            2,
        );
    }

    #[test]
    fn content_blocks_to_json_serializes_text() {
        use caliban_provider::TextBlock;
        let blocks = vec![ContentBlock::Text(TextBlock {
            text: "hi".into(),
            cache_control: None,
        })];
        let v = content_blocks_to_json(&blocks);
        assert_eq!(v[0]["type"], "text");
        assert_eq!(v[0]["text"], "hi");
    }
}
