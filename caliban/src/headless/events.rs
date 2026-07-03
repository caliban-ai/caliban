//! Stream event protocol (ADR 0025).
//!
//! NDJSON frames emitted in `--output-format stream-json` mode, plus the
//! single-object `result` frame returned by `--output-format json`.
//!
//! Field-naming convention:
//! - Fields the ADR explicitly names in `camelCase` (e.g. `hookEventName`,
//!   `hookSpecificOutput`, `settingSources`) stay `camelCase`.
//! - Everything else (`session_id`, `total_cost_usd`, …) is `snake_case` so
//!   downstream `jq` consumers see consistent JSON keys.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level kind of an outbound event frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EventKind {
    /// `system/*` envelope.
    System,
    /// Incremental assistant text delta.
    Text,
    /// Incremental assistant reasoning delta (emitted under
    /// `--include-partial-messages`). Previously absent here, so a consumer
    /// deserializing the frame `type` into `EventKind` failed on it (#184 HL5).
    Thinking,
    /// A tool-use block (assistant calling a tool).
    ToolUse,
    /// A tool-result block (tool reply to the assistant).
    ToolResult,
    /// A full assistant message.
    Message,
    /// A `user` frame (only when `--replay-user-messages`).
    User,
    /// A non-fatal `warning/<subtype>` frame (e.g. model mismatch). Previously
    /// absent here, breaking `EventKind` deserialization (#184 HL5).
    Warning,
    /// A `hook_event` frame (only when `--include-hook-events`).
    HookEvent,
    /// The final `result` frame (always last in stream-json).
    Result,
}

/// Subtype for the final `result` frame.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResultSubtype {
    /// Run completed normally.
    Success,
    /// Run terminated with an error (tool / assistant failure).
    Error,
    /// `--max-turns` exceeded.
    MaxTurns,
    /// `--max-budget-usd` exceeded.
    BudgetExceeded,
    /// Cancelled (Ctrl-C / SIGTERM).
    Cancelled,
    /// Per-turn `max_tokens` budget exhausted with recovery disabled. Distinct
    /// from `Error` so callers can tell a clean budget blowout (the model ran
    /// long, output is partial) from a genuine failure (provider 5xx, hook
    /// denial, tool crash).
    MaxTokens,
}

impl ResultSubtype {
    /// Stable JSON spelling.
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::MaxTurns => "max_turns",
            Self::BudgetExceeded => "budget_exceeded",
            Self::Cancelled => "cancelled",
            Self::MaxTokens => "max_tokens",
        }
    }
}

/// `system/init` payload — the first frame of every stream-json run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SystemInit {
    /// Always `"system"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Always `"init"`.
    pub(crate) subtype: String,
    /// Opaque session identifier.
    pub(crate) session_id: String,
    /// Provider+model summary (e.g. `"anthropic/claude-sonnet-4-7"`).
    pub(crate) model: String,
    /// Registered tool names (alphabetical).
    pub(crate) tools: Vec<String>,
    /// Loaded plugin descriptors (empty until ADR 0030 lands).
    pub(crate) plugins: Vec<Value>,
    /// Settings source-chain (`camelCase` per ADR 0025).
    #[serde(rename = "settingSources")]
    pub(crate) setting_sources: Vec<String>,
    /// MCP server summaries (best-effort).
    #[serde(default)]
    pub(crate) mcp_servers: Vec<Value>,
    /// Whether `--bare` is in effect for this run.
    pub(crate) bare_mode: bool,
    /// Current working directory at run start.
    pub(crate) cwd: String,
    /// Effective permission mode for this run (camelCase per ADR 0029:
    /// `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`,
    /// `bypassPermissions`). The literal string `"disabled"` is emitted
    /// when `--no-permissions` is in effect (no `PermissionsHook` at all).
    /// Surfaces the resolved mode so operators can audit which gate
    /// actually ran (lmstudio Finding 15).
    pub(crate) permission_mode: String,
}

