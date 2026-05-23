//! Wire-format types for Google Gemini API requests.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level request body for `POST /{version}/models/{model}:generateContent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRequest {
    /// The conversation history.
    pub contents: Vec<NativeContent>,
    /// System instruction (separate from conversation contents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<NativeSystemInstruction>,
    /// Tool definitions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<NativeToolList>,
    /// Tool calling configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<NativeToolConfig>,
    /// Generation configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<NativeGenerationConfig>,
}

/// A single turn in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeContent {
    /// The role: `"user"` or `"model"`.
    pub role: String,
    /// The list of parts in this message.
    pub parts: Vec<NativePart>,
}

/// A single part within a message content.
///
/// Parts are tagged objects; each variant corresponds to one type of content.
/// Custom serialization is used because Gemini uses `{"text": ...}` style
/// single-key objects rather than a `type` discriminator.
#[derive(Debug, Clone, PartialEq)]
pub enum NativePart {
    /// Plain text content.
    Text(String),
    /// Inline base64-encoded binary data.
    InlineData(NativeInlineData),
    /// A file reference by URI (Vertex AI only; not supported on AI Studio).
    FileData(NativeFileData),
    /// A function call issued by the model.
    FunctionCall(NativeFunctionCall),
    /// A function response provided by the user.
    FunctionResponse(NativeFunctionResponse),
}

impl Serialize for NativePart {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        match self {
            NativePart::Text(s) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("text", s)?;
                map.end()
            }
            NativePart::InlineData(d) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("inlineData", d)?;
                map.end()
            }
            NativePart::FileData(d) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("fileData", d)?;
                map.end()
            }
            NativePart::FunctionCall(fc) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("functionCall", fc)?;
                map.end()
            }
            NativePart::FunctionResponse(fr) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("functionResponse", fr)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for NativePart {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let map = serde_json::Map::deserialize(deserializer)?;
        if let Some(v) = map.get("text") {
            let s = v
                .as_str()
                .ok_or_else(|| serde::de::Error::custom("text must be a string"))?;
            return Ok(NativePart::Text(s.to_string()));
        }
        if let Some(v) = map.get("inlineData") {
            let d: NativeInlineData =
                serde_json::from_value(v.clone()).map_err(serde::de::Error::custom)?;
            return Ok(NativePart::InlineData(d));
        }
        if let Some(v) = map.get("fileData") {
            let d: NativeFileData =
                serde_json::from_value(v.clone()).map_err(serde::de::Error::custom)?;
            return Ok(NativePart::FileData(d));
        }
        if let Some(v) = map.get("functionCall") {
            let fc: NativeFunctionCall =
                serde_json::from_value(v.clone()).map_err(serde::de::Error::custom)?;
            return Ok(NativePart::FunctionCall(fc));
        }
        if let Some(v) = map.get("functionResponse") {
            let fr: NativeFunctionResponse =
                serde_json::from_value(v.clone()).map_err(serde::de::Error::custom)?;
            return Ok(NativePart::FunctionResponse(fr));
        }
        Err(serde::de::Error::custom(
            "NativePart: unrecognized part type in object",
        ))
    }
}

/// Inline binary data (base64-encoded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeInlineData {
    /// MIME type of the data (e.g. `"image/png"`).
    pub mime_type: String,
    /// Base64-encoded data.
    pub data: String,
}

/// A file reference by URI (Vertex AI only).
///
/// AI Studio does not support `fileData` parts; use [`NativeInlineData`] instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeFileData {
    /// MIME type of the referenced file (e.g. `"image/png"`).
    pub mime_type: String,
    /// A URI pointing to the file (e.g. an HTTPS or GCS URL).
    pub file_uri: String,
}

/// A function call issued by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeFunctionCall {
    /// The function name.
    pub name: String,
    /// The function arguments as a JSON object.
    pub args: Value,
}

/// A function response provided by the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeFunctionResponse {
    /// The function name (correlates to the corresponding `functionCall`).
    pub name: String,
    /// The response body as a JSON object.
    pub response: Value,
}

/// System instruction passed outside the `contents` array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeSystemInstruction {
    /// The parts of the system instruction (typically a single text part).
    pub parts: Vec<NativePart>,
}

/// A list of function declarations (tools) sent in the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeToolList {
    /// The function declarations.
    pub function_declarations: Vec<NativeFunctionDeclaration>,
}

/// A single tool (function) declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeFunctionDeclaration {
    /// The function name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for the parameters.
    pub parameters: Value,
}

/// Tool calling configuration for the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeToolConfig {
    /// Configuration for function calling.
    pub function_calling_config: NativeFunctionCallingConfig,
}

/// Function calling mode configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeFunctionCallingConfig {
    /// The mode: `"AUTO"`, `"ANY"`, or `"NONE"`.
    pub mode: String,
    /// Restrict to these function names when mode is `"ANY"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_function_names: Vec<String>,
}

/// Generation parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeGenerationConfig {
    /// Maximum number of output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
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
}
