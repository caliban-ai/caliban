//! Recovery flows for the turn loop:
//! - `MaxTokens` Stage A (budget escalation) + Stage B (meta-continuation).
//! - Reactive compaction on `ContextTooLong`.
//! - Refusal / `ContentFilter` synthetic-message surfacing.

/// Stage B meta-continuation prompt. Kept terse and model-neutral so 3P
/// providers don't get Anthropic-flavored copy.
pub(crate) const META_CONTINUATION_PROMPT: &str = "Output token limit hit. Resume directly \u{2014} no apology, no recap. \
     Pick up mid-thought. Break remaining work into smaller pieces.";

/// Synthetic message text for `stop_reason: Refusal`.
pub(crate) const REFUSAL_SYNTHETIC: &str = "Model declined to respond.";

/// Synthetic message text for `stop_reason: ContentFilter`.
pub(crate) const CONTENT_FILTER_SYNTHETIC: &str = "Response blocked by content filter.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_non_empty() {
        assert!(!META_CONTINUATION_PROMPT.is_empty());
        assert!(!REFUSAL_SYNTHETIC.is_empty());
        assert!(!CONTENT_FILTER_SYNTHETIC.is_empty());
    }
}