/// `system/api_retry` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SystemApiRetry {
    /// Always `"system"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Always `"api_retry"`.
    pub(crate) subtype: String,
    /// 1-based attempt index.
    pub(crate) attempt: u32,
    /// Maximum retry attempts.
    pub(crate) max_retries: u32,
    /// Delay before this attempt, milliseconds.
    pub(crate) retry_delay_ms: u64,
    /// HTTP status code (when applicable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error_status: Option<u16>,
    /// Category bucket (`overloaded`, `rate_limit`, `timeout`, `network`,
    /// `server_error`, `other`).
    pub(crate) error_category: String,
}

/// `text` delta payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TextDelta {
    /// Always `"text"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Incremental text fragment.
    pub(crate) delta: String,
}

/// `thinking` delta payload — streamed reasoning text from models that emit
/// `reasoning_content` (Qwen3 reasoning variants, `DeepSeek-R1`, `OpenAI`
/// o-series). Mirrors [`TextDelta`]; distinguished by `type` and by the
/// fact that the parent block in the final `message` frame is a
/// `ContentBlock::Thinking` rather than `Text`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ThinkingDelta {
    /// Always `"thinking"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Incremental reasoning fragment.
    pub(crate) delta: String,
}

/// `tool_use` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ToolUse {
    /// Always `"tool_use"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Provider-assigned tool-use ID.
    pub(crate) id: String,
    /// Tool name.
    pub(crate) name: String,
    /// Parsed tool input JSON.
    pub(crate) input: Value,
}

/// `tool_result` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ToolResult {
    /// Always `"tool_result"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// The `tool_use_id` this result is for.
    pub(crate) tool_use_id: String,
    /// Whether the tool errored.
    pub(crate) is_error: bool,
    /// Raw content blocks returned by the tool.
    pub(crate) content: Value,
}

/// `message` payload (full assistant message; emitted when partial-message
/// streaming is off).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AssistantMessage {
    /// Always `"message"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Always `"assistant"`.
    pub(crate) role: String,
    /// Content blocks.
    pub(crate) content: Value,
}

/// `user` payload — echo of a user prompt (when `--replay-user-messages`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UserEcho {
    /// Always `"user"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Content blocks.
    pub(crate) content: Value,
}

/// `hook_event` payload — emitted when `--include-hook-events` is set.
///
/// Field names match ADR 0024 — `hookEventName` and `hookSpecificOutput`
/// stay `camelCase` for Claude Code parity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(
    clippy::struct_field_names,
    reason = "ADR 0024 mandates `hookEventName` / `hookSpecificOutput`"
)]
pub(crate) struct HookEvent {
    /// Always `"hook_event"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Hook event name (e.g. `"SessionStart"`, `"PreToolUse"`).
    #[serde(rename = "hookEventName")]
    pub(crate) hook_event_name: String,
    /// Event-specific payload.
    #[serde(rename = "hookSpecificOutput")]
    pub(crate) hook_specific_output: Value,
}

/// `warning` payload — informational, non-fatal divergences detected
/// mid-stream that operators should see but that don't terminate the run.
///
/// The first subtype is `model_mismatch`: emitted by the run driver when
/// the response's `model` field differs from the requested model (F4 from
/// the 2026-05-27 lmstudio probe — local servers silently substitute a
/// different model for unknown IDs). Future subtypes use the same shape;
/// the `details` map is for subtype-specific payload (e.g. `requested` /
/// `actual` for `model_mismatch`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WarningFrame {
    /// Always `"warning"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Subtype tag (e.g. `"model_mismatch"`).
    pub(crate) subtype: String,
    /// Human-readable message (one line).
    pub(crate) message: String,
    /// Subtype-specific structured data.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub(crate) details: Value,
}

/// Claude-Code-style token usage object, emitted alongside the flat
/// `total_input_tokens` / `total_output_tokens` for drop-in CC compatibility
/// (#222).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UsageTotals {
    pub(crate) input_tokens: u32,
    pub(crate) output_tokens: u32,
}

