//! Wire-format types for Ollama `/api/chat` API requests.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level request body for `POST /api/chat`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeRequest {
    /// The model to use (e.g. `"llama3.1"`).
    pub model: String,
    /// The messages to send.
    pub messages: Vec<NativeMessage>,
    /// Tool definitions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<NativeTool>,
    /// Whether to stream the response.
    pub stream: bool,
    /// Optional output format (e.g. `"json"` or a JSON Schema object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Value>,
    /// Model inference options (temperature, `top_p`, etc.).
    #[serde(default, skip_serializing_if = "NativeOptions::is_empty")]
    pub options: NativeOptions,
    /// How long to keep the model loaded in memory (e.g. `"5m"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<String>,
}

/// A single message in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMessage {
    /// The role of the message author (`"system"`, `"user"`, `"assistant"`, or `"tool"`).
    pub role: String,
    /// The text content of the message.
    pub content: String,
    /// Base64-encoded images (no MIME prefix — Ollama infers type).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// Tool calls made by the assistant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<NativeToolCall>,
}

/// A tool call made by the assistant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolCall {
    /// The function that was called.
    pub function: NativeFunctionCall,
}

/// The function call payload in an assistant tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeFunctionCall {
    /// The function name.
    pub name: String,
    /// The arguments as a JSON object (NOT a JSON-encoded string, unlike `OpenAI`).
    pub arguments: Value,
}

/// A tool definition sent in the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeTool {
    /// Always `"function"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The function definition.
    pub function: NativeToolFunction,
}

/// The function portion of a tool definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolFunction {
    /// The function name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for the parameters.
    pub parameters: Value,
}

/// Model inference options (subset of what Ollama supports).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NativeOptions {
    /// Maximum number of tokens to predict (equivalent to `max_tokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<u32>,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling probability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Top-k sampling cutoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Stop sequences.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
}

impl NativeOptions {
    /// Returns `true` if all options are unset (used for `skip_serializing_if`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.num_predict.is_none()
            && self.temperature.is_none()
            && self.top_p.is_none()
            && self.top_k.is_none()
            && self.stop.is_empty()
    }
}
