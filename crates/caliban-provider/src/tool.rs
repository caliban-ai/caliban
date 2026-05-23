//! Tool-use IR: declarations, calls, results.

use serde::{Deserialize, Serialize};

use crate::cache::CacheControl;
use crate::message::ContentBlock;

/// Declaration of a tool available to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    /// Machine-readable tool name.
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// JSON Schema describing the tool's input parameters.
    pub input_schema: serde_json::Value,
    /// Optional cache-control marker.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_control: Option<CacheControl>,
}

/// A tool-use invocation produced by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolUseBlock {
    /// Unique call identifier assigned by the provider.
    pub id: String,
    /// Name of the tool being called.
    pub name: String,
    /// Input arguments as a JSON value.
    pub input: serde_json::Value,
}

/// The result of executing a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultBlock {
    /// ID of the corresponding `ToolUseBlock`.
    pub tool_use_id: String,
    /// Content blocks returned by the tool.
    pub content: Vec<ContentBlock>,
    /// Whether this result represents an error.
    #[serde(default)]
    pub is_error: bool,
}

/// How the model should choose among available tools.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    /// Model decides whether to call a tool.
    #[default]
    Auto,
    /// Model must call at least one tool.
    Any,
    /// Model must call the named tool.
    Specific {
        /// Name of the required tool.
        name: String,
    },
    /// Model must not call any tool.
    None,
}
