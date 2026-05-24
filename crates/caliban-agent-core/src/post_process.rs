//! Assistant-text post-processor hook.
//!
//! Output styles (and, eventually, other extensions) may mutate the final
//! text of each assistant turn before it is appended to the conversation
//! history. The canonical use today is the `Learning` output style, which
//! inserts `TODO(human)` markers at inflection points so the user can fill
//! them in by hand.
//!
//! The trait lives here (rather than in `caliban-output-styles`) so other
//! crates — including plugin authors in the future — can implement it
//! without depending on the output-styles crate.

use std::borrow::Cow;

/// Mutate (or pass through) the final text of an assistant turn.
///
/// Called once per assistant message after streaming completes, before the
/// message is appended to the conversation history. Implementations must
/// be cheap; the post-processor runs on the hot path of every turn.
///
/// Identity implementations (e.g. for the `Default` style) should return
/// [`Cow::Borrowed`] to avoid allocating.
pub trait AssistantPostProcessor: Send + Sync {
    /// Process `text` and return either the original (borrowed) or a
    /// mutated (owned) version.
    fn process<'a>(&self, text: &'a str) -> Cow<'a, str>;
}

/// Default identity implementation. Returns the input unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopPostProcessor;

impl AssistantPostProcessor for NoopPostProcessor {
    fn process<'a>(&self, text: &'a str) -> Cow<'a, str> {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_returns_input_unchanged() {
        let p = NoopPostProcessor;
        let out = p.process("hello world");
        assert_eq!(out, "hello world");
        assert!(matches!(out, Cow::Borrowed(_)));
    }
}
