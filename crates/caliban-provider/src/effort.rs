//! Reasoning-effort level shared by every provider that supports a
//! "thinking" / "reasoning" knob. Lives in `caliban-provider` (rather
//! than `caliban-agent-core`) to keep the conversion to provider-native
//! shapes (e.g. `OpenAI` `reasoning.effort`, Anthropic
//! `thinking.budget_tokens`) reachable from each adapter crate without
//! introducing a cyclic dependency on `caliban-agent-core`.

use serde::{Deserialize, Serialize};

/// Reasoning-effort level. Maps to provider-specific knobs:
/// - `OpenAI` `reasoning.effort`: `low`/`medium`/`high` (Max clamps to `high`).
/// - Anthropic `thinking.budget_tokens`: 2 048 / 8 192 / 24 576 / 64 000.
/// - `Auto` means "let the provider decide" — omit the field entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort.
    Medium,
    /// High reasoning effort.
    High,
    /// Maximum reasoning effort (Anthropic 64k tokens; `OpenAI` clamps to `high`).
    Max,
    /// Provider-default; the field is omitted in the request.
    #[default]
    Auto,
}

impl Effort {
    /// Map to the `OpenAI` `reasoning.effort` string. `Auto` returns `None`
    /// so the caller omits the field.
    #[must_use]
    pub fn as_openai(self) -> Option<&'static str> {
        match self {
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High | Self::Max => Some("high"),
            Self::Auto => None,
        }
    }

    /// Map to the Anthropic `thinking.budget_tokens` integer. `Auto`
    /// returns `None` so the caller omits the field.
    #[must_use]
    pub fn as_anthropic_budget(self) -> Option<u32> {
        match self {
            Self::Low => Some(2_048),
            Self::Medium => Some(8_192),
            Self::High => Some(24_576),
            Self::Max => Some(64_000),
            Self::Auto => None,
        }
    }
}