/// Final `result` frame. Also the single body of `--output-format json`.
///
/// Field semantics by `subtype`:
/// - `success` → `result` carries the assistant's final reply (load-bearing
///   contract; downstream `jq` consumers depend on it). `last_assistant_text`
///   and `tool_calls_seen` are omitted.
/// - All non-`success` subtypes (`error`, `max_turns`, `budget_exceeded`,
///   `cancelled`) → `result` is omitted; consumers should read the
///   structured fields (`last_assistant_text`, `tool_calls_seen`,
///   `error`) instead. This avoids the old behavior where `result` was the
///   raw concatenation of every assistant-text fragment across a truncated
///   run, which couldn't be distinguished from a clean answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResultFrame {
    /// Always `"result"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Outcome category (`success`, `error`, `max_turns`, `budget_exceeded`,
    /// `cancelled`).
    pub(crate) subtype: String,
    /// Final assistant text (best-effort summary). Present only when
    /// `subtype == "success"`; for non-`success` subtypes see
    /// `last_assistant_text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<String>,
    /// Session identifier.
    pub(crate) session_id: String,
    /// Cumulative cost in USD computed by `caliban-telemetry::pricing`
    /// against the active rate card (ADR 0033). `0.0` for unknown
    /// `(provider, model)` pairs.
    pub(crate) total_cost_usd: f64,
    /// Number of completed turns.
    pub(crate) turns: u32,
    /// Total input tokens.
    pub(crate) total_input_tokens: u32,
    /// Total output tokens.
    pub(crate) total_output_tokens: u32,
    /// Claude-Code-contract alias for `turns` (additive; #222).
    pub(crate) num_turns: u32,
    /// `true` for any non-`success` subtype (additive; #222). Lets consumers
    /// branch without enumerating every subtype spelling.
    pub(crate) is_error: bool,
    /// Wall-clock run duration in milliseconds (#222).
    pub(crate) duration_ms: u64,
    /// Claude-Code-style usage object mirroring the flat token totals (#222).
    pub(crate) usage: UsageTotals,
    /// Structured output, when `--json-schema` was set and validation
    /// succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) structured_output: Option<Value>,
    /// Error message, when `subtype == "error"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// Most recent non-empty assistant text body the agent produced.
    /// Emitted for non-`success` subtypes (F7 follow-up); `null` when the
    /// run produced no assistant text. Consumers parsing partial runs read
    /// this instead of `result`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_assistant_text: Option<String>,
    /// Number of `ToolCallEnd` events observed across the entire run.
    /// Emitted for non-`success` subtypes (F7 follow-up); lets consumers
    /// distinguish an empty-but-active run (tool loop) from an empty-and-
    /// idle one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tool_calls_seen: Option<u32>,
    /// High-water mark of consecutive turns without a successful edit-class
    /// (non-read-only) tool call (#239). Always present; `0` means the
    /// agent made at least one successful non-read-only call every turn.
    pub(crate) turns_without_edit: u32,
    /// Whether the no-edit-progress nudge fired at least once this run
    /// (#239). Always present.
    pub(crate) no_edit_nudge_emitted: bool,
}

// ---------------------------------------------------------------------------
// Construction helpers
// ---------------------------------------------------------------------------

/// Build a `system/init` frame.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub(crate) fn system_init(
    session_id: impl Into<String>,
    model: impl Into<String>,
    tools: Vec<String>,
    plugins: Vec<Value>,
    setting_sources: Vec<String>,
    bare_mode: bool,
    cwd: impl Into<String>,
    permission_mode: impl Into<String>,
) -> SystemInit {
    SystemInit {
        kind: "system".into(),
        subtype: "init".into(),
        session_id: session_id.into(),
        model: model.into(),
        tools,
        plugins,
        setting_sources,
        mcp_servers: Vec::new(),
        bare_mode,
        cwd: cwd.into(),
        permission_mode: permission_mode.into(),
    }
}

