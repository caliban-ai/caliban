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
    /// `--input-format stream-json` stdin contained no `user` frames.
    #[error("no user frame found in stream-json stdin input")]
    NoUserInput,
}

/// Map a [`HeadlessError`] to a process exit code per ADR 0025.
#[must_use]
pub(crate) fn exit_code_for(err: &HeadlessError) -> i32 {
    match err {
        HeadlessError::MaxTurnsExceeded(_) => 130,
        HeadlessError::BudgetExceeded { .. } => 137,
        HeadlessError::StdinTooLarge { .. } | HeadlessError::Configuration(_) => 78,
        HeadlessError::ResumeNotFound(_)
        | HeadlessError::NoSessionsToContinue
        | HeadlessError::NoUserInput => 66,
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
    /// Cumulative cost USD; real now that ADR 0033 wired `caliban-telemetry`
    /// pricing into `BudgetTracker`. Unknown (provider, model) pairs still
    /// contribute `$0.00` with a debounced `tracing::warn!`.
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

/// A non-`EndOfTurn` terminal stop reported by [`HeadlessDriver::run_single_pass`].
///
/// The outer driver (single-frame `run` or multi-frame `run_frames`) decides
/// how to surface it — typically by emitting one final `result` frame and
/// returning the matching [`HeadlessError`].
#[derive(Debug, Clone)]
enum TerminalStop {
    /// `--max-turns` (or the agent's own cap) was reached.
    MaxTurns(u32),
    /// Run was cancelled (Ctrl-C / SIGTERM).
    Cancelled,
    /// Provider error / hook denial / compaction failure surfaced as
    /// `StopCondition::ProviderError | HookDenied | CompactionFailed`.
    RunError(String),
    /// `--max-budget-usd` was exceeded after a turn ended.
    BudgetExceeded,
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
    pub(crate) async fn run(
        &mut self,
        agent: Arc<Agent>,
        messages: Vec<Message>,
        cancel: CancellationToken,
    ) -> Result<HeadlessRunSummary, HeadlessError> {
        self.emit_init()?;
        // Drain any hook frames captured before the run began (e.g.
        // `SessionStart`). Must happen after `emit_init` so the init
        // frame stays first in the NDJSON stream.
        self.flush_hook_events()?;
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

        let mut final_text = String::new();
        let mut turns: u32 = 0;
        let mut at_column_zero = true;

        let outcome = self
            .run_single_pass(
                Arc::clone(&agent),
                messages,
                cancel,
                &mut final_text,
                &mut turns,
                &mut at_column_zero,
            )
            .await?;
        if let Some(terminal) = outcome {
            // The agent loop terminated for a reason other than `EndOfTurn`.
            // Emit the matching final `result` frame and surface the error
            // so the binary picks the right exit code.
            return self.emit_terminal_result(&terminal, &final_text, turns);
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

    /// Handle one event from an in-flight agent stream.
    ///
    /// Returns `Ok(Some(stop))` when the event was a `RunEnd` with a
    /// non-`EndOfTurn` stop condition. The caller (single-frame `run` or
    /// multi-frame `run_frames`) is responsible for emitting the final
    /// `result` frame for that stop. Returns `Ok(None)` otherwise.
    fn handle_event(
        &mut self,
        event: TurnEvent,
        final_text: &mut String,
        turns: &mut u32,
        at_column_zero: &mut bool,
    ) -> Result<Option<TerminalStop>, HeadlessError> {
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
                let (provider, model) = split_model_summary(&self.config.model_summary);
                self.config
                    .budget
                    .record_with_model(&usage, 0.0, provider, model);
                if matches!(self.config.output_format, OutputFormat::StreamJson)
                    && !self.config.include_partial_messages
                {
                    let content_value = content_blocks_to_json(&assistant_message.content);
                    self.write_ndjson(&events::assistant_message(content_value))?;
                }
            }
            TurnEvent::RunEnd { stopped_for, .. } => {
                // Each non-EndOfTurn variant terminates the run. We hand the
                // matching `TerminalStop` up to the caller so a single final
                // `result` frame can be emitted (correct for both single-frame
                // and multi-frame stream-json input).
                match stopped_for {
                    StopCondition::EndOfTurn => {}
                    StopCondition::MaxTurnsReached(n) => {
                        return Ok(Some(TerminalStop::MaxTurns(n)));
                    }
                    StopCondition::Cancelled => {
                        return Ok(Some(TerminalStop::Cancelled));
                    }
                    StopCondition::ProviderError(msg)
                    | StopCondition::HookDenied(msg)
                    | StopCondition::CompactionFailed(msg) => {
                        // ADR 0025: error variants emit a `result` frame with
                        // subtype=error + populated `error` field, and the
                        // process exits 1. Mirrors the schema-validation
                        // failure path (H-9 evidence).
                        return Ok(Some(TerminalStop::RunError(msg.clone())));
                    }
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
        Ok(None)
    }

    /// Stream one agent pass to completion, updating shared accumulators.
    ///
    /// Returns `Ok(None)` when the stream ended with `StopCondition::EndOfTurn`
    /// (the agent finished a turn naturally). Returns `Ok(Some(stop))` when
    /// the agent reported a non-`EndOfTurn` terminal stop or the budget was
    /// exceeded mid-stream — the caller decides how to surface it.
    ///
    /// Stream errors are mapped to [`HeadlessError::Run`].
    async fn run_single_pass(
        &mut self,
        agent: Arc<Agent>,
        messages: Vec<Message>,
        cancel: CancellationToken,
        final_text: &mut String,
        turns: &mut u32,
        at_column_zero: &mut bool,
    ) -> Result<Option<TerminalStop>, HeadlessError> {
        let mut stream = agent.stream_until_done(messages, cancel);
        while let Some(event_result) = stream.next().await {
            let event = event_result.map_err(|e| HeadlessError::Run(e.to_string()))?;
            if let Some(stop) = self.handle_event(event, final_text, turns, at_column_zero)? {
                return Ok(Some(stop));
            }
            self.flush_hook_events()?;
            if self.config.budget.is_exceeded() {
                return Ok(Some(TerminalStop::BudgetExceeded));
            }
        }
        Ok(None)
    }

    /// Emit the final `result` frame for a non-`EndOfTurn` terminal stop and
    /// return the matching [`HeadlessError`] so the binary can pick an exit
    /// code via [`exit_code_for`].
    fn emit_terminal_result(
        &mut self,
        stop: &TerminalStop,
        final_text: &str,
        turns: u32,
    ) -> Result<HeadlessRunSummary, HeadlessError> {
        let (i_tok, o_tok) = self.config.budget.total_tokens();
        let total_input_tokens = u32::try_from(i_tok).unwrap_or(u32::MAX);
        let total_output_tokens = u32::try_from(o_tok).unwrap_or(u32::MAX);
        let total_cost_usd = self.config.budget.total_cost_usd();
        let (subtype, error) = match stop {
            TerminalStop::MaxTurns(_) => (ResultSubtype::MaxTurns, None),
            TerminalStop::Cancelled => (ResultSubtype::Cancelled, None),
            TerminalStop::RunError(msg) => (ResultSubtype::Error, Some(msg.clone())),
            TerminalStop::BudgetExceeded => (ResultSubtype::BudgetExceeded, None),
        };
        let summary = HeadlessRunSummary {
            subtype,
            final_text: final_text.to_string(),
            turns,
            total_input_tokens,
            total_output_tokens,
            total_cost_usd,
            structured_output: None,
            error,
        };
        self.emit_result(&summary)?;
        match stop {
            TerminalStop::MaxTurns(n) => Err(HeadlessError::MaxTurnsExceeded(*n)),
            TerminalStop::Cancelled => Err(HeadlessError::Cancelled),
            TerminalStop::RunError(msg) => Err(HeadlessError::Run(msg.clone())),
            TerminalStop::BudgetExceeded => Err(HeadlessError::BudgetExceeded {
                limit: self.config.budget.max_usd(),
            }),
        }
    }

    /// Multi-frame driver entry point for `--input-format stream-json`.
    ///
    /// Emits exactly one `system/init` frame at start and one final `result`
    /// frame at end. Reads NDJSON lines from `input` and runs one agent pass
    /// per `User` frame, accumulating turn counts across frames. `Control`
    /// frames currently log a stderr warning (best-effort interrupt support
    /// is deferred). EOF with zero `User` frames returns
    /// [`HeadlessError::NoUserInput`]. A parse failure mid-stream flushes
    /// the in-flight agent frames, emits a result frame with subtype=error,
    /// and returns [`HeadlessError::InputParse`].
    ///
    /// Replaces the legacy single-shot `for ... break;` loop that processed
    /// only the first user frame (lmstudio Finding 10).
    ///
    /// # Errors
    /// See variants of [`HeadlessError`]. On success returns the cumulative
    /// summary.
    #[allow(
        clippy::too_many_lines,
        reason = "linear stream-json loop is clearer in one body"
    )]
    pub(crate) async fn run_frames(
        &mut self,
        agent: Arc<Agent>,
        base_messages: Vec<Message>,
        input: &str,
        cancel: CancellationToken,
    ) -> Result<HeadlessRunSummary, HeadlessError> {
        self.emit_init()?;
        self.flush_hook_events()?;

        let mut messages = base_messages;
        let mut final_text = String::new();
        let mut turns: u32 = 0;
        let mut at_column_zero = true;
        let mut consumed_user_frames: u32 = 0;

        for raw_line in input.lines() {
            // Parse one line at a time so prior turns' frames are already
            // flushed before a parse error abort.
            let parsed = match input::parse_input_line(raw_line) {
                Ok(opt) => opt,
                Err(HeadlessError::InputParse(msg)) => {
                    // Flush prior turns + emit one final error result frame.
                    let (i_tok, o_tok) = self.config.budget.total_tokens();
                    let summary = HeadlessRunSummary {
                        subtype: ResultSubtype::Error,
                        final_text: final_text.clone(),
                        turns,
                        total_input_tokens: u32::try_from(i_tok).unwrap_or(u32::MAX),
                        total_output_tokens: u32::try_from(o_tok).unwrap_or(u32::MAX),
                        total_cost_usd: self.config.budget.total_cost_usd(),
                        structured_output: None,
                        error: Some(msg.clone()),
                    };
                    self.emit_result(&summary)?;
                    return Err(HeadlessError::InputParse(msg));
                }
                Err(e) => return Err(e),
            };
            let Some(frame) = parsed else { continue };
            match frame {
                events::InputFrame::User { content } => {
                    let text = events::InputFrame::extract_text(&content);
                    self.emit_user_echo(&text)?;
                    messages.push(Message::user_text(text));
                    consumed_user_frames += 1;
                    // Reset `final_text` per frame so the trailing `result`
                    // frame reflects the *last* turn's assistant reply
                    // (not a concatenation across every frame).
                    final_text.clear();
                    let outcome = self
                        .run_single_pass(
                            Arc::clone(&agent),
                            messages.clone(),
                            cancel.clone(),
                            &mut final_text,
                            &mut turns,
                            &mut at_column_zero,
                        )
                        .await?;
                    if let Some(terminal) = outcome {
                        return self.emit_terminal_result(&terminal, &final_text, turns);
                    }
                }
                events::InputFrame::Control { subtype } => {
                    // Best-effort interrupt support is deferred (ADR 0025);
                    // surface a stderr warning so operators don't think it
                    // silently took effect.
                    eprintln!(
                        "[caliban] stream-json control/{subtype} frame received; \
                         interrupts are not yet honored (ADR 0025 deferral)"
                    );
                }
            }
        }

        if consumed_user_frames == 0 {
            let (i_tok, o_tok) = self.config.budget.total_tokens();
            let summary = HeadlessRunSummary {
                subtype: ResultSubtype::Error,
                final_text: String::new(),
                turns: 0,
                total_input_tokens: u32::try_from(i_tok).unwrap_or(u32::MAX),
                total_output_tokens: u32::try_from(o_tok).unwrap_or(u32::MAX),
                total_cost_usd: self.config.budget.total_cost_usd(),
                structured_output: None,
                error: Some("no user frame found in stream-json stdin input".to_string()),
            };
            self.emit_result(&summary)?;
            return Err(HeadlessError::NoUserInput);
        }

        // Structured-output validation runs on the cumulative `final_text`
        // (i.e. the assistant's final-turn reply, since each pass resets it).
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

/// Split a `provider/model` summary into its parts. Falls back to empty
/// strings if the format is unexpected (which short-circuits rate-card lookup
/// in [`BudgetTracker::record_with_model`]).
fn split_model_summary(summary: &str) -> (&str, &str) {
    summary.split_once('/').unwrap_or((summary, ""))
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

    // -------------------------------------------------------------------
    // Shared test-mod imports + helpers, used by both:
    // - Finding 8 regression (`run_emits_exactly_one_system_init_frame`)
    // - Findings 5 + 9 RunEnd.stopped_for surfacing tests
    // -------------------------------------------------------------------

    use async_trait::async_trait;
    use caliban_agent_core::{Agent, Compactor, Hooks, NoopHooks, RunCtx, ToolRegistry};
    use caliban_provider::{
        Capabilities, Message, MockProvider, Provider, StopReason, StreamEvent,
        StreamingContentType, StreamingDelta, Usage,
    };
    use tokio_util::sync::CancellationToken;

    /// Stream that natively ends with `EndTurn`. Reused by the
    /// `HookDenied` / `CompactionFailed` tests where the provider should
    /// never be consulted, but enqueuing a benign response keeps the
    /// agent loop from panicking if it ever advances past the gate.
    fn benign_text_stream() -> Vec<caliban_provider::error::Result<StreamEvent>> {
        vec![
            Ok(StreamEvent::MessageStart {
                id: "msg_1".into(),
                model: "mock".into(),
            }),
            Ok(StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamingContentType::Text,
            }),
            Ok(StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::Text("ok".into()),
            }),
            Ok(StreamEvent::ContentBlockStop { index: 0 }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: Some(StopReason::EndTurn),
                usage_delta: Some(Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                }),
            }),
            Ok(StreamEvent::MessageStop),
        ]
    }

    /// Regression test for Finding 8 (lmstudio probe, 2026-05-25):
    /// `HeadlessDriver::run` must emit exactly one `system/init` frame
    /// per stream-json run. Previously the bin emitted one externally
    /// before calling `run()` and `run()` itself emitted a second.
    #[tokio::test]
    async fn run_emits_exactly_one_system_init_frame() {
        let mock = Arc::new(MockProvider::new());
        mock.enqueue_stream(benign_text_stream());

        let provider_dyn: Arc<dyn Provider + Send + Sync> = mock;
        let agent = Agent::builder()
            .provider(provider_dyn)
            .tools(ToolRegistry::new())
            .model("mock")
            .max_tokens(64)
            .max_turns(2)
            .build()
            .expect("agent builder");
        let agent = Arc::new(agent);

        let config = HeadlessRunConfig::minimal(OutputFormat::StreamJson);
        let buf: Vec<u8> = Vec::new();
        let mut driver = HeadlessDriver::new(buf, config);

        let messages = vec![Message::user_text("hi")];
        let _summary = driver
            .run(agent, messages, CancellationToken::new())
            .await
            .expect("driver run succeeded");

        let bytes = driver.writer;
        let text = String::from_utf8(bytes).expect("valid utf-8");
        let init_count = text
            .lines()
            .filter(|line| !line.is_empty())
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|v| {
                v.get("type") == Some(&serde_json::json!("system"))
                    && v.get("subtype") == Some(&serde_json::json!("init"))
            })
            .count();
        assert_eq!(
            init_count, 1,
            "expected exactly one system/init frame per stream-json run, got {init_count}.\n\
             Output was:\n{text}",
        );
    }

    fn build_agent_with(
        mock: Arc<MockProvider>,
        hooks: Option<Arc<dyn Hooks + Send + Sync>>,
        compactor: Option<Arc<dyn Compactor + Send + Sync>>,
        max_turns: u32,
    ) -> Arc<Agent> {
        let provider: Arc<dyn Provider + Send + Sync> = mock;
        let mut builder = Agent::builder()
            .provider(provider)
            .tools(ToolRegistry::new())
            .model("mock")
            .max_tokens(64)
            .max_turns(max_turns);
        if let Some(h) = hooks {
            builder = builder.hooks(h);
        }
        if let Some(c) = compactor {
            builder = builder.compactor(c);
        }
        Arc::new(builder.build().expect("agent builder"))
    }

    /// Parse the captured driver output into a single JSON value. The
    /// JSON output format emits one object terminated by a newline.
    fn parse_json_frame(buf: &[u8]) -> serde_json::Value {
        let s = std::str::from_utf8(buf).expect("driver output not utf-8");
        let line = s.trim_end_matches('\n');
        serde_json::from_str(line).expect("driver output not valid JSON")
    }

    #[tokio::test]
    async fn run_end_provider_error_emits_error_subtype_and_returns_run_err() {
        let mock = Arc::new(MockProvider::new());
        // Trigger ProviderError via a non-retryable Auth error.
        mock.enqueue_stream_error(caliban_provider::Error::Auth("bad key".into()));
        let agent = build_agent_with(Arc::clone(&mock), None, None, 10);

        let mut buf: Vec<u8> = Vec::new();
        let mut driver =
            HeadlessDriver::new(&mut buf, HeadlessRunConfig::minimal(OutputFormat::Json));
        let err = driver
            .run(
                agent,
                vec![Message::user_text("hi")],
                CancellationToken::new(),
            )
            .await
            .expect_err("provider error should surface as Run err");
        assert!(
            matches!(&err, HeadlessError::Run(msg) if msg.contains("authentication")),
            "expected Run(authentication…), got {err:?}",
        );
        assert_eq!(exit_code_for(&err), 1);

        let frame = parse_json_frame(&buf);
        assert_eq!(frame["type"], "result");
        assert_eq!(frame["subtype"], "error");
        assert!(
            frame["error"]
                .as_str()
                .unwrap_or_default()
                .contains("authentication"),
            "result.error should mention authentication, got {frame}",
        );
    }

    /// Hook that fails `before_run` so the runloop surfaces a `HookDenied`
    /// stop condition immediately (no provider call).
    struct FailingBeforeRun;
    #[async_trait]
    impl Hooks for FailingBeforeRun {
        async fn before_run(&self, _ctx: &RunCtx<'_>) -> caliban_agent_core::Result<()> {
            Err(caliban_agent_core::Error::HookFailed(
                "policy: run blocked".into(),
            ))
        }
    }

    #[tokio::test]
    async fn run_end_hook_denied_emits_error_subtype_and_returns_run_err() {
        let mock = Arc::new(MockProvider::new());
        // The provider should never be consulted, but enqueue a benign
        // response so a regression that bypasses the hook doesn't panic.
        mock.enqueue_stream(benign_text_stream());
        let hooks: Arc<dyn Hooks + Send + Sync> = Arc::new(FailingBeforeRun);
        let agent = build_agent_with(Arc::clone(&mock), Some(hooks), None, 10);

        let mut buf: Vec<u8> = Vec::new();
        let mut driver =
            HeadlessDriver::new(&mut buf, HeadlessRunConfig::minimal(OutputFormat::Json));
        let err = driver
            .run(
                agent,
                vec![Message::user_text("hi")],
                CancellationToken::new(),
            )
            .await
            .expect_err("hook denial should surface as Run err");
        assert!(
            matches!(&err, HeadlessError::Run(msg) if msg.contains("policy: run blocked")),
            "expected Run(…policy: run blocked…), got {err:?}",
        );
        assert_eq!(exit_code_for(&err), 1);

        let frame = parse_json_frame(&buf);
        assert_eq!(frame["subtype"], "error");
        let error_str = frame["error"].as_str().unwrap_or_default();
        assert!(
            error_str.contains("policy: run blocked"),
            "result.error should include hook message, got {error_str}",
        );
    }

    /// Compactor that always fails. Triggers `StopCondition::CompactionFailed`
    /// on the runloop's first compaction attempt (turn 0).
    struct FailingCompactor;
    #[async_trait]
    impl Compactor for FailingCompactor {
        async fn compact(
            &self,
            _messages: &[Message],
            _capabilities: &Capabilities,
        ) -> caliban_agent_core::Result<Option<Vec<Message>>> {
            Err(caliban_agent_core::Error::Compaction(
                "compactor: ran out of budget".into(),
            ))
        }
        fn strategy_name(&self) -> &'static str {
            "FailingCompactor"
        }
    }

    #[tokio::test]
    async fn run_end_compaction_failed_emits_error_subtype_and_returns_run_err() {
        let mock = Arc::new(MockProvider::new());
        mock.enqueue_stream(benign_text_stream());
        let hooks: Arc<dyn Hooks + Send + Sync> = Arc::new(NoopHooks);
        let compactor: Arc<dyn Compactor + Send + Sync> = Arc::new(FailingCompactor);
        let agent = build_agent_with(Arc::clone(&mock), Some(hooks), Some(compactor), 10);

        let mut buf: Vec<u8> = Vec::new();
        let mut driver =
            HeadlessDriver::new(&mut buf, HeadlessRunConfig::minimal(OutputFormat::Json));
        let err = driver
            .run(
                agent,
                vec![Message::user_text("hi")],
                CancellationToken::new(),
            )
            .await
            .expect_err("compaction failure should surface as Run err");
        assert!(
            matches!(&err, HeadlessError::Run(msg) if msg.contains("ran out of budget")),
            "expected Run(…ran out of budget…), got {err:?}",
        );
        assert_eq!(exit_code_for(&err), 1);

        let frame = parse_json_frame(&buf);
        assert_eq!(frame["subtype"], "error");
        assert!(
            frame["error"]
                .as_str()
                .unwrap_or_default()
                .contains("ran out of budget"),
            "result.error should include compactor message, got {frame}",
        );
    }

    #[tokio::test]
    async fn run_end_max_turns_emits_max_turns_subtype_and_exits_130() {
        // Drive max_turns=1 with a tool-using response so the loop wants
        // to continue but hits the cap. The driver short-circuits when
        // max_turns is configured to 0; max_turns=1 reaches the model
        // call but breaks after the single turn.
        let mock = Arc::new(MockProvider::new());
        // Single turn that asks for a tool call; without a registered
        // tool the runloop still records the turn, then sees max_turns
        // exhausted on the second pass.
        mock.enqueue_stream(vec![
            Ok(StreamEvent::MessageStart {
                id: "msg_1".into(),
                model: "mock".into(),
            }),
            Ok(StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamingContentType::Text,
            }),
            Ok(StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::Text("loop".into()),
            }),
            Ok(StreamEvent::ContentBlockStop { index: 0 }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: Some(StopReason::ToolUse),
                usage_delta: Some(Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                }),
            }),
            Ok(StreamEvent::MessageStop),
        ]);
        let agent = build_agent_with(Arc::clone(&mock), None, None, 1);

        let mut buf: Vec<u8> = Vec::new();
        let mut driver =
            HeadlessDriver::new(&mut buf, HeadlessRunConfig::minimal(OutputFormat::Json));
        let err = driver
            .run(
                agent,
                vec![Message::user_text("hi")],
                CancellationToken::new(),
            )
            .await
            .expect_err("max_turns should surface as MaxTurnsExceeded");
        assert!(
            matches!(&err, HeadlessError::MaxTurnsExceeded(1)),
            "expected MaxTurnsExceeded(1), got {err:?}",
        );
        assert_eq!(exit_code_for(&err), 130);

        let frame = parse_json_frame(&buf);
        assert_eq!(frame["subtype"], "max_turns");
        assert!(
            frame["error"].is_null(),
            "max_turns frame should not carry error, got {frame}",
        );
    }

    #[tokio::test]
    async fn run_end_cancelled_emits_cancelled_subtype_and_exits_124() {
        let mock = Arc::new(MockProvider::new());
        mock.enqueue_stream(benign_text_stream());
        let agent = build_agent_with(Arc::clone(&mock), None, None, 10);

        let cancel = CancellationToken::new();
        // Cancel before the run begins; before_run is invoked first, but
        // the loop's first action after that is a cancellation check
        // which transitions to StopCondition::Cancelled.
        cancel.cancel();

        let mut buf: Vec<u8> = Vec::new();
        let mut driver =
            HeadlessDriver::new(&mut buf, HeadlessRunConfig::minimal(OutputFormat::Json));
        let err = driver
            .run(agent, vec![Message::user_text("hi")], cancel)
            .await
            .expect_err("pre-cancelled run should surface as Cancelled");
        assert!(
            matches!(err, HeadlessError::Cancelled),
            "expected Cancelled, got {err:?}",
        );
        assert_eq!(exit_code_for(&err), 124);

        let frame = parse_json_frame(&buf);
        assert_eq!(frame["subtype"], "cancelled");
        assert!(frame["error"].is_null());
    }

    // -------------------------------------------------------------------
    // Finding 10 — `--input-format stream-json` multi-frame loop.
    //
    // Tests for `HeadlessDriver::run_frames`, which iterates NDJSON `User`
    // frames from stdin, runs the agent once per frame, and emits a single
    // `system/init` + a single final `result` frame per stream-json run.
    // -------------------------------------------------------------------

    /// Build a single-turn assistant text stream that says `text`. Each
    /// stream-json `User` frame triggers a fresh agent run that consumes
    /// one of these enqueued streams.
    fn text_turn_stream(text: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
        vec![
            Ok(StreamEvent::MessageStart {
                id: "msg".into(),
                model: "mock".into(),
            }),
            Ok(StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamingContentType::Text,
            }),
            Ok(StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::Text(text.into()),
            }),
            Ok(StreamEvent::ContentBlockStop { index: 0 }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: Some(StopReason::EndTurn),
                usage_delta: Some(Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                }),
            }),
            Ok(StreamEvent::MessageStop),
        ]
    }

    /// Collect every NDJSON line in `buf` as a `serde_json::Value`.
    fn parse_ndjson_lines(buf: &[u8]) -> Vec<serde_json::Value> {
        let s = std::str::from_utf8(buf).expect("driver output not utf-8");
        s.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("ndjson line not JSON"))
            .collect()
    }

    /// Two `user` frames on stdin → two assistant turns + one final
    /// `result` frame; the result records `turns: 2` and only one
    /// `system/init` is emitted.
    #[tokio::test]
    async fn run_frames_two_user_frames_produces_two_turns_and_one_result() {
        let mock = Arc::new(MockProvider::new());
        mock.enqueue_stream(text_turn_stream("alpha"));
        mock.enqueue_stream(text_turn_stream("beta"));
        let agent = build_agent_with(Arc::clone(&mock), None, None, 10);

        let mut buf: Vec<u8> = Vec::new();
        let mut driver = HeadlessDriver::new(
            &mut buf,
            HeadlessRunConfig::minimal(OutputFormat::StreamJson),
        );

        let stdin = "{\"type\":\"user\",\"content\":\"first\"}\n\
                     {\"type\":\"user\",\"content\":\"second\"}\n";
        let summary = driver
            .run_frames(agent, Vec::new(), stdin, CancellationToken::new())
            .await
            .expect("multi-frame run succeeded");

        assert_eq!(summary.turns, 2, "two user frames must produce two turns");
        assert_eq!(summary.subtype, ResultSubtype::Success);

        let frames = parse_ndjson_lines(&buf);
        let init_count = frames
            .iter()
            .filter(|v| v["type"] == "system" && v["subtype"] == "init")
            .count();
        assert_eq!(init_count, 1, "exactly one system/init frame per run");

        let result_count = frames.iter().filter(|v| v["type"] == "result").count();
        assert_eq!(result_count, 1, "exactly one final result frame per run");

        // Final result frame is the last line.
        let last = frames.last().expect("at least one frame");
        assert_eq!(last["type"], "result");
        assert_eq!(last["subtype"], "success");
        assert_eq!(last["turns"], 2);

        // Two assistant `message` frames should appear, one per turn.
        let message_count = frames
            .iter()
            .filter(|v| v["type"] == "message" && v["role"] == "assistant")
            .count();
        assert_eq!(
            message_count, 2,
            "two assistant messages, one per user frame"
        );
    }

    /// One `user` frame on stdin → one assistant turn + one final result
    /// (regression: preserves the prior single-frame behavior).
    #[tokio::test]
    async fn run_frames_single_user_frame_produces_one_turn() {
        let mock = Arc::new(MockProvider::new());
        mock.enqueue_stream(text_turn_stream("only"));
        let agent = build_agent_with(Arc::clone(&mock), None, None, 10);

        let mut buf: Vec<u8> = Vec::new();
        let mut driver = HeadlessDriver::new(
            &mut buf,
            HeadlessRunConfig::minimal(OutputFormat::StreamJson),
        );

        let stdin = "{\"type\":\"user\",\"content\":\"only one\"}\n";
        let summary = driver
            .run_frames(agent, Vec::new(), stdin, CancellationToken::new())
            .await
            .expect("single-frame run succeeded");

        assert_eq!(summary.turns, 1);
        assert_eq!(summary.subtype, ResultSubtype::Success);

        let frames = parse_ndjson_lines(&buf);
        let init_count = frames
            .iter()
            .filter(|v| v["type"] == "system" && v["subtype"] == "init")
            .count();
        assert_eq!(init_count, 1);
        let result_count = frames.iter().filter(|v| v["type"] == "result").count();
        assert_eq!(result_count, 1);
    }

    /// Empty stdin → no agent turn, init + result frame still emitted,
    /// subtype indicates the absence of input, and the run surfaces an
    /// error that maps to exit 66 (`EX_NOINPUT`).
    #[tokio::test]
    async fn run_frames_empty_stdin_emits_error_subtype_no_input() {
        let mock = Arc::new(MockProvider::new());
        // No streams enqueued — the agent should never be consulted.
        let agent = build_agent_with(Arc::clone(&mock), None, None, 10);

        let mut buf: Vec<u8> = Vec::new();
        let mut driver = HeadlessDriver::new(
            &mut buf,
            HeadlessRunConfig::minimal(OutputFormat::StreamJson),
        );

        let err = driver
            .run_frames(agent, Vec::new(), "", CancellationToken::new())
            .await
            .expect_err("empty stdin must surface as NoUserInput");
        assert!(
            matches!(err, HeadlessError::NoUserInput),
            "expected NoUserInput, got {err:?}",
        );
        assert_eq!(exit_code_for(&err), 66);

        let frames = parse_ndjson_lines(&buf);
        // init + result, in that order. Nothing else.
        assert_eq!(frames.len(), 2, "init + result only, got {frames:?}");
        assert_eq!(frames[0]["type"], "system");
        assert_eq!(frames[0]["subtype"], "init");
        assert_eq!(frames[1]["type"], "result");
        assert_eq!(frames[1]["subtype"], "error");
        assert_eq!(frames[1]["turns"], 0);
        let error_str = frames[1]["error"].as_str().unwrap_or_default();
        assert!(
            error_str.contains("no user frame"),
            "result.error should mention no user frame; got {error_str}",
        );
    }

    /// Malformed frame mid-stream → prior turns' assistant frames are
    /// already flushed; the driver emits a final `result` frame with
    /// subtype=error + the parse error in `error`, and returns
    /// `HeadlessError::InputParse` (exit 64).
    #[tokio::test]
    async fn run_frames_malformed_mid_stream_flushes_prior_turns_then_errors() {
        let mock = Arc::new(MockProvider::new());
        // Only the first user frame should reach the model. The second
        // line is malformed and must abort the run before agent invocation.
        mock.enqueue_stream(text_turn_stream("first"));
        let agent = build_agent_with(Arc::clone(&mock), None, None, 10);

        let mut buf: Vec<u8> = Vec::new();
        let mut driver = HeadlessDriver::new(
            &mut buf,
            HeadlessRunConfig::minimal(OutputFormat::StreamJson),
        );

        let stdin = "{\"type\":\"user\",\"content\":\"good\"}\n\
                     {bad json line\n";
        let err = driver
            .run_frames(agent, Vec::new(), stdin, CancellationToken::new())
            .await
            .expect_err("malformed mid-stream must surface as InputParse");
        assert!(
            matches!(err, HeadlessError::InputParse(_)),
            "expected InputParse, got {err:?}",
        );
        assert_eq!(exit_code_for(&err), 64);

        let frames = parse_ndjson_lines(&buf);
        // Prior turn's assistant `message` frame must already be in the
        // stream (it was flushed before the parse error surfaced).
        let message_count = frames
            .iter()
            .filter(|v| v["type"] == "message" && v["role"] == "assistant")
            .count();
        assert_eq!(message_count, 1, "prior assistant turn must be flushed");

        // Single trailing result frame with subtype=error.
        let result_frames: Vec<&serde_json::Value> =
            frames.iter().filter(|v| v["type"] == "result").collect();
        assert_eq!(result_frames.len(), 1);
        assert_eq!(result_frames[0]["subtype"], "error");
        assert_eq!(
            result_frames[0]["turns"], 1,
            "result must reflect the single completed turn"
        );
        let error_str = result_frames[0]["error"].as_str().unwrap_or_default();
        assert!(
            !error_str.is_empty(),
            "result.error must carry the parse error message",
        );
    }
}
