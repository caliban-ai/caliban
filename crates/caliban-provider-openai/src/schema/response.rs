//! Wire-format types for `OpenAI` Chat Completions API responses.

use serde::{Deserialize, Serialize};

use super::request::NativeToolCall;

/// Top-level response from `POST /chat/completions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeResponse {
    /// Unique response identifier.
    pub id: String,
    /// The model that generated the response.
    pub model: String,
    /// The list of completion choices (we use `choices[0]`).
    pub choices: Vec<NativeChoice>,
    /// Token usage statistics.
    #[serde(default)]
    pub usage: NativeUsage,
}

/// A single completion choice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeChoice {
    /// The choice index (always 0 for n=1).
    pub index: u32,
    /// The generated message.
    pub message: NativeResponseMessage,
    /// Why the model stopped generating.
    pub finish_reason: NativeFinishReason,
}

/// The message returned in a completion choice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeResponseMessage {
    /// The role (`"assistant"`).
    pub role: String,
    /// Text content; absent when the response is pure tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Full reasoning trace from reasoning-family models (Qwen3.x reasoning
    /// variants, DeepSeek-R1, etc.). Captured so the field doesn't get
    /// silently dropped on non-streaming responses; consumers may surface it
    /// as a Thinking block in future.
    ///
    /// The `reasoning` alias captures MLX-based servers (e.g. `mlx_lm.server`),
    /// which use `reasoning` rather than the `reasoning_content` field llama.cpp
    /// uses (see ADR 0056).
    #[serde(default, alias = "reasoning", skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Tool calls issued by the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<NativeToolCall>,
    /// A refusal string from the model's safety layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

/// Why the model stopped generating tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeFinishReason {
    /// Natural end of the response.
    Stop,
    /// `max_tokens` limit reached.
    Length,
    /// The model issued one or more tool calls.
    ToolCalls,
    /// Content was filtered.
    ContentFilter,
    /// Legacy function-call finish reason.
    FunctionCall,
}

/// Token usage statistics for a completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NativeUsage {
    /// Tokens in the prompt.
    pub prompt_tokens: u32,
    /// Tokens in the completion.
    pub completion_tokens: u32,
    /// Total tokens (prompt + completion).
    #[serde(default)]
    pub total_tokens: u32,
    /// Breakdown of prompt token categories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<NativePromptTokensDetails>,
}

/// Breakdown of prompt token categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NativePromptTokensDetails {
    /// Tokens served from the prompt cache.
    pub cached_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::NativeResponseMessage;

    #[test]
    fn response_message_accepts_reasoning_alias() {
        // MLX servers return the trace under `reasoning`; the serde alias must
        // map it onto reasoning_content, matching the streaming path (ADR 0056).
        let j = r#"{"role":"assistant","reasoning":"pondering","content":"hi"}"#;
        let m: NativeResponseMessage = serde_json::from_str(j).unwrap();
        assert_eq!(m.reasoning_content.as_deref(), Some("pondering"));
        assert_eq!(m.content.as_deref(), Some("hi"));

        // The canonical field name still deserializes unchanged.
        let j2 = r#"{"role":"assistant","reasoning_content":"pondering"}"#;
        let m2: NativeResponseMessage = serde_json::from_str(j2).unwrap();
        assert_eq!(m2.reasoning_content.as_deref(), Some("pondering"));
    }
}
