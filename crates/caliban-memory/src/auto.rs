//! Per-project auto-memory: topic file enumerator, reader, and writer.
//!
//! See `docs/superpowers/specs/2026-05-24-auto-memory-design.md` and
//! `docs/adr/0035-auto-memory.md`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{MemoryError, Result};

/// The four memory-type categories the model classifies a topic file under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicKind {
    /// Durable facts about the user.
    User,
    /// User-issued corrections / preferences for future interactions.
    Feedback,
    /// Durable project facts not already in the repo.
    Project,
    /// Stable external context (account IDs, URLs, API quotas).
    Reference,
}

impl TopicKind {
    /// Lower-case wire form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }

    /// Parse from a string, accepting case-insensitively. Returns `None` for
    /// any input that is not one of the four valid types.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "user" => Some(Self::User),
            "feedback" => Some(Self::Feedback),
            "project" => Some(Self::Project),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }
}

/// Lightweight summary of a topic file (frontmatter only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicSummary {
    /// The slug (kebab-case, must match filename stem).
    pub name: String,
    /// One-line description (≤ 120 chars by convention).
    pub description: String,
    /// Memory type classification.
    pub kind: TopicKind,
    /// Absolute path to the topic file.
    pub path: PathBuf,
}

/// A fully-loaded topic file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicFile {
    /// The slug (kebab-case).
    pub name: String,
    /// One-line description.
    pub description: String,
    /// Memory type classification.
    pub kind: TopicKind,
    /// Markdown body (everything after the closing frontmatter `---`).
    pub body: String,
    /// Absolute path.
    pub path: PathBuf,
}

impl TopicSummary {
    /// Parse a topic summary from raw file text (frontmatter only).
    pub(crate) fn parse(raw: &str, path: &Path) -> Result<TopicSummary> {
        let (fm, _) = parse_frontmatter(raw, path)?;
        let kind =
            TopicKind::parse(fm.metadata.kind.as_deref().unwrap_or("")).ok_or_else(|| {
                MemoryError::InvalidTopic {
                    path: path.to_path_buf(),
                    reason: format!(
                        "metadata.type must be one of user|feedback|project|reference (got {:?})",
                        fm.metadata.kind
                    ),
                }
            })?;
        Ok(TopicSummary {
            name: fm.name,
            description: fm.description,
            kind,
            path: path.to_path_buf(),
        })
    }
}

impl TopicFile {
    /// Parse a full topic (frontmatter + body) from raw file text.
    pub(crate) fn parse(raw: &str, path: &Path) -> Result<TopicFile> {
        let (_, body) = parse_frontmatter(raw, path)?;
        let s = TopicSummary::parse(raw, path)?;
        Ok(TopicFile {
            name: s.name,
            description: s.description,
            kind: s.kind,
            body: body.to_string(),
            path: s.path,
        })
    }
}

/// Draft passed to [`crate::TopicLoader::write`]. The loader fills in path /
/// on-disk frontmatter from these fields.
#[derive(Debug, Clone)]
pub struct TopicDraft {
    /// The slug (kebab-case). Must pass [`validate_slug`].
    pub name: String,
    /// One-line description for the `MEMORY.md` index entry + frontmatter.
    pub description: String,
    /// Memory type classification.
    pub kind: TopicKind,
    /// Raw markdown body (no frontmatter — the loader emits it).
    pub body: String,
}

/// Frontmatter shape used by [`crate::TopicLoader::read`] / [`crate::TopicLoader::list`].
#[derive(Debug, Deserialize)]
struct RawFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    metadata: RawMetadata,
}

#[derive(Debug, Default, Deserialize)]
struct RawMetadata {
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// Validate a topic slug. Rules: non-empty, no path separators (`/`, `\\`),
/// no `..`, no leading dot.
///
/// # Errors
///
/// Returns [`MemoryError::InvalidSlug`] if the slug fails any rule.
pub fn validate_slug(slug: &str) -> Result<()> {
    if slug.is_empty() {
        return Err(MemoryError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug must be non-empty".into(),
        });
    }
    if slug.contains('/') || slug.contains('\\') {
        return Err(MemoryError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug must not contain path separators".into(),
        });
    }
    if slug.contains("..") {
        return Err(MemoryError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug must not contain '..'".into(),
        });
    }
    if slug.starts_with('.') {
        return Err(MemoryError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug must not start with '.'".into(),
        });
    }
    if slug.contains('\0') {
        return Err(MemoryError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug must not contain NUL".into(),
        });
    }
    Ok(())
}