/// Build a `system/api_retry` frame.
#[must_use]
pub(crate) fn system_api_retry(
    attempt: u32,
    max_retries: u32,
    retry_delay_ms: u64,
    error_status: Option<u16>,
    error_category: impl Into<String>,
) -> SystemApiRetry {
    SystemApiRetry {
        kind: "system".into(),
        subtype: "api_retry".into(),
        attempt,
        max_retries,
        retry_delay_ms,
        error_status,
        error_category: error_category.into(),
    }
}

/// Build a `text` delta frame.
#[must_use]
pub(crate) fn text_delta(delta: impl Into<String>) -> TextDelta {
    TextDelta {
        kind: "text".into(),
        delta: delta.into(),
    }
}

/// Build a `thinking` delta frame. Emitted under `--include-partial-messages`
/// when the upstream model streams `reasoning_content` deltas.
#[must_use]
pub(crate) fn thinking_delta(delta: impl Into<String>) -> ThinkingDelta {
    ThinkingDelta {
        kind: "thinking".into(),
        delta: delta.into(),
    }
}

/// Build a `tool_use` frame.
#[must_use]
pub(crate) fn tool_use(id: impl Into<String>, name: impl Into<String>, input: Value) -> ToolUse {
    ToolUse {
        kind: "tool_use".into(),
        id: id.into(),
        name: name.into(),
        input,
    }
}

/// Build a `tool_result` frame.
#[must_use]
pub(crate) fn tool_result(
    tool_use_id: impl Into<String>,
    is_error: bool,
    content: Value,
) -> ToolResult {
    ToolResult {
        kind: "tool_result".into(),
        tool_use_id: tool_use_id.into(),
        is_error,
        content,
    }
}

/// Build a `message` (full assistant message) frame.
#[must_use]
pub(crate) fn assistant_message(content: Value) -> AssistantMessage {
    AssistantMessage {
        kind: "message".into(),
        role: "assistant".into(),
        content,
    }
}

/// Build a `user` echo frame.
#[must_use]
pub(crate) fn user_echo(content: Value) -> UserEcho {
    UserEcho {
        kind: "user".into(),
        content,
    }
}

/// Build a `warning/model_mismatch` frame (F4 — the OpenAI-compatible
/// response's `model` field differs from the requested model).
#[must_use]
pub(crate) fn warning_model_mismatch(requested: &str, actual: &str) -> WarningFrame {
    WarningFrame {
        kind: "warning".into(),
        subtype: "model_mismatch".into(),
        message: format!(
            "model mismatch: requested {requested:?} but provider responded with {actual:?}"
        ),
        details: serde_json::json!({
            "requested": requested,
            "actual": actual,
        }),
    }
}

/// Build a `hook_event` frame.
#[must_use]
pub(crate) fn hook_event(event_name: impl Into<String>, payload: Value) -> HookEvent {
    HookEvent {
        kind: "hook_event".into(),
        hook_event_name: event_name.into(),
        hook_specific_output: payload,
    }
}

/// Build a final `result` frame.
///
/// Successful runs carry the assistant's reply in `result`; non-`success`
/// runs carry it (best-effort) in `last_assistant_text` along with
/// `tool_calls_seen`, and `result` is omitted from the serialized JSON.
/// This is the F7 follow-up shape — the prior protocol concatenated every
/// assistant fragment into `result` for max-turns runs, which couldn't be
/// distinguished from a clean answer downstream.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub(crate) fn result_frame(
    subtype: ResultSubtype,
    result_text: impl Into<String>,
    session_id: impl Into<String>,
    total_cost_usd: f64,
    turns: u32,
    total_input_tokens: u32,
    total_output_tokens: u32,
    structured_output: Option<Value>,
    error: Option<String>,
    last_assistant_text: Option<String>,
    tool_calls_seen: u32,
    turns_without_edit: u32,
    no_edit_nudge_emitted: bool,
    duration_ms: u64,
) -> ResultFrame {
    let is_success = matches!(subtype, ResultSubtype::Success);
    let result_text: String = result_text.into();
    let (result, last_assistant_text, tool_calls_seen) = if is_success {
        // Success keeps the documented `result` field; structured fields
        // are omitted so the success shape stays unchanged.
        (Some(result_text), None, None)
    } else {
        // Non-success: surface structured fields instead of the raw
        // concatenated `result`. `last_assistant_text` falls back to the
        // existing `result_text` accumulator when the per-turn tracker is
        // empty (e.g. a budget cutoff fires before the first TurnEnd).
        let fallback = if result_text.is_empty() {
            None
        } else {
            Some(result_text)
        };
        let lat = last_assistant_text.or(fallback);
        (None, lat, Some(tool_calls_seen))
    };
    ResultFrame {
        kind: "result".into(),
        subtype: subtype.as_str().into(),
        result,
        session_id: session_id.into(),
        total_cost_usd,
        turns,
        total_input_tokens,
        total_output_tokens,
        num_turns: turns,
        is_error: !is_success,
        duration_ms,
        usage: UsageTotals {
            input_tokens: total_input_tokens,
            output_tokens: total_output_tokens,
        },
        structured_output,
        error,
        last_assistant_text,
        tool_calls_seen,
        turns_without_edit,
        no_edit_nudge_emitted,
    }
}

