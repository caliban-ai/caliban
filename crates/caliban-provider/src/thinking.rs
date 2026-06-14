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

/// User-facing extended-thinking control, decoupled from the reasoning
/// [`crate::Effort`] hint (ticket #100).
///
/// Historically the thinking budget was derived *solely* from `Effort`, so
/// there was no way to force thinking on or off independently. This tri-state
/// is snapshotted onto every live [`crate::CompletionRequest`] and honored by
/// each provider's converter:
///
/// - [`ThinkingSetting::Auto`] — preserve the legacy behavior: derive thinking
///   from `Effort` (the request omits the field unless effort implies it).
/// - [`ThinkingSetting::Off`] — never request thinking/reasoning, even at high
///   effort.
/// - [`ThinkingSetting::On`] — force thinking on, with an optional explicit
///   token budget (`None` falls back to the effort-derived or a provider
///   default budget).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "mode", content = "budget_tokens")]
pub enum ThinkingSetting {
    /// Derive extended thinking from the `Effort` hint (legacy default).
    #[default]
    Auto,
    /// Explicitly disable extended thinking / reasoning, regardless of effort.
    Off,
    /// Explicitly enable extended thinking with an optional token budget.
    On(Option<u32>),
}
