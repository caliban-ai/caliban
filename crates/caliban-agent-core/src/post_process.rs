//! Assistant-text post-processor hook + global tool-result size cap.
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
//!
//! This module also hosts [`ToolResultCap`], the global per-tool-result
//! size limiter invoked after parallel tool dispatch. Overflow content is
//! persisted to `<overflow_dir>/<session_id>/<tool_use_id>.txt`; the inline
//! block is replaced with a `[truncated: ...]` placeholder carrying a head
//! and tail preview so the model retains some context without paying for
//! the full payload.

use std::borrow::Cow;
use std::path::PathBuf;

use caliban_provider::{ContentBlock, TextBlock};

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

// ---------------------------------------------------------------------------
// ToolResultCap — global per-tool-result size limiter
// ---------------------------------------------------------------------------

/// Number of characters preserved at each end of an overflowed tool result.
const HEAD_TAIL_CHARS: usize = 2048;

/// Global per-tool-result size cap. Walks a batch of `ToolResult` blocks
/// after parallel dispatch; any block whose concatenated text exceeds
/// `max_chars` is rewritten in place with a `[truncated: ...]` placeholder
/// and head/tail preview, and the original is persisted to
/// `<overflow_dir>/<session_id>/<tool_use_id>.txt`.
///
/// Idempotent: blocks already starting with `[truncated:` or `[superseded:`
/// are left untouched.
pub struct ToolResultCap {
    /// Maximum characters allowed inline. `0` disables the cap entirely.
    pub max_chars: usize,
    /// Root directory where overflow files are written.
    pub overflow_dir: PathBuf,
    /// Session identifier (also used as the leaf directory under `overflow_dir`).
    pub session_id: String,
}

impl ToolResultCap {
    /// Walks the blocks and replaces oversized `ToolResult` content with a
    /// truncation placeholder + head/tail preview; writes the full original
    /// to `<overflow_dir>/<session_id>/<tool_use_id>.txt`.
    ///
    /// Returns the count of blocks that overflowed.
    ///
    /// # Errors
    ///
    /// Propagates filesystem errors from `mkdir` / `write`. Callers are
    /// free to treat these as non-fatal (the agent loop does).
    pub async fn cap(&self, blocks: &mut [ContentBlock]) -> std::io::Result<usize> {
        if self.max_chars == 0 {
            return Ok(0);
        }
        let session_dir = self.overflow_dir.join(&self.session_id);
        let mut overflows = 0;
        for block in blocks.iter_mut() {
            let ContentBlock::ToolResult(tr) = block else {
                continue;
            };
            // Skip already-truncated/superseded blocks (idempotent).
            if let Some(ContentBlock::Text(t)) = tr.content.first()
                && (t.text.starts_with("[truncated:") || t.text.starts_with("[superseded:"))
            {
                continue;
            }
            // Concatenate all text segments for the size check.
            let full: String = tr
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text(t) => Some(t.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            let full_chars = full.chars().count();
            if full_chars <= self.max_chars {
                continue;
            }
            tokio::fs::create_dir_all(&session_dir).await?;
            let path = session_dir.join(format!("{}.txt", tr.tool_use_id));
            tokio::fs::write(&path, &full).await?;
            let head: String = full.chars().take(HEAD_TAIL_CHARS).collect();
            let tail_start = full_chars.saturating_sub(HEAD_TAIL_CHARS);
            let tail: String = full.chars().skip(tail_start).collect();
            let placeholder = format!(
                "[truncated: {} chars, full content at {}]\n\n--- head 2KB ---\n{}\n--- tail 2KB ---\n{}",
                full_chars,
                path.display(),
                head,
                tail,
            );
            tr.content = vec![ContentBlock::Text(TextBlock {
                text: placeholder,
                cache_control: None,
            })];
            overflows += 1;
        }
        Ok(overflows)
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
