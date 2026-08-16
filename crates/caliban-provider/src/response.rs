//! Completion response, usage, stop-reason.

use serde::{Deserialize, Serialize};

use crate::message::Message;

/// A complete, non-streaming response from a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// Provider-assigned response identifier.
    pub id: String,
    /// The model that produced the response.
    pub model: String,
    /// The assistant message produced.
    pub message: Message,
    /// Why the model stopped generating.
    pub stop_reason: StopReason,
    /// The stop sequence that triggered termination, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    /// Token usage for this response.
    pub usage: Usage,
}

/// Reason the model stopped generating tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model reached a natural stopping point.
    EndTurn,
    /// The `max_tokens` limit was reached.
    MaxTokens,
    /// A stop sequence was encountered.
    StopSequence,
    /// The model invoked a tool.
    ToolUse,
    /// A content-safety filter triggered.
    ContentFilter,
    /// The model produced a refusal.
    Refusal,
}

/// Token usage statistics for a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Total prompt tokens (the `OpenAI` convention), **inclusive** of any cached
    /// portion. Every provider adapter normalizes to this: for Anthropic the
    /// three disjoint wire counters are summed, and `cache_creation_input_tokens`
    /// / `cache_read_input_tokens` below are informational **subsets** of this
    /// value — never add them back on top of it (they were double-counted on the
    /// generation span before #493).
    pub input_tokens: u32,
    /// Tokens generated in the output.
    pub output_tokens: u32,
    /// Cache-creation tokens, if applicable. A subset of `input_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    /// Cache-read tokens, if applicable. A subset of `input_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}

impl Usage {
    /// Merge `other` into `self` by summing each field.
    pub fn merge(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens = match (
            self.cache_creation_input_tokens,
            other.cache_creation_input_tokens,
        ) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        self.cache_read_input_tokens =
            match (self.cache_read_input_tokens, other.cache_read_input_tokens) {
                (Some(a), Some(b)) => Some(a + b),
                (Some(a), None) | (None, Some(a)) => Some(a),
                (None, None) => None,
            };
    }
}
