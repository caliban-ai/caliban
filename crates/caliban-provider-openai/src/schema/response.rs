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
