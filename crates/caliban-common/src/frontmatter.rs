//! Markdown YAML-frontmatter splitting.
//!
//! Several crates (skills, output-styles, memory) each hand-rolled the same
//! delimiter logic to peel a `---\n … \n---` YAML header off the front of a
//! markdown file. The copies had drifted in their closing-delimiter handling
//! (`.find` + `starts_with` vs `.rfind` + whitespace-tail), so trailing-content
//! edge cases parsed differently per crate. This consolidates the split into
//! one place; callers deserialize the returned YAML chunk into their own typed
//! `Frontmatter` struct and keep their own error mapping.
//!
//! The grammar is deliberately small and matches what every caller already
//! expected:
//! - An optional leading UTF-8 BOM is stripped.
//! - The file must start with `---\n` (the opening delimiter).
//! - The header runs up to the first `\n---\n` closing delimiter, or a
//!   trailing `\n---` at end-of-file (a `\n---` whose remainder is only
//!   whitespace).
//! - The body is everything after the closing delimiter (empty if none).

/// Why [`split`] could not separate frontmatter from body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontmatterError {
    /// The file did not begin with the `---\n` opening delimiter (after an
    /// optional BOM).
    MissingOpening,
    /// No closing `---` delimiter was found.
    MissingClosing,
}

impl FrontmatterError {
    /// A short human-readable reason, reused by callers when building their
    /// own richer error types.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::MissingOpening => "missing leading `---` frontmatter delimiter",
            Self::MissingClosing => "missing closing `---` frontmatter delimiter",
        }
    }
}

impl std::fmt::Display for FrontmatterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for FrontmatterError {}

const OPENING: &str = "---\n";
const STRICT_CLOSING: &str = "\n---\n";
const EOF_CLOSING: &str = "\n---";

/// Split `raw` into `(yaml_chunk, body)`.
///
/// `yaml_chunk` is the text between the delimiters (suitable for
/// `serde_yaml::from_str`); `body` is everything after the closing delimiter,
/// or `""` when the header runs to end-of-file.
///
/// # Errors
///
/// Returns [`FrontmatterError::MissingOpening`] when `raw` (after an optional
/// BOM) does not start with `---\n`, and [`FrontmatterError::MissingClosing`]
/// when no closing `---` delimiter is present.
pub fn split(raw: &str) -> Result<(&str, &str), FrontmatterError> {
    let trimmed = raw.trim_start_matches('\u{feff}');
    if !trimmed.starts_with(OPENING) {
        return Err(FrontmatterError::MissingOpening);
    }
    let after_open = &trimmed[OPENING.len()..];

    // Prefer the strict `\n---\n` form (the first occurrence, so a `---` line
    // inside the body never closes the header early). Fall back to a trailing
    // `\n---` at EOF whose remainder is only whitespace.
    if let Some(end) = after_open.find(STRICT_CLOSING) {
        let yaml = &after_open[..end];
        let body = &after_open[end + STRICT_CLOSING.len()..];
        return Ok((yaml, body));
    }
    if let Some(end) = after_open.rfind(EOF_CLOSING)
        && after_open[end + EOF_CLOSING.len()..]
            .chars()
            .all(char::is_whitespace)
    {
        let yaml = &after_open[..end];
        return Ok((yaml, ""));
    }
    Err(FrontmatterError::MissingClosing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_header_and_body() {
        let (yaml, body) = split("---\nname: x\ndescription: y\n---\nthe body\n").unwrap();
        assert_eq!(yaml, "name: x\ndescription: y");
        assert_eq!(body, "the body\n");
    }

    #[test]
    fn strips_leading_bom() {
        let (yaml, body) = split("\u{feff}---\nname: x\n---\nbody").unwrap();
        assert_eq!(yaml, "name: x");
        assert_eq!(body, "body");
    }

    #[test]
    fn tolerates_closing_at_eof_without_trailing_newline() {
        let (yaml, body) = split("---\nname: x\n---").unwrap();
        assert_eq!(yaml, "name: x");
        assert_eq!(body, "");
    }

    #[test]
    fn strict_close_keeps_trailing_whitespace_as_body() {
        // A genuine `\n---\n` close wins even when only whitespace follows —
        // that whitespace is the (whitespace-only) body, verbatim.
        let (yaml, body) = split("---\nname: x\n---\n  \n").unwrap();
        assert_eq!(yaml, "name: x");
        assert_eq!(body, "  \n");
    }

    #[test]
    fn tolerates_eof_close_with_trailing_whitespace_and_no_newline() {
        // No `\n---\n` here (the `---` line has trailing spaces, not a
        // newline), so the EOF fallback applies and the body is empty.
        let (yaml, body) = split("---\nname: x\n---   ").unwrap();
        assert_eq!(yaml, "name: x");
        assert_eq!(body, "");
    }

    #[test]
    fn first_strict_delimiter_closes_header_not_a_body_rule() {
        // A `---` line inside the body must not be mistaken for the closing
        // delimiter: the first `\n---\n` wins, the rest is body verbatim.
        let (yaml, body) = split("---\nname: x\n---\nintro\n---\nmore\n").unwrap();
        assert_eq!(yaml, "name: x");
        assert_eq!(body, "intro\n---\nmore\n");
    }

    #[test]
    fn empty_body_after_strict_close() {
        let (yaml, body) = split("---\nname: x\n---\n").unwrap();
        assert_eq!(yaml, "name: x");
        assert_eq!(body, "");
    }

    #[test]
    fn missing_opening_is_an_error() {
        assert_eq!(
            split("name: x\n---\nbody"),
            Err(FrontmatterError::MissingOpening)
        );
        assert_eq!(
            split("not frontmatter"),
            Err(FrontmatterError::MissingOpening)
        );
    }

    #[test]
    fn missing_closing_is_an_error() {
        assert_eq!(
            split("---\nname: x\nno closing"),
            Err(FrontmatterError::MissingClosing),
        );
    }
}
