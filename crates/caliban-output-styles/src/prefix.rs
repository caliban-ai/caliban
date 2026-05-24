//! `OutputStylePrefix` — splice the active style's body into the system prompt.

use crate::style::OutputStyle;

/// The output-style splice. Holds the currently-active style (or `None` for
/// the no-op default).
///
/// Composes with `caliban_memory::MemoryPrefix::splice_into`: memory tiers
/// are rendered first (highest precedence in the prompt cache key), the
/// output-style block goes second, and the base prompt body goes last.
#[derive(Debug, Clone, Default)]
pub struct OutputStylePrefix {
    /// Active style, or `None` for the no-op default.
    pub active: Option<OutputStyle>,
}

impl OutputStylePrefix {
    /// Wrap a style.
    #[must_use]
    pub const fn new(active: Option<OutputStyle>) -> Self {
        Self { active }
    }

    /// Render the style block (if any) and prepend it to `base`. When
    /// `active` is `None` *or* the style's body is empty (which is the case
    /// for the built-in `default` style), returns `base` unchanged so that
    /// the prompt-cache key is identical to "no style configured".
    #[must_use]
    pub fn splice_into(&self, base: &str) -> String {
        let Some(style) = self.active.as_ref() else {
            return base.to_string();
        };
        if style.body.trim().is_empty() {
            return base.to_string();
        }

        let mut out = String::with_capacity(base.len() + style.body.len() + 64);
        out.push_str("<output-style name=\"");
        out.push_str(&style.name);
        out.push_str("\">\n");
        out.push_str(&style.body);
        if !style.body.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("</output-style>\n\n");
        out.push_str(base);
        out
    }

    /// `true` iff the active style requests that the default coding-
    /// assistant guidance be dropped from the base prompt.
    #[must_use]
    pub fn drops_coding_instructions(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|s| !s.keep_coding_instructions)
    }
}