/// Split a raw file into frontmatter struct + body. Frontmatter delimiters are
/// `---\n` opening and `\n---\n` (or `\n---` at EOF) closing.
fn parse_frontmatter<'a>(raw: &'a str, path: &Path) -> Result<(RawFrontmatter, &'a str)> {
    let (yaml_chunk, body) =
        caliban_common::frontmatter::split(raw).map_err(|e| MemoryError::InvalidTopic {
            path: path.to_path_buf(),
            reason: e.reason().into(),
        })?;
    let fm: RawFrontmatter =
        serde_yaml::from_str(yaml_chunk).map_err(|e| MemoryError::InvalidTopic {
            path: path.to_path_buf(),
            reason: format!("yaml: {e}"),
        })?;
    if fm.name.trim().is_empty() {
        return Err(MemoryError::InvalidTopic {
            path: path.to_path_buf(),
            reason: "name must be non-empty".into(),
        });
    }
    if fm.description.trim().is_empty() {
        return Err(MemoryError::InvalidTopic {
            path: path.to_path_buf(),
            reason: "description must be non-empty".into(),
        });
    }
    Ok((fm, body))
}

/// Render a [`TopicDraft`] to on-disk markdown (frontmatter + body).
pub(crate) fn render_topic_file(draft: &TopicDraft) -> String {
    let mut out = String::with_capacity(draft.body.len() + 256);
    out.push_str("---\n");
    out.push_str("name: ");
    out.push_str(&draft.name);
    out.push('\n');
    out.push_str("description: \"");
    out.push_str(&escape_yaml_string(&draft.description));
    out.push_str("\"\n");
    out.push_str("metadata:\n");
    out.push_str("  node_type: memory\n");
    out.push_str("  type: ");
    out.push_str(draft.kind.as_str());
    out.push('\n');
    out.push_str("---\n\n");
    out.push_str(&draft.body);
    if !draft.body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Escape `"` and `\` for a double-quoted YAML scalar.
fn escape_yaml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// Strip every `<!-- … -->` HTML comment (greedy, multi-line) from `body`.
/// Used by the memory loader before splicing into the system prompt — the
/// on-disk file is untouched.
#[must_use]
pub fn strip_html_comments(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 3 < bytes.len() && &bytes[i..i + 4] == b"<!--" {
            // find closing -->; if not found, drop the rest.
            if let Some(end) = find_subslice(&bytes[i + 4..], b"-->") {
                i += 4 + end + 3;
                continue;
            }
            break;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    for i in 0..=hay.len() - needle.len() {
        if &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic_md(name: &str, kind: &str, desc: &str, body: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: \"{desc}\"\nmetadata:\n  node_type: memory\n  type: {kind}\n---\n\n{body}\n",
        )
    }

    #[test]
    fn validate_slug_rejects_path_traversal() {
        assert!(validate_slug("ok").is_ok());
        assert!(validate_slug("ok-slug_1").is_ok());
        assert!(validate_slug("").is_err());
        assert!(validate_slug("a/b").is_err());
        assert!(validate_slug("a\\b").is_err());
        assert!(validate_slug("..").is_err());
        assert!(validate_slug("a..b").is_err());
        assert!(validate_slug(".hidden").is_err());
    }

    #[test]
    fn strip_html_comments_handles_single_and_multiline() {
        let single = "hello <!-- inline --> world";
        assert_eq!(strip_html_comments(single), "hello  world");

        let multi = "before\n<!-- line one\nline two\n-->\nafter";
        let stripped = strip_html_comments(multi);
        assert!(stripped.contains("before"));
        assert!(stripped.contains("after"));
        assert!(!stripped.contains("line one"));
        assert!(!stripped.contains("line two"));
    }

    // --- TopicKind ----------------------------------------------------------

    #[test]
    fn topic_kind_as_str_covers_all_variants() {
        assert_eq!(TopicKind::User.as_str(), "user");
        assert_eq!(TopicKind::Feedback.as_str(), "feedback");
        assert_eq!(TopicKind::Project.as_str(), "project");
        assert_eq!(TopicKind::Reference.as_str(), "reference");
    }

    #[test]
    fn topic_kind_parse_is_case_and_whitespace_insensitive() {
        assert_eq!(TopicKind::parse("USER"), Some(TopicKind::User));
        assert_eq!(TopicKind::parse("  Feedback  "), Some(TopicKind::Feedback));
        assert_eq!(TopicKind::parse("Project"), Some(TopicKind::Project));
        assert_eq!(TopicKind::parse("rEfErEnCe"), Some(TopicKind::Reference));
    }

    #[test]
    fn topic_kind_parse_rejects_unknown_and_empty() {
        assert_eq!(TopicKind::parse(""), None);
        assert_eq!(TopicKind::parse("   "), None);
        assert_eq!(TopicKind::parse("junk"), None);
    }

    // --- parse_frontmatter edge cases ---------------------------------------

    #[test]
    fn parse_frontmatter_strips_bom() {
        let raw = format!(
            "\u{feff}{}",
            topic_md("bom", "user", "with bom", "body line")
        );
        let path = Path::new("bom.md");
        let (fm, body) = parse_frontmatter(&raw, path).unwrap();
        assert_eq!(fm.name, "bom");
        assert!(body.contains("body line"));
    }

    #[test]
    fn parse_frontmatter_rejects_missing_leading_delimiter() {
        let raw = "name: x\ndescription: y\n---\nbody\n";
        let err = parse_frontmatter(raw, Path::new("x.md")).unwrap_err();
        match err {
            MemoryError::InvalidTopic { reason, .. } => assert!(reason.contains("leading")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_frontmatter_rejects_missing_closing_delimiter() {
        let raw = "---\nname: x\ndescription: y\nno closing here\n";
        let err = parse_frontmatter(raw, Path::new("x.md")).unwrap_err();
        match err {
            MemoryError::InvalidTopic { reason, .. } => assert!(reason.contains("closing")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_frontmatter_accepts_closing_delimiter_at_eof_without_body() {
        // Closing `\n---` with no trailing newline and no body.
        let raw = "---\nname: eof\ndescription: d\nmetadata:\n  type: user\n---";
        let (fm, body) = parse_frontmatter(raw, Path::new("eof.md")).unwrap();
        assert_eq!(fm.name, "eof");
        assert_eq!(body, "");
    }

    #[test]
    fn parse_frontmatter_rejects_empty_name() {
        let raw = "---\nname: \"  \"\ndescription: d\n---\nbody\n";
        let err = parse_frontmatter(raw, Path::new("x.md")).unwrap_err();
        match err {
            MemoryError::InvalidTopic { reason, .. } => assert!(reason.contains("name")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_frontmatter_rejects_empty_description() {
        let raw = "---\nname: x\ndescription: \"  \"\n---\nbody\n";
        let err = parse_frontmatter(raw, Path::new("x.md")).unwrap_err();
        match err {
            MemoryError::InvalidTopic { reason, .. } => assert!(reason.contains("description")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_frontmatter_rejects_invalid_yaml() {
        let raw = "---\nname: [unbalanced\ndescription: d\n---\nbody\n";
        let err = parse_frontmatter(raw, Path::new("x.md")).unwrap_err();
        match err {
            MemoryError::InvalidTopic { reason, .. } => assert!(reason.contains("yaml")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    // --- render_topic_file / escape_yaml_string -----------------------------

    #[test]
    fn render_topic_file_appends_trailing_newline_when_missing() {
        let draft = TopicDraft {
            name: "no-nl".to_string(),
            description: "desc".to_string(),
            kind: TopicKind::Project,
            body: "body without newline".to_string(),
        };
        let rendered = render_topic_file(&draft);
        assert!(rendered.ends_with("body without newline\n"));
        assert!(rendered.contains("type: project"));
    }

    #[test]
    fn render_topic_file_preserves_single_trailing_newline() {
        let draft = TopicDraft {
            name: "has-nl".to_string(),
            description: "desc".to_string(),
            kind: TopicKind::User,
            body: "body\n".to_string(),
        };
        let rendered = render_topic_file(&draft);
        // Exactly one trailing newline (no double newline appended).
        assert!(rendered.ends_with("body\n"));
        assert!(!rendered.ends_with("body\n\n"));
    }

    #[test]
    fn escape_yaml_string_escapes_special_chars() {
        assert_eq!(escape_yaml_string("a\"b"), "a\\\"b");
        assert_eq!(escape_yaml_string("a\\b"), "a\\\\b");
        assert_eq!(escape_yaml_string("a\nb"), "a\\nb");
        assert_eq!(escape_yaml_string("a\rb"), "a\\rb");
        assert_eq!(escape_yaml_string("plain"), "plain");
    }

    // --- validate_slug NUL --------------------------------------------------

    #[test]
    fn validate_slug_rejects_nul() {
        let err = validate_slug("a\0b").unwrap_err();
        match err {
            MemoryError::InvalidSlug { reason, .. } => assert!(reason.contains("NUL")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    // --- strip_html_comments edge cases -------------------------------------

    #[test]
    fn strip_html_comments_drops_unterminated_comment_tail() {
        let input = "keep me <!-- never closed";
        let out = strip_html_comments(input);
        assert_eq!(out, "keep me ");
    }

    #[test]
    fn strip_html_comments_no_comment_is_identity() {
        let input = "plain text with < and > but no comment";
        assert_eq!(strip_html_comments(input), input);
    }
}
