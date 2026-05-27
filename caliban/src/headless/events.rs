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
    /// A tool-use block (assistant calling a tool).
    ToolUse,
    /// A tool-result block (tool reply to the assistant).
    ToolResult,
    /// A full assistant message.
    Message,
    /// A `user` frame (only when `--replay-user-messages`).
    User,
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

/// Final `result` frame. Also the single body of `--output-format json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResultFrame {
    /// Always `"result"`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Outcome category (`success`, `error`, `max_turns`, `budget_exceeded`,
    /// `cancelled`).
    pub(crate) subtype: String,
    /// Final assistant text (best-effort summary).
    pub(crate) result: String,
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
    /// Structured output, when `--json-schema` was set and validation
    /// succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) structured_output: Option<Value>,
    /// Error message, when `subtype == "error"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

// ---------------------------------------------------------------------------
// Construction helpers
// ---------------------------------------------------------------------------

/// Build a `system/init` frame.
#[must_use]
pub(crate) fn system_init(
    session_id: impl Into<String>,
    model: impl Into<String>,
    tools: Vec<String>,
    plugins: Vec<Value>,
    setting_sources: Vec<String>,
    bare_mode: bool,
    cwd: impl Into<String>,
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
) -> ResultFrame {
    ResultFrame {
        kind: "result".into(),
        subtype: subtype.as_str().into(),
        result: result_text.into(),
        session_id: session_id.into(),
        total_cost_usd,
        turns,
        total_input_tokens,
        total_output_tokens,
        structured_output,
        error,
    }
}

// ---------------------------------------------------------------------------
// Inbound stream-json frames (from stdin in --input-format stream-json mode)
// ---------------------------------------------------------------------------

/// A parsed line from stdin in `--input-format stream-json` mode.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InputFrame {
    /// A user message; the driver runs one agent loop pass per `user` frame.
    User {
        /// Either a JSON string or an array of content blocks. Both are
        /// accepted; we flatten to text in `extract_text`.
        content: Value,
    },
    /// A `control` frame (best-effort interrupt support).
    Control {
        /// Control subtype (only `"interrupt"` is recognized today).
        subtype: String,
    },
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
    fn system_init_serializes_camelcase_setting_sources() {
        let frame = system_init(
            "sess-1",
            "anthropic/claude",
            vec!["Read".into(), "Write".into()],
            vec![serde_json::json!({"name": "skill-pack", "version": "0.2.0", "source": "user"})],
            vec!["managed".into(), "user".into(), "project".into()],
            false,
            "/tmp",
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
    fn result_frame_serializes_all_subtypes() {
        for (subtype, expected) in [
            (ResultSubtype::Success, "success"),
            (ResultSubtype::Error, "error"),
            (ResultSubtype::MaxTurns, "max_turns"),
            (ResultSubtype::BudgetExceeded, "budget_exceeded"),
            (ResultSubtype::Cancelled, "cancelled"),
        ] {
            let frame = result_frame(subtype, "answer", "s1", 0.0, 1, 10, 20, None, None);
            let json = serde_json::to_value(&frame).unwrap();
            assert_eq!(json["type"], "result");
            assert_eq!(json["subtype"], expected);
            assert_eq!(json["result"], "answer");
            assert_eq!(json["total_cost_usd"], 0.0);
        }
    }

    #[test]
    fn result_subtype_str_matches_serialization() {
        assert_eq!(ResultSubtype::Success.as_str(), "success");
        assert_eq!(ResultSubtype::MaxTurns.as_str(), "max_turns");
        assert_eq!(ResultSubtype::BudgetExceeded.as_str(), "budget_exceeded");
    }

    #[test]
    fn input_frame_parses_user_string_content() {
        let line = r#"{"type":"user","content":"hello"}"#;
        let frame: InputFrame = serde_json::from_str(line).unwrap();
        match frame {
            InputFrame::User { content } => {
                assert_eq!(InputFrame::extract_text(&content), "hello");
            }
            InputFrame::Control { .. } => panic!("expected user"),
        }
    }

    #[test]
    fn input_frame_parses_user_block_array() {
        let line = r#"{"type":"user","content":[{"type":"text","text":"abc"},{"type":"text","text":"def"}]}"#;
        let frame: InputFrame = serde_json::from_str(line).unwrap();
        match frame {
            InputFrame::User { content } => {
                assert_eq!(InputFrame::extract_text(&content), "abcdef");
            }
            InputFrame::Control { .. } => panic!("expected user"),
        }
    }

    #[test]
    fn input_frame_parses_control_interrupt() {
        let line = r#"{"type":"control","subtype":"interrupt"}"#;
        let frame: InputFrame = serde_json::from_str(line).unwrap();
        assert!(matches!(frame, InputFrame::Control { subtype } if subtype == "interrupt"));
    }

    #[test]
    fn input_frame_rejects_unknown_type() {
        let line = r#"{"type":"banana","content":"x"}"#;
        let res: Result<InputFrame, _> = serde_json::from_str(line);
        assert!(res.is_err());
    }
}
