//! Wire-format types for Anthropic Messages API responses.

use serde::{Deserialize, Serialize};

use super::request::NativeContentBlock;

/// Top-level response from the Anthropic Messages API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeResponse {
    /// Unique message identifier.
    pub id: String,
    /// The model that generated this response.
    pub model: String,
    /// Always `"assistant"`.
    pub role: String,
    /// Always `"message"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The generated content blocks.
    pub content: Vec<NativeContentBlock>,
    /// Reason the model stopped generating.
    pub stop_reason: NativeStopReason,
    /// The stop sequence that triggered the stop, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    /// Token usage for this request.
    pub usage: NativeUsage,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeStopReason {
    /// Natural end of the response.
    EndTurn,
    /// Hit the `max_tokens` limit.
    MaxTokens,
    /// A stop sequence was encountered.
    StopSequence,
    /// The model invoked a tool.
    ToolUse,
    /// The model issued a refusal.
    Refusal,
    /// Turn was paused (Bedrock-specific).
    PauseTurn,
}

/// Token usage counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NativeUsage {
    /// Tokens in the input. `#[serde(default)]` so a `message_delta.usage` that
    /// omits `input_tokens` (some API/proxy versions send only `output_tokens`)
    /// doesn't fail deserialization and discard the whole accumulated turn (#424).
    #[serde(default)]
    pub input_tokens: u32,
    /// Tokens in the output.
    #[serde(default)]
    pub output_tokens: u32,
    /// Tokens written to the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    /// Tokens read from the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}