// ---------------------------------------------------------------------------
// Inbound stream-json frames (from stdin in --input-format stream-json mode)
// ---------------------------------------------------------------------------

/// A parsed line from stdin in `--input-format stream-json` mode.
///
/// Each variant's payload is `deny_unknown_fields` so the driver fails
/// loud on non-caliban shapes (e.g. a Claude-Code-style
/// `{"type":"user","message":{...}}` envelope) rather than silently
/// running the agent with a blank prompt (lmstudio Finding 13). serde
/// doesn't accept `deny_unknown_fields` on enum variants directly, so
/// each variant payload is a named struct and the enum carries the
/// `tag` + `content` discriminator.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InputFrame {
    /// A user message; the driver runs one agent loop pass per `user` frame.
    User(InputUser),
    /// A `control` frame (best-effort interrupt support).
    Control(InputControl),
}

/// Payload of an `InputFrame::User` variant. Standalone struct so
/// `deny_unknown_fields` actually applies (serde rejects it on enum
/// variants).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputUser {
    /// Either a JSON string or an array of content blocks. Both are
    /// accepted; we flatten to text in [`InputFrame::extract_text`].
    pub(crate) content: Value,
}

/// Payload of an `InputFrame::Control` variant.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputControl {
    /// Control subtype (only `"interrupt"` is recognized today).
    pub(crate) subtype: String,
}

impl InputFrame {
    /// Extract a flat text representation of a `User` frame's content.
    ///
    /// - Plain string → returned as-is.
    /// - Array of content blocks → text blocks concatenated.
    /// - Anything else → JSON-stringified.
    #[must_use]
    pub(crate) fn extract_text(content: &Value) -> String {
        if let Some(s) = content.as_str() {
            return s.to_string();
        }
        if let Some(arr) = content.as_array() {
            let mut out = String::new();
            for block in arr {
                if block.get("type").and_then(Value::as_str) == Some("text")
                    && let Some(t) = block.get("text").and_then(Value::as_str)
                {
                    out.push_str(t);
                }
            }
            return out;
        }
        content.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_deserializes_every_emitted_frame_type() {
        // #184 HL5: a consumer classifying frames by `type` must be able to
        // deserialize every type the driver emits — including thinking/warning.
        for (s, want) in [
            ("system", EventKind::System),
            ("text", EventKind::Text),
            ("thinking", EventKind::Thinking),
            ("tool_use", EventKind::ToolUse),
            ("tool_result", EventKind::ToolResult),
            ("message", EventKind::Message),
            ("user", EventKind::User),
            ("warning", EventKind::Warning),
            ("hook_event", EventKind::HookEvent),
            ("result", EventKind::Result),
        ] {
            let got: EventKind = serde_json::from_value(serde_json::json!(s)).unwrap();
            assert_eq!(got, want, "type {s:?} must deserialize");
        }
        // The actual emitted frames carry these `kind` strings.
        assert_eq!(thinking_delta("x").kind, "thinking");
        assert_eq!(warning_model_mismatch("a", "b").kind, "warning");
        assert_eq!(text_delta("x").kind, "text");
    }

    #[test]
    fn system_init_serializes_camelcase_setting_sources() {
        let frame = system_init(
            "sess-1",
            "anthropic/claude",
            vec!["Read".into(), "Write".into()],
            vec![serde_json::json!({"name": "skill-pack", "version": "0.2.0", "source": "user"})],
            vec!["managed".into(), "user".into(), "project".into()],
            false,
            "/tmp",
            "default",
        );
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["type"], "system");
        assert_eq!(json["subtype"], "init");
        assert_eq!(json["session_id"], "sess-1");
        assert!(json.get("settingSources").is_some(), "must be `camelCase`");
        assert_eq!(json["settingSources"][0], "managed");
        assert_eq!(json["settingSources"][2], "project");
        assert_eq!(json["bare_mode"], false);
        assert_eq!(json["cwd"], "/tmp");
        assert_eq!(json["plugins"][0]["name"], "skill-pack");
        assert_eq!(json["permission_mode"], "default");
    }

