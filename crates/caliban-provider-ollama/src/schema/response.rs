//! Wire-format types for Ollama `/api/chat` API responses.

use serde::{Deserialize, Serialize};

use super::request::NativeMessage;

/// Response body from `POST /api/chat` (both non-streaming final response and each streaming chunk).
///
/// For streaming, intermediate chunks have `done: false`; the final chunk has `done: true`
/// with `done_reason` and token-count fields populated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeResponse {
    /// The model that generated the response.
    pub model: String,
    /// ISO-8601 timestamp when the response was created.
    pub created_at: String,
    /// The generated or partial message.
    pub message: NativeMessage,
    /// Whether this is the final response chunk.
    pub done: bool,
    /// Why the model stopped (`"stop"`, `"length"`, `"tool_calls"`, etc.); only set when `done: true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_reason: Option<String>,
    /// Number of tokens in the prompt; only meaningful when `done: true`.
    #[serde(default)]
    pub prompt_eval_count: u32,
    /// Number of tokens generated; only meaningful when `done: true`.
    #[serde(default)]
    pub eval_count: u32,
}
