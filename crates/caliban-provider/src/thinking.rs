//! Extended-thinking IR.

use serde::{Deserialize, Serialize};

/// A thinking block produced by a model during extended reasoning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingBlock {
    /// The thinking text produced by the model.
    pub thinking: String,
    /// Optional opaque signature attached by the provider.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub signature: Option<String>,
}

/// Configuration to enable extended thinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingConfig {
    /// Maximum tokens the model may use for thinking.
    pub budget_tokens: u32,
}