    #[test]
    fn system_api_retry_serializes_with_category() {
        let frame = system_api_retry(2, 5, 1500, Some(529), "overloaded");
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["type"], "system");
        assert_eq!(json["subtype"], "api_retry");
        assert_eq!(json["attempt"], 2);
        assert_eq!(json["max_retries"], 5);
        assert_eq!(json["retry_delay_ms"], 1500);
        assert_eq!(json["error_status"], 529);
        assert_eq!(json["error_category"], "overloaded");
    }

    #[test]
    fn text_delta_serializes() {
        let frame = text_delta("Hello");
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["delta"], "Hello");
    }

    #[test]
    fn thinking_delta_serializes() {
        let frame = thinking_delta("Let me think...");
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["type"], "thinking");
        assert_eq!(json["delta"], "Let me think...");
    }

    #[test]
    fn thinking_delta_distinguishable_from_text_delta() {
        // Same top-level shape, different `type` discriminator — consumers
        // route on `type`.
        let t = serde_json::to_value(text_delta("hi")).unwrap();
        let r = serde_json::to_value(thinking_delta("hi")).unwrap();
        assert_ne!(t["type"], r["type"]);
        assert_eq!(t["delta"], r["delta"]);
    }

    #[test]
    fn tool_use_serializes() {
        let input = serde_json::json!({"command": "ls"});
        let frame = tool_use("toolu_01", "Bash", input.clone());
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["type"], "tool_use");
        assert_eq!(json["id"], "toolu_01");
        assert_eq!(json["name"], "Bash");
        assert_eq!(json["input"], input);
    }

    #[test]
    fn tool_result_serializes() {
        let content = serde_json::json!([{"type": "text", "text": "ok"}]);
        let frame = tool_result("toolu_01", false, content.clone());
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["tool_use_id"], "toolu_01");
        assert_eq!(json["is_error"], false);
        assert_eq!(json["content"], content);
    }

    #[test]
    fn assistant_message_serializes_with_role() {
        let content = serde_json::json!([{"type": "text", "text": "hi"}]);
        let frame = assistant_message(content.clone());
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["type"], "message");
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["content"], content);
    }

    #[test]
    fn user_echo_serializes() {
        let content = serde_json::json!([{"type": "text", "text": "fix it"}]);
        let frame = user_echo(content.clone());
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["type"], "user");
        assert_eq!(json["content"], content);
    }

    #[test]
    fn hook_event_uses_camelcase_field_names() {
        let payload = serde_json::json!({"matcher": "Bash", "decision": "allow"});
        let frame = hook_event("PreToolUse", payload.clone());
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["type"], "hook_event");
        assert!(
            json.get("hookEventName").is_some(),
            "hookEventName must be `camelCase`",
        );
        assert!(
            json.get("hookSpecificOutput").is_some(),
            "hookSpecificOutput must be `camelCase`",
        );
        assert_eq!(json["hookEventName"], "PreToolUse");
        assert_eq!(json["hookSpecificOutput"], payload);
    }

    #[test]
    fn result_frame_adds_cc_contract_fields() {
        let f = result_frame(
            ResultSubtype::Success,
            "final answer",
            "sess-1",
            0.0,
            3, // turns
            100,
            42,
            None,
            None,
            None,
            0,
            0,
            false,
            1234, // duration_ms
        );
        let v = serde_json::to_value(&f).unwrap();
        // result = the passed final message.
        assert_eq!(v["result"], "final answer");
        // Additive CC keys.
        assert_eq!(v["is_error"], false);
        assert_eq!(v["num_turns"], 3);
        assert_eq!(v["usage"]["input_tokens"], 100);
        assert_eq!(v["usage"]["output_tokens"], 42);
        assert_eq!(v["duration_ms"], 1234);
        // Legacy keys still present (non-breaking).
        assert_eq!(v["turns"], 3);
        assert_eq!(v["total_input_tokens"], 100);
        assert_eq!(v["total_output_tokens"], 42);
    }

    #[test]
    fn result_frame_is_error_true_for_non_success() {
        for st in [
            ResultSubtype::Error,
            ResultSubtype::MaxTurns,
            ResultSubtype::Cancelled,
            ResultSubtype::BudgetExceeded,
            ResultSubtype::MaxTokens,
        ] {
            let f = result_frame(st, "", "s", 0.0, 1, 0, 0, None, None, None, 0, 0, false, 0);
            let v = serde_json::to_value(&f).unwrap();
            assert_eq!(v["is_error"], true, "subtype {st:?} must be is_error=true");
        }
    }

    #[test]
    fn result_frame_success_carries_result_field() {
        let frame = result_frame(
            ResultSubtype::Success,
            "answer",
            "s1",
            0.0,
            1,
            10,
            20,
            None,
            None,
            None,
            0,
            0,
            false,
            0,
        );
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["type"], "result");
        assert_eq!(json["subtype"], "success");
        assert_eq!(json["result"], "answer");
        // Structured fields are suppressed for success — the legacy contract.
        assert!(
            json.get("last_assistant_text").is_none(),
            "success must not carry last_assistant_text, got {json}",
        );
        assert!(
            json.get("tool_calls_seen").is_none(),
            "success must not carry tool_calls_seen, got {json}",
        );
        assert_eq!(json["total_cost_usd"], 0.0);
    }

    #[test]
    fn result_frame_non_success_uses_structured_fields() {
        for (subtype, expected) in [
            (ResultSubtype::Error, "error"),
            (ResultSubtype::MaxTurns, "max_turns"),
            (ResultSubtype::BudgetExceeded, "budget_exceeded"),
            (ResultSubtype::Cancelled, "cancelled"),
            (ResultSubtype::MaxTokens, "max_tokens"),
        ] {
            let frame = result_frame(
                subtype,
                "fragments fragments fragments",
                "s1",
                0.0,
                3,
                10,
                20,
                None,
                None,
                Some("last clean reply".into()),
                7,
                0,
                false,
                0,
            );
            let json = serde_json::to_value(&frame).unwrap();
            assert_eq!(json["type"], "result");
            assert_eq!(json["subtype"], expected);
            // F7 follow-up: non-success drops the concatenated `result`
            // field and exposes structured fields instead.
            assert!(
                json.get("result").is_none(),
                "non-success ({expected}) must not carry `result`, got {json}",
            );
            assert_eq!(json["last_assistant_text"], "last clean reply");
            assert_eq!(json["tool_calls_seen"], 7);
        }
    }

    #[test]
    fn result_frame_non_success_falls_back_to_result_text_when_no_explicit_last() {
        // Caller didn't supply `last_assistant_text`; the builder falls
        // back to the accumulator text so we don't emit `null` when there
        // IS some assistant content the consumer can show.
        let frame = result_frame(
            ResultSubtype::MaxTurns,
            "partial assistant text",
            "s1",
            0.0,
            1,
            10,
            20,
            None,
            None,
            None,
            0,
            0,
            false,
            0,
        );
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["last_assistant_text"], "partial assistant text");
        assert_eq!(json["tool_calls_seen"], 0);
    }

    #[test]
    fn result_frame_non_success_omits_last_assistant_text_when_empty() {
        // No accumulator text and no explicit override → field is absent
        // (rather than `""`) so consumers can distinguish "no observable
        // output" from "empty assistant reply".
        let frame = result_frame(
            ResultSubtype::MaxTurns,
            "",
            "s1",
            0.0,
            0,
            0,
            0,
            None,
            None,
            None,
            0,
            0,
            false,
            0,
        );
        let json = serde_json::to_value(&frame).unwrap();
        assert!(
            json.get("last_assistant_text").is_none(),
            "absent text should drop the field, got {json}",
        );
        assert_eq!(json["tool_calls_seen"], 0);
    }

    #[test]
    fn result_subtype_str_matches_serialization() {
        assert_eq!(ResultSubtype::Success.as_str(), "success");
        assert_eq!(ResultSubtype::MaxTurns.as_str(), "max_turns");
        assert_eq!(ResultSubtype::BudgetExceeded.as_str(), "budget_exceeded");
        assert_eq!(ResultSubtype::MaxTokens.as_str(), "max_tokens");
    }

    #[test]
    fn input_frame_parses_user_string_content() {
        let line = r#"{"type":"user","content":"hello"}"#;
        let frame: InputFrame = serde_json::from_str(line).unwrap();
        match frame {
            InputFrame::User(user) => {
                assert_eq!(InputFrame::extract_text(&user.content), "hello");
            }
            InputFrame::Control(_) => panic!("expected user"),
        }
    }

    #[test]
    fn input_frame_parses_user_block_array() {
        let line = r#"{"type":"user","content":[{"type":"text","text":"abc"},{"type":"text","text":"def"}]}"#;
        let frame: InputFrame = serde_json::from_str(line).unwrap();
        match frame {
            InputFrame::User(user) => {
                assert_eq!(InputFrame::extract_text(&user.content), "abcdef");
            }
            InputFrame::Control(_) => panic!("expected user"),
        }
    }

    #[test]
    fn input_frame_parses_control_interrupt() {
        let line = r#"{"type":"control","subtype":"interrupt"}"#;
        let frame: InputFrame = serde_json::from_str(line).unwrap();
        assert!(matches!(frame, InputFrame::Control(ctrl) if ctrl.subtype == "interrupt"));
    }

    #[test]
    fn input_frame_rejects_unknown_type() {
        let line = r#"{"type":"banana","content":"x"}"#;
        let res: Result<InputFrame, _> = serde_json::from_str(line);
        assert!(res.is_err());
    }

    /// Regression for lmstudio Finding 13: a Claude-Code-shaped envelope
    /// (`{"type":"user","message":{...}}`) must NOT silently parse to a
    /// blank `User` frame. We rely on `#[serde(deny_unknown_fields)]` on
    /// each variant to surface the unknown `message` field as a hard
    /// parse error.
    #[test]
    fn input_frame_rejects_claude_code_envelope_shape() {
        let line =
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#;
        let err = serde_json::from_str::<InputFrame>(line)
            .expect_err("unknown `message` field must error");
        let msg = err.to_string();
        assert!(
            msg.contains("message") || msg.contains("unknown field"),
            "error must name the unknown field; got: {msg}",
        );
    }

    /// Defensive: an extra unrecognized field on a `user` frame is
    /// rejected as well, so consumers can rely on caliban surfacing
    /// shape-drift instead of dropping data.
    #[test]
    fn input_frame_rejects_extra_field_on_user() {
        let line = r#"{"type":"user","content":"hi","extra":"field"}"#;
        let res: Result<InputFrame, _> = serde_json::from_str(line);
        assert!(res.is_err(), "extra field on user frame must error");
    }
}
