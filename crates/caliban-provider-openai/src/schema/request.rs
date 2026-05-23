//! Wire-format types for `OpenAI` Chat Completions API requests.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level request body for `POST /chat/completions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeRequest {
    /// The model to use.
    pub model: String,
    /// The messages to send.
    pub messages: Vec<NativeMessage>,
    /// Tool definitions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<NativeTool>,
    /// Tool choice strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<NativeToolChoice>,
    /// Maximum tokens to generate (works for all current models).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Maximum completion tokens (o1+ preferred name; unused in B.4 scope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling probability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Stop sequences.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    /// An end-user identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Whether to stream the response.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    /// Options for streaming (e.g. `include_usage`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<NativeStreamOptions>,
}

/// Options passed alongside a streaming request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeStreamOptions {
    /// Whether to include token usage in the final streaming chunk.
    pub include_usage: bool,
}

/// A single message in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMessage {
    /// The role of the message author.
    pub role: String,
    /// Text or parts content; absent for pure tool-call assistant messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<NativeContent>,
    /// Tool calls made by the assistant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<NativeToolCall>,
    /// For `role:"tool"` messages, the ID of the tool call being responded to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// The name of the tool (used in some legacy patterns).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Content of a message: either a plain string or an array of content parts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativeContent {
    /// Plain text.
    Text(String),
    /// Multimodal parts array.
    Parts(Vec<NativeContentPart>),
}

/// A single part in a multimodal content array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeContentPart {
    /// Text part.
    Text {
        /// The text content.
        text: String,
    },
    /// Image URL part.
    ImageUrl {
        /// The image URL (or data URI).
        image_url: NativeImageUrl,
    },
}

/// An image URL payload within a content part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeImageUrl {
    /// The URL or data URI for the image.
    pub url: String,
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

/// Tool choice — either a string keyword or a specific function name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativeToolChoice {
    /// A keyword: `"auto"`, `"required"`, or `"none"`.
    Auto(String),
    /// A specific function to call.
    Specific {
        /// Always `"function"`.
        #[serde(rename = "type")]
        kind: String,
        /// The function to call.
        function: NativeToolFunctionName,
    },
}

/// Identifies a specific function by name for `tool_choice`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeToolFunctionName {
    /// The function name.
    pub name: String,
}

/// A tool call made by the assistant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolCall {
    /// Unique identifier for the tool call.
    pub id: String,
    /// Always `"function"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The function that was called.
    pub function: NativeFunctionCall,
}

/// The function call payload in an assistant tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeFunctionCall {
    /// The function name.
    pub name: String,
    /// The arguments as a JSON-encoded string (not a parsed object).
    pub arguments: String,
}
