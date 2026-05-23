//! Wire-format types for Anthropic Messages API requests.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The top-level request body for the Anthropic Messages API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeRequest {
    /// The model identifier.
    pub model: String,
    /// The conversation messages.
    pub messages: Vec<NativeMessage>,
    /// Optional system prompt (string or block array).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<NativeSystem>,
    /// Tool definitions available to the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<NativeTool>,
    /// How the model should choose a tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<NativeToolChoice>,
    /// Maximum tokens to generate.
    pub max_tokens: u32,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling probability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Top-k sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Stop sequences.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    /// Extended-thinking configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<NativeThinking>,
    /// Per-request metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<NativeMetadata>,
    /// Whether to stream the response.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    /// Bedrock requires this field; Direct/Vertex ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_version: Option<String>,
}

/// System prompt representation — either a plain string or a list of text blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativeSystem {
    /// Simple string system prompt.
    Text(String),
    /// Structured text blocks (supports `cache_control`).
    Blocks(Vec<NativeTextBlock>),
}

/// A single conversation message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMessage {
    /// Role — `"user"` or `"assistant"`.
    pub role: String,
    /// Message content — string or block array.
    pub content: NativeContent,
}

/// Message content — either a plain string or structured blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativeContent {
    /// Plain text shorthand.
    Text(String),
    /// Structured content blocks.
    Blocks(Vec<NativeContentBlock>),
}

/// A typed content block in a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeContentBlock {
    /// A text block.
    Text(NativeTextBlock),
    /// An image block.
    Image(NativeImageBlock),
    /// A tool-use invocation block.
    ToolUse(NativeToolUseBlock),
    /// A tool-result block.
    ToolResult(NativeToolResultBlock),
    /// An extended-thinking block.
    Thinking(NativeThinkingBlock),
    /// A redacted-thinking block (server-generated).
    RedactedThinking {
        /// Opaque redacted data.
        data: String,
    },
}

/// A plain-text content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeTextBlock {
    /// The text content.
    pub text: String,
    /// Optional cache-control marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<NativeCacheControl>,
}

/// An image content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeImageBlock {
    /// The image source.
    pub source: NativeImageSource,
    /// Optional cache-control marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<NativeCacheControl>,
}

/// Image source — base64 or URL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeImageSource {
    /// Base64-encoded image.
    Base64 {
        /// MIME type.
        media_type: String,
        /// Base64 data.
        data: String,
    },
    /// URL-referenced image.
    Url {
        /// The image URL.
        url: String,
    },
}

/// A tool-use invocation block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolUseBlock {
    /// Unique tool-use ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Tool input as a JSON object.
    pub input: Value,
}

/// A tool-result block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolResultBlock {
    /// Matches the `id` of the corresponding tool-use block.
    pub tool_use_id: String,
    /// Result content.
    pub content: NativeContent,
    /// Whether this result represents an error.
    #[serde(default)]
    pub is_error: bool,
}

/// An extended-thinking block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeThinkingBlock {
    /// The thinking text.
    pub thinking: String,
    /// Optional signature for the thinking block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Cache-control marker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeCacheControl {
    /// Ephemeral cache breakpoint.
    Ephemeral,
}

/// A tool definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeTool {
    /// Tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for the tool's input.
    pub input_schema: Value,
    /// Optional cache-control marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<NativeCacheControl>,
}

/// How the model should select a tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeToolChoice {
    /// The model chooses automatically.
    Auto,
    /// The model must use at least one tool.
    Any,
    /// The model must use the named tool.
    Tool {
        /// The required tool name.
        name: String,
    },
    /// The model must not use any tool.
    None,
}

/// Extended-thinking configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeThinking {
    /// Must be `"enabled"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Token budget for thinking.
    pub budget_tokens: u32,
}

/// Per-request metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMetadata {
    /// Caller-supplied user identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}
